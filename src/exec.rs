//! Nushell command execution with background process support

use crate::state::{AppState, ProcessStatus, push_truncated};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, error, info};
use schemars::JsonSchema;

/// NuExec tool arguments
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NuExecArgs {
    /// The Nushell pipeline to execute. Example: 'ls src | to json' or 'cargo build'.
    pub command: String,
    /// Set to true for long-running tasks (servers, watchers). Returns a job ID immediately.
    #[serde(default)]
    pub background: bool,
    /// Working directory for the command (optional, defaults to current directory).
    pub cwd: Option<String>,
    /// Environment variables to set for the command (optional).
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Timeout in seconds for blocking execution (optional, default 60).
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// NuOutput tool arguments
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NuOutputArgs {
    /// The job ID returned by a background `nu.exec` call.
    pub id: String,
    /// If true, waits for the process to exit before returning final logs (max 5 minutes).
    #[serde(default)]
    pub block: Option<bool>,
}

/// NuKill tool arguments
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NuKillArgs {
    /// The job ID of the background process to terminate.
    pub id: String,
}

/// NuApply tool arguments
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NuApplyArgs {
    /// Absolute path to the file to edit.
    pub path: String,
    /// Brief first-person description of the change to disambiguate the edit.
    pub instructions: String,
    /// The partial code with `// ... existing code ...` markers.
    pub code_edit: String,
}

/// Nushell executor
#[derive(Clone)]
pub struct NuExecutor {
    pub nu_path: String,
    pub default_timeout_sec: u64,
}

impl NuExecutor {
    pub fn new(nu_path: String, _initial_cwd: String) -> Self {
        Self {
            nu_path,
            default_timeout_sec: 60,
        }
    }

    /// Get timeout
    pub fn resolve_timeout(&self, timeout: Option<u64>) -> Duration {
        timeout
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(self.default_timeout_sec))
    }

    /// Execute command (blocking mode) with stateful CWD tracking
    /// Uses active pipe draining and kill-on-timeout to prevent hangs.
    pub async fn exec_blocking(
        &self,
        state: &AppState,
        command: &str,
        env: &HashMap<String, String>,
        timeout: Duration,
    ) -> anyhow::Result<NuExecResult> {
        let start = std::time::Instant::now();
        let cwd = state.get_cwd().await;
        debug!("Executing blocking in {}: {}", cwd, command);

        // Robust CWD wrapper: use 'try' to handle deleted directories gracefully
        // Nushell syntax: single quotes for path safety, print for output
        let sentinel = ":::CWD:::";
        let full_command = format!("try {{ cd '{}' }}; {}; print $\"{}(pwd)\"", cwd, command, sentinel);

        let mut cmd = Command::new(&self.nu_path);
        cmd.arg("-c").arg(&full_command);
        for (k, v) in env {
            cmd.env(k, v);
        }

        // Spawn the process and take pipes immediately
        // CRITICAL: Set stdin to null to prevent child from blocking waiting for input
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("Failed to take stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("Failed to take stderr"))?;

        // Use Arc<Mutex<String>> for shared buffers between tasks
        let stdout_buf = Arc::new(TokioMutex::new(String::new()));
        let stderr_buf = Arc::new(TokioMutex::new(String::new()));

        // Spawn tasks to actively drain pipes into shared buffers
        let stdout_task = {
            let buf = stdout_buf.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut b = buf.lock().await;
                    push_truncated(&mut b, &format!("{}\n", line), 200_000);
                }
            })
        };

        let stderr_task = {
            let buf = stderr_buf.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut b = buf.lock().await;
                    push_truncated(&mut b, &format!("{}\n", line), 50_000);
                }
            })
        };

        // Race between: timeout, child exit, and pipe draining
        let (exit_code, timed_out) = tokio::select! {
            // Timer expires first - abort drains and kill the child
            _ = tokio::time::sleep(timeout) => {
                debug!("Command timed out after {:?}", timeout);
                stdout_task.abort();
                stderr_task.abort();
                let _ = child.kill().await;
                (-1, true)
            }
            // Child exits first - wait for drains to complete
            result = child.wait() => {
                let code = match result {
                    Ok(status) => status.code().unwrap_or(-1),
                    Err(e) => {
                        error!("Child wait error: {:?}", e);
                        -1
                    }
                };
                // Give drain tasks a moment to finish collecting all output
                let _ = tokio::time::timeout(Duration::from_secs(1), stdout_task).await;
                let _ = tokio::time::timeout(Duration::from_secs(1), stderr_task).await;
                (code, false)
            }
        };

        let took_ms = start.elapsed().as_millis();

        // Extract the final buffer contents
        let stdout_final = stdout_buf.lock().await.clone();
        let stderr_final = stderr_buf.lock().await.clone();

        // Extract CWD from sentinel and return clean output
        // Sentinel is on its own line: ":::CWD:::/path/to/dir"
        // Everything before sentinel is user output, everything after is CWD (trimmed)
        let (stdout, new_cwd) = if let Some(idx) = stdout_final.find(sentinel) {
            // Found sentinel - split and extract
            let before_sentinel = &stdout_final[..idx];
            let after_sentinel = &stdout_final[idx + sentinel.len()..];

            // Extract CWD from everything after sentinel (trim newlines/whitespace)
            let extracted_cwd = after_sentinel
                .lines()
                .next()
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| cwd.clone());

            // Update state with new CWD
            state.set_cwd(extracted_cwd.clone()).await;

            // Clean output: remove trailing newline from before_sentinel
            let clean_output = before_sentinel.trim_end().to_string();
            (clean_output, extracted_cwd)
        } else {
            // Sentinel not found - command likely failed or was killed, return raw output and keep current CWD
            (stdout_final.clone(), cwd.clone())
        };

        if timed_out {
            info!("Command timed out: {}ms, cwd={}", took_ms, new_cwd);
        } else {
            info!("Command completed: exit={}, {}ms, cwd={}", exit_code, took_ms, new_cwd);
        }

        Ok(NuExecResult {
            exit_code,
            stdout,
            stderr: stderr_final,
            took_ms,
            success: !timed_out && exit_code == 0,
        })
    }

    /// Execute command (background mode) with stateful CWD
    pub async fn exec_background(
        &self,
        state: &AppState,
        command: &str,
        env: &HashMap<String, String>,
    ) -> anyhow::Result<NuBgResult> {
        let cwd = state.get_cwd().await;
        debug!("Executing background in {}: {}", cwd, command);

        // Robust CWD wrapper for background mode
        let full_command = format!("try {{ cd '{}' }}; {}", cwd, command);

        let mut cmd = Command::new(&self.nu_path);
        cmd.arg("-c").arg(&full_command);
        for (k, v) in env {
            cmd.env(k, v);
        }

        // Ensure pipes are set up for reading output later
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = cmd.spawn()?;
        let id = AppState::generate_id();

        // Register the process in global state
        state.register_process(id.clone(), child, command.to_string()).await;

        // Start background monitor task that drains pipes
        let state_clone = state.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            monitor_and_drain_pipes(state_clone, id_clone).await;
        });

        Ok(NuBgResult {
            id: id.clone(),
            status: "started".to_string(),
            message: format!("Background process started. ID: {}. Use nu.output to see output.", id),
        })
    }

    /// Read output from background process
    pub async fn read_output(
        &self,
        state: &AppState,
        id: &str,
        block: bool,
    ) -> anyhow::Result<NuOutputResult> {
        // If block=true, wait for process to complete
        if block {
            // Wait up to 5 minutes for process completion
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(300) {
                if let Some(snapshot) = state.get_process(id).await {
                    if snapshot.status != ProcessStatus::Running {
                        // Process completed
                        return Ok(NuOutputResult {
                            id: snapshot.id,
                            status: format!("{:?}", snapshot.status).to_lowercase(),
                            stdout: snapshot.stdout,
                            stderr: snapshot.stderr,
                            exit_code: snapshot.exit_code,
                            took_secs: snapshot.started_at_secs,
                        });
                    }
                } else {
                    return Err(anyhow::anyhow!("Process {} not found", id));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // Timeout - return current state
        }

        // Non-blocking: return current snapshot
        match state.get_process(id).await {
            Some(snapshot) => Ok(NuOutputResult {
                id: snapshot.id,
                status: format!("{:?}", snapshot.status).to_lowercase(),
                stdout: snapshot.stdout,
                stderr: snapshot.stderr,
                exit_code: snapshot.exit_code,
                took_secs: snapshot.started_at_secs,
            }),
            None => Err(anyhow::anyhow!("Process {} not found", id)),
        }
    }

    /// Kill background process
    pub async fn kill_process(
        &self,
        state: &AppState,
        id: &str,
    ) -> anyhow::Result<NuKillResult> {
        if let Some(info) = state.remove_process(id).await {
            // Kill the child process
            let child = info.child.lock().await.take();
            if let Some(mut child) = child {
                match child.kill().await {
                    Ok(_) => {
                        info!("Killed process {}", id);
                        Ok(NuKillResult {
                            id: id.to_string(),
                            status: "killed".to_string(),
                            command: info.command,
                        })
                    }
                    Err(e) => {
                        error!("Failed to kill process {}: {}", id, e);
                        Err(anyhow::anyhow!("Failed to kill: {}", e))
                    }
                }
            } else {
                // Child already gone
                Ok(NuKillResult {
                    id: id.to_string(),
                    status: "already_exited".to_string(),
                    command: info.command,
                })
            }
        } else {
            Err(anyhow::anyhow!("Process {} not found", id))
        }
    }

    /// Apply code edit using OpenAI-compatible API (provider-agnostic)
    pub async fn apply_file(
        &self,
        path: &str,
        instructions: &str,
        code_edit: &str,
    ) -> anyhow::Result<NuApplyResult> {
        let path_obj = Path::new(path);

        // Read current file content
        let initial_code = fs::read_to_string(&path_obj).await
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", path, e))?;

        // Get provider configuration from environment
        let api_url = std::env::var("APPLY_API_URL")
            .unwrap_or_else(|_| "https://api.morphllm.com/v1".to_string());
        let api_key = std::env::var("APPLY_API_KEY")
            .unwrap_or_else(|_| "ollama".to_string());
        let model = std::env::var("APPLY_MODEL")
            .unwrap_or_else(|_| "morph-v3-fast".to_string());

        // Construct the content for Fast Apply
        let content = format!("{}\n{}\n{}", instructions, initial_code, code_edit);

        // Call OpenAI-compatible API
        let url = format!("{}/chat/completions", api_url.trim_end_matches('/'));
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&json!({
                "model": model,
                "messages": [{
                    "role": "user",
                    "content": content
                }]
            }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("API request failed (URL: {}, Model: {}): {}", api_url, model, e))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("API error (URL: {}, Model: {}): {} - {}", api_url, model, status, error_text);
        }

        let api_response: serde_json::Value = response.json().await
            .map_err(|e| anyhow::anyhow!("Failed to parse API response: {}", e))?;

        let result = api_response["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid API response format: missing content"))?;

        // Write result back to file
        fs::write(&path_obj, result).await
            .map_err(|e| anyhow::anyhow!("Failed to write file {}: {}", path, e))?;

        Ok(NuApplyResult {
            path: path.to_string(),
            status: "applied".to_string(),
            message: format!("Code edit applied to {}", path),
        })
    }
}

/// Monitor background process and actively drain pipes into buffers
async fn monitor_and_drain_pipes(state: AppState, id: String) {
    // Get buffer references before taking the child
    let buffers = match state.get_buffers(&id).await {
        Some(b) => b,
        None => {
            error!("Process {} not found for monitoring", id);
            return;
        }
    };

    // Take the child out for monitoring (ProcessInfo stays in map)
    let mut child = match state.take_child(&id).await {
        Some(c) => c,
        None => {
            error!("Process {} child already taken or not found", id);
            return;
        }
    };

    // Take stdout and stderr handles
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Spawn stdout drain task
    let stdout_task = if let Some(stdout_pipe) = stdout {
        let buf = buffers.stdout.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stdout_pipe);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut buf = buf.lock().await;
                push_truncated(&mut buf, &format!("{}\n", line), 100_000);
                drop(buf);
            }
        }))
    } else {
        None
    };

    // Spawn stderr drain task
    let stderr_task = if let Some(stderr_pipe) = stderr {
        let buf = buffers.stderr.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stderr_pipe);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut buf = buf.lock().await;
                push_truncated(&mut buf, &format!("{}\n", line), 100_000);
                drop(buf);
            }
        }))
    } else {
        None
    };

    // Wait for process to complete
    let result = tokio::time::timeout(
        Duration::from_secs(300),
        child.wait(),
    ).await;

    let (exit_code, status) = match result {
        Ok(Ok(exit_status)) => {
            let code = exit_status.code().unwrap_or(-1);
            info!("Process {} exited with code {}", id, code);
            (code, if code == 0 { ProcessStatus::Completed } else { ProcessStatus::Failed })
        }
        Ok(Err(e)) => {
            error!("Process {} wait error: {:?}", id, e);
            (-1, ProcessStatus::Failed)
        }
        Err(_) => {
            error!("Process {} monitor timeout", id);
            (-1, ProcessStatus::Failed)
        }
    };

    // Wait for drain tasks to complete
    if let Some(task) = stdout_task {
        let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
    }
    if let Some(task) = stderr_task {
        let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
    }

    // Update final status (ProcessInfo is still in the map with these Arc'd fields)
    *buffers.exit_code.lock().await = Some(exit_code);
    *buffers.status.lock().await = status;

    debug!("Process {} monitoring complete, status={:?}", id, status);
}

/// Result structs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub took_ms: u128,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuBgResult {
    pub id: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuOutputResult {
    pub id: String,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub took_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuKillResult {
    pub id: String,
    pub status: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuApplyResult {
    pub path: String,
    pub status: String,
    pub message: String,
}

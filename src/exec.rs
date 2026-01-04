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
use tracing::{debug, error, info, warn};
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

/// NuSearch tool arguments
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NuSearchArgs {
    /// Search query string.
    pub query: String,
    /// Search category: general, cargo, packages, it, repos, skills, etc. (default: general).
    #[serde(default)]
    pub category: String,
    /// Maximum number of results to return (default: 10).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Specific engines to use (comma-separated, e.g., "npm,pypi").
    pub engines: Option<String>,
}

/// NuFetch tool arguments
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NuFetchArgs {
    /// URL to fetch.
    pub url: String,
    /// HTTP headers as key-value pairs (optional).
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    /// Request timeout in seconds (default: 30).
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// NuFetch result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuFetchResult {
    pub url: String,
    pub status: u16,
    pub content_type: String,
    pub content: String,
    pub format: String,
    pub error: Option<String>,
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
        let (clean_output, new_cwd) = if let Some(idx) = stdout_final.find(sentinel) {
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
            let clean_output_inner = before_sentinel.trim_end().to_string();
            (clean_output_inner, extracted_cwd)
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
            output: format!("{}{}", clean_output, if !stderr_final.is_empty() { format!("\n[stderr]\n{}", stderr_final) } else { String::new() }),
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

    /// Read output from background process (returns current snapshot immediately)
    pub async fn read_output(
        &self,
        state: &AppState,
        id: &str,
    ) -> anyhow::Result<NuOutputResult> {
        match state.get_process(id).await {
            Some(snapshot) => Ok(NuOutputResult {
                id: snapshot.id,
                status: format!("{:?}", snapshot.status).to_lowercase(),
                output: format!("{}{}", snapshot.stdout, if !snapshot.stderr.is_empty() { format!("\n[stderr]\n{}", snapshot.stderr) } else { String::new() }),
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
        let original_len = initial_code.len();

        // Get provider configuration from environment
        let api_url = std::env::var("APPLY_API_URL")
            .unwrap_or_else(|_| "https://api.morphllm.com/v1".to_string());
        let api_key = std::env::var("APPLY_API_KEY")
            .unwrap_or_else(|_| "ollama".to_string());
        let model = std::env::var("APPLY_MODEL")
            .unwrap_or_else(|_| "morph-v3-fast".to_string());

        // Warn if using non-Fast-Apply model
        if !model.contains("morph") && !model.contains("fast") {
            warn!("Using non-Fast-Apply model '{}' may cause corruption. Consider using 'morph-v3-fast'.", model);
        }

        // Construct the content for Fast Apply (canonical Morph SDK XML format)
        // Format: <instruction>{instructions}</instruction>\n<code>{original}</code>\n<update>{edit}</update>
        let content = format!("<instruction>{}</instruction>\n<code>{}</code>\n<update>{}</update>", instructions, initial_code, code_edit);

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

        // Sanitize the response to prevent corruption
        let sanitized = sanitize_response(result, original_len)
            .map_err(|e| anyhow::anyhow!("Response sanitization failed: {}", e))?;

        // Validate sanitized content is not empty
        if sanitized.trim().is_empty() {
            anyhow::bail!("Sanitized response is empty - refusing to overwrite file");
        }

        // Atomic backup system: create .bak file before writing
        let backup_path = format!("{}.bak", path);
        fs::copy(&path_obj, &backup_path).await
            .map_err(|e| anyhow::anyhow!("Failed to create backup at {}: {}", backup_path, e))?;

        // Write sanitized result back to file
        let write_result = fs::write(&path_obj, &sanitized).await;

        match write_result {
            Ok(_) => {
                // Success - remove the backup
                let _ = fs::remove_file(&backup_path).await;
                info!("Successfully applied edit to {} ({} -> {} chars)", path, original_len, sanitized.len());
                Ok(NuApplyResult {
                    path: path.to_string(),
                    status: "applied".to_string(),
                    message: format!("Code edit applied to {}", path),
                })
            }
            Err(e) => {
                // Write failed - report backup location
                Err(anyhow::anyhow!("Failed to write file {}: {}. Backup available at: {}", path, e, backup_path))
            }
        }
    }

    /// Search using SearXNG instance
    pub async fn search(&self, args: &NuSearchArgs) -> anyhow::Result<NuSearchResult> {
        let searx_url = std::env::var("SEARXNG_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8888".to_string());

        let limit = args.limit.unwrap_or(10);
        let category = if args.category.is_empty() { "general".to_string() } else { args.category.clone() };

        // Build URL with query parameters
        let mut url = format!(
            "{}/search?q={}&format=json",
            searx_url.trim_end_matches('/'),
            urlencoding::encode(&args.query)
        );

        if category != "general" {
            url = format!("{}&categories={}", url, category);
        }

        if let Some(ref engines) = args.engines {
            url = format!("{}&engines={}", url, engines);
        }

        debug!("Searching SearXNG: {}", url);

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("SearXNG request failed: {}", e))?;

        if !response.status().is_success() {
            anyhow::bail!("SearXNG returned error: {}", response.status());
        }

        let api_response: serde_json::Value = response.json().await
            .map_err(|e| anyhow::anyhow!("Failed to parse SearXNG response: {}", e))?;

        let results = api_response["results"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid SearXNG response: missing results"))?;

        let total = api_response["number_of_results"]
            .as_u64()
            .unwrap_or(0) as usize;

        // Take only the requested limit
        let limited_results: Vec<SearchResultItem> = results
            .iter()
            .take(limit)
            .filter_map(|r| {
                Some(SearchResultItem {
                    title: r["title"].as_str()?.to_string(),
                    url: r["url"].as_str()?.to_string(),
                    content: r["content"].as_str().unwrap_or("").to_string(),
                    engine: r["engine"].as_str().unwrap_or("unknown").to_string(),
                    category: r["category"].as_str().unwrap_or(&category).to_string(),
                })
            })
            .collect();

        Ok(NuSearchResult {
            query: args.query.clone(),
            results: limited_results.clone(),
            total,
            returned: limited_results.len(),
            answers: api_response["answers"].as_array().cloned().unwrap_or_default(),
            infoboxes: api_response["infoboxes"].as_array().cloned().unwrap_or_default(),
            suggestions: api_response["suggestions"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
        })
    }

    /// Fetch web content with browser-like headers and auto format conversion
    pub async fn fetch(&self, args: &NuFetchArgs) -> anyhow::Result<NuFetchResult> {
        let timeout_sec = args.timeout.unwrap_or(30);

        debug!("Fetching URL: {}", args.url);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_sec))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;

        let mut request = client.get(&args.url);

        // Add custom headers if provided
        if let Some(ref headers_map) = args.headers {
            for (key, value) in headers_map {
                request = request.header(key, value);
            }
        }

        // Add browser-like User-Agent if not custom provided
        if args.headers.is_none() || !args.headers.as_ref().unwrap().contains_key("User-Agent") {
            request = request.header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
            );
        }

        let response = request
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

        let body_str = String::from_utf8_lossy(&body_bytes).to_string();

        // Auto-detect and convert format
        let (content, final_format) = if content_type.contains("html") {
            (html2md::parse_html(&body_str), "markdown".to_string())
        } else {
            (body_str, "text".to_string())
        };

        Ok(NuFetchResult {
            url: args.url.clone(),
            status,
            content_type,
            content,
            format: final_format,
            error: if status >= 400 {
                Some(format!("HTTP {} error", status))
            } else {
                None
            },
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

    // Take the child out for monitoring (ProcessInfo stays in the map)
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
    pub output: String,
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
    pub output: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuSearchResult {
    pub query: String,
    pub results: Vec<SearchResultItem>,
    pub total: usize,
    pub returned: usize,
    pub answers: Vec<serde_json::Value>,
    pub infoboxes: Vec<serde_json::Value>,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    pub content: String,
    pub engine: String,
    pub category: String,
}

/// Extract code content from markdown-wrapped API responses
/// Handles formats like "```lua\ncode\n```" or "```\ncode\n```"
fn extract_code_block(response: &str) -> String {
    // Find all code blocks and extract their content
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current_block = String::new();

    for line in response.lines() {
        if line.trim().starts_with("```") {
            if in_block {
                // End of block - save it
                if !current_block.is_empty() {
                    blocks.push(current_block.clone());
                }
                current_block = String::new();
                in_block = false;
            } else {
                // Start of block
                in_block = true;
            }
        } else if in_block {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    // If we found code blocks, use the largest one
    if !blocks.is_empty() {
        blocks.into_iter()
            .max_by_key(|b| b.len())
            .unwrap_or_default()
    } else {
        // No code blocks found - return original
        response.to_string()
    }
}

/// Check if the response appears to be conversational text rather than code
/// This catches cases where the LLM explains instead of returning code
fn is_conversational_response(content: &str) -> bool {
    let content_lower = content.to_lowercase();
    let content_trimmed = content.trim();

    // Check for common conversational patterns
    let conversational_patterns = [
        "here is the",
        "i've updated",
        "i have updated",
        "the code has been",
        "here's the",
        "below is the",
        "the updated code",
        "sure, here",
        "here you go",
    ];

    // If it's very short and contains conversational markers
    if content_trimmed.len() < 500 {
        for pattern in &conversational_patterns {
            if content_lower.contains(pattern) {
                return true;
            }
        }
    }

    // Check if it's entirely conversational with no code-like content
    // (no braces, no function keywords, no markers)
    let has_code_indicators = content.contains("{")
        || content.contains("}")
        || content.contains("fn ")
        || content.contains("function")
        || content.contains("return")
        || content.contains("// ... existing code ...")
        || content.contains("-- ... existing code ...");

    // If we have conversational patterns but no code indicators, it's likely conversational
    if !has_code_indicators && content_trimmed.len() < 2000 {
        for pattern in &conversational_patterns {
            if content_lower.contains(pattern) {
                return true;
            }
        }
    }

    false
}

/// Sanitize API response by stripping markdown and validating content
fn sanitize_response(response: &str, original_len: usize) -> anyhow::Result<String> {
    let content = response.trim();

    // Check for empty response
    if content.is_empty() {
        anyhow::bail!("API returned empty response");
    }

    // If response contains markdown code blocks, extract content
    let sanitized = if content.contains("```") {
        extract_code_block(content)
    } else {
        content.to_string()
    };

    let sanitized = sanitized.trim();

    // Check for conversational response
    if is_conversational_response(sanitized) {
        anyhow::bail!("Model returned conversational response instead of code. Response: {}",
                      sanitized.chars().take(200).collect::<String>());
    }

    // Validate output length is reasonable (not severely truncated)
    // Allow up to 90% reduction for deletions, but not more
    if !sanitized.is_empty() && sanitized.len() < original_len / 10 {
        anyhow::bail!("Truncation Guard: The resulting file is too small ({} chars vs {} original). If this is a partial edit, you MUST include '// ... existing code ...' markers to indicate skipped sections. If you intended a full rewrite, ensure the content is complete.",
                      sanitized.len(), original_len);
    }

    // Check if response contains the marker (should be present in most edits)
    // Only skip this check for very small files where markers might not be needed
    if original_len > 500 && !sanitized.contains("... existing code ...") {
        // For larger files, the marker should typically be preserved
        // But we allow it in case the model legitimately removed it
        warn!("Response does not contain '... existing code ...' marker");
    }

    Ok(sanitized.to_string())
}
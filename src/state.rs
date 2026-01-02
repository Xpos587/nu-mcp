//! Global state for background process management

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tokio::process::Child;

/// Running process metadata with output buffering
#[derive(Debug)]
pub struct ProcessInfo {
    /// Child process (wrapped so we can take it while keeping ProcessInfo in map)
    pub child: Arc<TokioMutex<Option<Child>>>,
    pub started_at: std::time::Instant,
    pub command: String,
    /// Buffered stdout output
    pub stdout_buffer: Arc<TokioMutex<String>>,
    /// Buffered stderr output
    pub stderr_buffer: Arc<TokioMutex<String>>,
    /// Exit code (set when process completes)
    pub exit_code: Arc<TokioMutex<Option<i32>>>,
    /// Process status
    pub status: Arc<TokioMutex<ProcessStatus>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    Running,
    Completed,
    Failed,
}

/// Buffer references for monitor task
pub struct BufferRefs {
    pub stdout: Arc<TokioMutex<String>>,
    pub stderr: Arc<TokioMutex<String>>,
    pub status: Arc<TokioMutex<ProcessStatus>>,
    pub exit_code: Arc<TokioMutex<Option<i32>>>,
}

impl ProcessInfo {
    pub fn new(child: Child, command: String) -> Self {
        Self {
            child: Arc::new(TokioMutex::new(Some(child))),
            started_at: std::time::Instant::now(),
            command,
            stdout_buffer: Arc::new(TokioMutex::new(String::new())),
            stderr_buffer: Arc::new(TokioMutex::new(String::new())),
            exit_code: Arc::new(TokioMutex::new(None)),
            status: Arc::new(TokioMutex::new(ProcessStatus::Running)),
        }
    }

    /// Take the child out (for monitoring) - leaves None in its place
    pub async fn take_child(&self) -> Option<Child> {
        self.child.lock().await.take()
    }
}

/// Push data to buffer with truncation
pub fn push_truncated(buffer: &mut String, data: &str, max_size: usize) {
    if buffer.len() + data.len() > max_size {
        let remaining = max_size.saturating_sub(100);
        if data.len() > remaining {
            buffer.push_str(&data[..remaining]);
            buffer.push_str("\n... <truncated> ...");
        } else {
            buffer.truncate(remaining);
            buffer.push_str(data);
            buffer.push_str("\n... <truncated> ...");
        }
    } else {
        buffer.push_str(data);
    }
}

/// Global application state
#[derive(Clone)]
pub struct AppState {
    pub processes: Arc<TokioMutex<HashMap<String, ProcessInfo>>>,
    pub cwd: Arc<TokioMutex<String>>,
}

impl AppState {
    pub fn new() -> Self {
        let initial_cwd = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .to_string_lossy()
            .to_string();
        Self {
            processes: Arc::new(TokioMutex::new(HashMap::new())),
            cwd: Arc::new(TokioMutex::new(initial_cwd)),
        }
    }

    /// Get current working directory
    pub async fn get_cwd(&self) -> String {
        self.cwd.lock().await.clone()
    }

    /// Set working directory to a specific path
    pub async fn set_cwd(&self, path: String) {
        *self.cwd.lock().await = path;
    }

    /// Generate unique process ID
    pub fn generate_id() -> String {
        use nanoid::nanoid;
        format!("job_{}", nanoid!(6))
    }

    /// Register a new background process
    pub async fn register_process(&self, id: String, child: Child, command: String) {
        let info = ProcessInfo::new(child, command);
        self.processes.lock().await.insert(id, info);
    }

    /// Remove process from tracking
    pub async fn remove_process(&self, id: &str) -> Option<ProcessInfo> {
        self.processes.lock().await.remove(id)
    }

    /// Take the child out for monitoring while keeping ProcessInfo in map
    pub async fn take_child(&self, id: &str) -> Option<Child> {
        let processes = self.processes.lock().await;
        processes.get(id)?.take_child().await
    }

    /// Get process info without removing (for reading status/buffers)
    pub async fn get_process(&self, id: &str) -> Option<ProcessSnapshot> {
        let processes = self.processes.lock().await;
        let info = processes.get(id)?;

        // Clone Arcs first to avoid holding lock across await
        let command = info.command.clone();
        let status_buf = info.status.clone();
        let exit_code_buf = info.exit_code.clone();
        let stdout_buf = info.stdout_buffer.clone();
        let stderr_buf = info.stderr_buffer.clone();
        let started_at = info.started_at.elapsed().as_secs();
        drop(processes); // release lock

        // Compute all values first
        let status = *status_buf.lock().await;
        let exit_code = *exit_code_buf.lock().await;
        let stdout = stdout_buf.lock().await.clone();
        let stderr = stderr_buf.lock().await.clone();

        Some(ProcessSnapshot {
            id: id.to_string(),
            command,
            status,
            exit_code,
            stdout,
            stderr,
            started_at_secs: started_at,
        })
    }

    /// Get buffer references directly (for monitor task)
    pub async fn get_buffers(&self, id: &str) -> Option<BufferRefs> {
        let processes = self.processes.lock().await;
        let info = processes.get(id)?;

        Some(BufferRefs {
            stdout: info.stdout_buffer.clone(),
            stderr: info.stderr_buffer.clone(),
            status: info.status.clone(),
            exit_code: info.exit_code.clone(),
        })
    }
}

/// Snapshot of process state for reading
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessSnapshot {
    pub id: String,
    pub command: String,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub started_at_secs: u64,
}

//! Nushell MCP Server - Simplified Architecture
//! Three tools: NuExec, NuRead, NuKill
//! Principle: Consolidation - fewer, more general-purpose tools

use rmcp::{
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerInfo, CallToolResult, Content},
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServiceExt,
};
use std::collections::HashMap;
use tracing::{error, info};

mod exec;
mod state;

use exec::{NuApplyArgs, NuExecArgs, NuExecutor, NuKillArgs, NuOutputArgs};
use state::AppState;

#[derive(Clone)]
pub struct NuServer {
    tool_router: ToolRouter<Self>,
    state: AppState,
    executor: NuExecutor,
}

impl Default for NuServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl NuServer {
    pub fn new() -> Self {
        let nu_path = std::env::var("NU_PATH").unwrap_or_else(|_| "nu".to_string());

        Self {
            tool_router: Self::tool_router(),
            state: AppState::new(),
            executor: NuExecutor::new(nu_path, String::new()),
        }
    }

    /// NuExec - Execute Nushell commands (blocking or background)
    ///
    /// Use this tool for ALL shell command execution.
    ///
    /// Args:
    ///   command: Nushell pipeline to execute
    ///   background: If true, runs in background and returns job ID
    ///   cwd: Working directory (optional)
    ///   env: Environment variables (optional)
    ///   timeout: Timeout in seconds (optional, default 60)
    ///
    /// Returns:
    ///   blocking: {exit_code, stdout, stderr, took_ms, success}
    ///   background: {id, status, message}
    ///
    /// Examples:
    ///   "ls src"
    ///   "cargo build"
    ///   "open package.json | from json"
    #[tool(
        name = "nu.exec",
        description = r#"Executes a Nushell command or pipeline in a persistent session.

IMPORTANT: Use this for terminal operations (git, cargo, docker, etc.) and data inspection.

STRATEGY: Prefer native Nushell commands (ls, ps, open) piped to `to json` for structured output.

OUTPUT: Nushell commands return objects. To see output in stdout, pipe to `print`. Example: `ls | print` or `sys | to json | print`.

WARNING: Avoid searching in 'target/', '.git/', or '.cache/' directories as they cause timeouts and encoding errors. Always use `--exclude-dir` with `ug+`. Avoid piping giant search results directly to `from json` - use `| take 50` BEFORE `| to json`. If a command times out, immediately search in a specific subdirectory instead.

SAFETY: Always quote file paths with spaces. Nushell `mkdir` creates parents by default (no `-p` flag).

DO NOT use `cat`, `grep`, `sed`, or `awk` - use Nushell pipelines or `nu.apply` instead."#
    )]
    pub async fn nu_exec(&self, args: Parameters<NuExecArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;
        let env = args.env.as_ref().unwrap_or(&HashMap::new()).clone();

        // If cwd is explicitly provided, temporarily override the state CWD
        let state = if let Some(ref provided_cwd) = args.cwd {
            // Create a temporary state with the provided CWD
            let temp_state = self.state.clone();
            temp_state.set_cwd(provided_cwd.clone()).await;
            temp_state
        } else {
            self.state.clone()
        };

        let result = if args.background {
            let bg_result = self.executor
                .exec_background(&state, &args.command, &env)
                .await
                .map_err(|e| McpError::invalid_request(format!("exec_background failed: {e}"), None))?;
            serde_json::to_value(&bg_result).unwrap()
        } else {
            let timeout = self.executor.resolve_timeout(args.timeout);
            let exec_result = self.executor
                .exec_blocking(&state, &args.command, &env, timeout)
                .await
                .map_err(|e| McpError::invalid_request(format!("exec_blocking failed: {e}"), None))?;
            serde_json::to_value(&exec_result).unwrap()
        };

        Ok(CallToolResult::success(vec![Content::json(result)?]))
    }

    /// NuOutput - Read output from background process
    ///
    /// Use this to get output from processes started with background=true.
    ///
    /// Args:
    ///   id: Job ID from NuExec
    ///   block: If true, wait for process to complete (optional, default false)
    ///
    /// Returns:
    ///   {id, status, stdout?, stderr?, exit_code?, took_secs?}
    #[tool(
        name = "nu.output",
        description = r#"Retrieves stdout/stderr from a running or completed background process started via `nu.exec`.

BEHAVIOR: Returns current buffer snapshots. Use `block: true` to wait for process completion (max 5 min)."#
    )]
    pub async fn nu_output(&self, args: Parameters<NuOutputArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;
        let block = args.block.unwrap_or(false);

        let result = self.executor
            .read_output(&self.state, &args.id, block)
            .await
            .map_err(|e| McpError::invalid_request(format!("read_output failed: {e}"), None))?;

        let json = serde_json::to_value(&result).unwrap();
        Ok(CallToolResult::success(vec![Content::json(json)?]))
    }

    /// NuKill - Kill a background process
    ///
    /// Use this to terminate a running background process.
    ///
    /// Args:
    ///   id: Job ID to kill
    ///
    /// Returns:
    ///   {id, status, command}
    #[tool(
        name = "nu.kill",
        description = r#"Terminate a running background process by its job ID to release system resources."#
    )]
    pub async fn nu_kill(&self, args: Parameters<NuKillArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;

        let result = self.executor
            .kill_process(&self.state, &args.id)
            .await
            .map_err(|e| McpError::invalid_request(format!("kill_process failed: {e}"), None))?;

        let json = serde_json::to_value(&result).unwrap();
        Ok(CallToolResult::success(vec![Content::json(json)?]))
    }

    /// NuApply - Apply code edits via OpenAI-compatible API
    ///
    /// Use this tool to edit files using partial code snippets and '// ... existing code ...' markers.
    /// It is much faster and more reliable than standard Edit.
    ///
    /// Supports any OpenAI-compatible provider: MorphLLM (default), Ollama, vLLM, DeepSeek, etc.
    /// Configure via environment variables: APPLY_API_URL, APPLY_API_KEY, APPLY_MODEL.
    ///
    /// NOTE: Requires APPLY_API_KEY (or 'ollama' for local) and APPLY_API_URL to be configured.
    ///
    /// Args:
    ///   path: Absolute path to file to edit
    ///   instructions: What to change
    ///   code_edit: Code with `// ... existing code ...` markers
    ///
    /// Returns:
    ///   {path, status, message}
    ///
    /// Example:
    ///   instructions: "Add a new function"
    ///   code_edit: "// ... existing code ...\n\nfn new_function() { }\n\n// ... existing code ..."
    #[tool(
        name = "nu.apply",
        description = r#"Surgically edit existing files using the 'Fast Apply' pattern.

NOTE: Requires external API configuration (APPLY_API_KEY, APPLY_API_URL). Use 'ollama' for local.

RULES:
1. Use `// ... existing code ...` markers to represent unchanged blocks.
2. Include minimal context around edits for disambiguation.
3. Preserve exact indentation.
4. For deletions, show context before/after and omit deleted lines.
5. Batch multiple edits to the same file in one call.
6. NEVER rewrite the whole file unless it's very small."#
    )]
    pub async fn nu_apply(&self, args: Parameters<NuApplyArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;

        let result = self.executor
            .apply_file(&args.path, &args.instructions, &args.code_edit)
            .await
            .map_err(|e| McpError::invalid_request(format!("apply_file failed: {e}"), None))?;

        let json = serde_json::to_value(&result).unwrap();
        Ok(CallToolResult::success(vec![Content::json(json)?]))
    }
}

#[tool_handler]
impl rmcp::ServerHandler for NuServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: rmcp::model::ProtocolVersion::V_2024_11_05,
            server_info: rmcp::model::Implementation {
                name: "nu-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                title: None,
                icons: None,
                website_url: None,
            },
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: Some(
                "Nushell execution server with 4 tools: nu.exec (run commands), nu.output (read bg process output), nu.kill (kill bg process), nu.apply (fast code edits).".to_string(),
            ),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::level_filters::LevelFilter::INFO.into()),
        )
        .without_time()
        .init();

    let service = NuServer::new()
        .serve(stdio())
        .await
        .inspect_err(|e| error!("Error starting server: {}", e))?;

    info!("nu-mcp server started");
    service.waiting().await?;
    Ok(())
}

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

use exec::{NuApplyArgs, NuExecArgs, NuExecutor, NuFetchArgs, NuKillArgs, NuOutputArgs, NuSearchArgs};
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
    ///   blocking: {exit_code, output, took_ms, success}
    ///   background: {id, status, message}
    ///
    /// Examples:
    ///   "ls src"
    ///   "cargo build"
    ///   "open package.json | from json"
    #[tool(
        name = "nu.exec",
        description = r#"Execute Nushell commands with structured data pipelines. Nushell treats data as tables/records, not text.

BASIC SYNTAX:
- Variables: `$name`, `$env.PATH`
- String interpolation: $"hello ($name)"
- Ranges: `1..10`, `1..2..10` (step by 2)
- Lists: `[1 2 3]`, records: `{a: 1, b: 2}`

FILE OPERATIONS (native):
- List: `ls`, `ls *.txt`, `ls | where type == file`
- Read: `open file.txt`, `open data.json`, `open config.toml`
- Write: `"content" | save file.txt`, `data | to json | save out.json`
- Move/copy/remove: `mv old.txt new.txt`, `cp src dst`, `rm file.txt`
- Mkdir: `mkdir dir` (creates parents by default, no -p flag)

PIPELINE OPERATIONS:
- Filter: `ls | where size > 1mb`, `ls | where name =~ "test"`
- Select: `ls | select name size`, `ls | get name`
- Sort: `ls | sort-by size | reverse`
- Transform: `ls | update size { $it.size / 1000 }`
- Aggregate: `ls | length`, `[1 2 3] | math sum`

STRUCTURED DATA:
- JSON: `open data.json | get users | where age > 25`
- CSV: `open data.csv | where active == true | to json`
- Convert: `open data.txt | from json | to csv | save out.csv`
- HTTP: `http get https://api.example.com/users`

STRINGS:
- Case: `"hello" | str upcase`, `"HELLO" | str downcase`
- Trim: `"  text  " | str trim`
- Replace: `"hello world" | str replace "world" "nu"`
- Contains: `"hello" | str contains "ell"` (returns boolean)
- Split: `"a,b,c" | split row ","`

CONDITIONALS:
- if/else: `if $age > 18 { "adult" } else { "minor" }`
- match: `match $val { 1 => "one", 2 => "two", _ => "other" }`

LOOPS:
- For: `for x in 1..5 { print $x }`
- While: `mut i = 0; while $i < 5 { print $i; $i += 1 }`
- Each: `[1 2 3] | each { |x| $x * 2 }`

EXTERNAL COMMANDS:
- Prefix with `^`: `^git status`, `^cargo build`
- Capture output: `let out = (^git status | complete)

AVOID BASHISMS - use Nushell native:
- Instead of `cat`: use `open`
- Instead of `grep`: use `where` with string operations
- Instead of `sed/awk`: use `str replace`, `update`, `select`
- Instead of `find`: use `ls` with filters
- Instead of `&&`: use `;` for chaining
- Instead of `|`: use `|` (same, but data is structured)
- Instead of `$VAR`: use `$env.VAR` or `$var`
- Instead of `$(cmd)`: use `(cmd)` or `^cmd`
- Instead of `sort | uniq`: use `lines | uniq` for text, or just `uniq` for lists

OUTPUT FORMATTING:
- To see stdout: pipe to `print` → `ls | print`
- To get JSON: pipe to `to json` → `ls | to json | print`
- To CSV: pipe to `to csv` → `ls | to csv`
- Truncate large output: `ls | take 50 | to json`

WARNING:
- Avoid searching in 'target/', '.git/', '.cache/', 'node_modules/', '.venv/' (timeouts/encoding errors)
- Use `| take N BEFORE | to json` for large results
- If command times out, search in specific subdirectory instead
- Quote file paths with spaces: `"my path/file.txt""#
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

            format!("Background process started.\nID: {}\nStatus: {}\n{}", bg_result.id, bg_result.status, bg_result.message)
        } else {
            let timeout = self.executor.resolve_timeout(args.timeout);
            let exec_result = self.executor
                .exec_blocking(&state, &args.command, &env, timeout)
                .await
                .map_err(|e| McpError::invalid_request(format!("exec_blocking failed: {e}"), None))?;

            format!("Exit code: {}\nTime: {}ms\n\n{}",
                exec_result.exit_code,
                exec_result.took_ms,
                exec_result.output
            )
        };

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    /// NuOutput - Read output from background process
    ///
    /// Use this to get output from processes started with background=true.
    ///
    /// Args:
    ///   id: Job ID from NuExec
    ///
    /// Returns:
    ///   {id, status, output, exit_code?, took_secs?}
    #[tool(
        name = "nu.output",
        description = r#"Retrieves output from a running or completed background process started via `nu.exec`.

Returns current buffer snapshot immediately. Output includes stdout with stderr appended (marked with [stderr] if present)."#
    )]
    pub async fn nu_output(&self, args: Parameters<NuOutputArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;

        let result = self.executor
            .read_output(&self.state, &args.id)
            .await
            .map_err(|e| McpError::invalid_request(format!("read_output failed: {e}"), None))?;

        let text = format!("ID: {}\nStatus: {}\nRunning for: {}s\nExit code: {}\n\n{}",
            result.id,
            result.status,
            result.took_secs,
            result.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "running".to_string()),
            result.output
        );

        Ok(CallToolResult::success(vec![Content::text(text)]))
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

        let text = format!("ID: {}\nStatus: {}\nCommand: {}", result.id, result.status, result.command);
        Ok(CallToolResult::success(vec![Content::text(text)]))
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
        description = r#"Use this tool to edit existing files by showing only the changed lines.

Use "// ... existing code ..." to represent unchanged code blocks. Include just enough surrounding context to locate each edit precisely.

Example format:
// ... existing code ...
FIRST_EDIT
// ... existing code ...
SECOND_EDIT
// ... existing code ...

Rules:
- ALWAYS use "// ... existing code ..." for unchanged sections (omitting this marker will cause deletions)
- Include minimal context around edits for disambiguation
- Preserve exact indentation
- For deletions: show context before and after, omit the deleted lines
- Batch multiple edits to the same file in one call"#
    )]
    pub async fn nu_apply(&self, args: Parameters<NuApplyArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;

        let result = self.executor
            .apply_file(&args.path, &args.instructions, &args.code_edit)
            .await
            .map_err(|e| McpError::invalid_request(format!("apply_file failed: {e}"), None))?;

        let text = format!("Path: {}\nStatus: {}\n{}", result.path, result.status, result.message);
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// NuSearch - Search using SearXNG instance
    ///
    /// Use this tool to search the web, package repositories, and code repositories.
    ///
    /// NOTE: Requires SearXNG instance running (default: http://127.0.0.1:8888).
    /// Configure via SEARXNG_URL environment variable.
    ///
    /// Args:
    ///   query: Search query
    ///   category: Search category (general, cargo, packages, it, repos, skills, etc.)
    ///   limit: Max results to return (default: 10)
    ///   engines: Specific engines to use (e.g., "npm,pypi")
    ///
    /// Returns:
    ///   {query, results: [{title, url, content, engine, category}], total, returned, answers, infoboxes, suggestions}
    ///
    /// Examples:
    ///   query: "tokio" category: "cargo" -> Search Rust crates
    ///   query: "express" engines: "npm" -> Search npm packages only
    ///   query: "requests" engines: "pypi" -> Search PyPI packages
    ///   query: "machine learning" category: "repos" -> Search code repositories
    ///   query: "rust async" category: "it" -> Search IT/tech resources
    #[tool(
        name = "nu.search",
        description = r#"Search web, package repositories, and code using SearXNG metasearch engine. Returns structured JSON with results from multiple engines.

CATEGORIES (use for broad search scope):
- general: Web search (default)
- cargo: Rust crates from crates.io
- packages: Multi-repo package search (npm, PyPI, rubygems, hackage, hex, packagist, metacpan, pub.dev, go, docker, alpine)
- it: IT/tech resources (GitHub, Docker Hub, crates.io, Stack Overflow, Wikitech)
- repos: Code repositories (GitHub, GitLab, Gitea, Codeberg, Bitbucket)
- code: Code search
- skills: Claude Code Agent Skills
- science: Scientific publications
- news: News articles
- videos, images, music, books, files: Media-specific search

ENGINES (use for specific sources, comma-separated):
- Web: duckduckgo, google, bing, startpage, brave
- Packages: npm, pypi, crates.io, rubygems, hackage, hex, packagist, metacpan, pub.dev, go, docker, alpine
- Repos: github, gitlab, gitea, codeberg, bitbucket
- IT: stackoverflow, wikitech, github, docker hub

WHEN TO USE category vs engines:
- Use category for broad search across multiple engines in a domain
- Use engines when you need results from specific sources only
- Example: category="packages" searches ALL package repos; engines="npm,pypi" searches only npm and PyPI

USAGE EXAMPLES:
1. Search Rust crates: query="tokio" category="cargo"
2. Search npm only: query="express" engines="npm"
3. Search PyPI only: query="requests" engines="pypi"
4. GitHub repos: query="machine learning" category="repos"
5. IT/tech: query="rust async" category="it"
6. General web: query="latest rust news" category="general"
7. Multi-package: query="http client" category="packages"
8. Multiple engines: query="web framework" engines="npm,crate,composer"

RESPONSE STRUCTURE:
- query: The search query
- results: Array of {title, url, content, engine, category, score}
- total: Total results available
- returned: Number of results returned
- answers: Direct answers/infoboxes from SearXNG (e.g., calculators, conversions)
- infoboxes: Knowledge panels with structured information
- suggestions: Search query suggestions

ANSWERS/INFOBOXES:
- SearXNG returns direct answers for factual queries
- Examples: "capital of france", "2+2", "python version"
- Check answers and infoboxes fields for instant results

KNOWN ISSUES:
- Cargo category sometimes returns empty: Try category="packages" or category="it" (also includes crates.io)
- PyPI search takes 1-2 seconds: Loading package index from Simple API
- Rate limiting: SearXNG may rate-limit if too many requests in quick succession
- Some engines may be unresponsive: Check unresponsive_engines in response

ARGS:
- query: Search query string (required)
- category: Search category (default: general)
- limit: Max results to return (default: 10)
- engines: Specific engines to use (comma-separated, e.g., "npm,pypi")"#
    )]
    pub async fn nu_search(&self, args: Parameters<NuSearchArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;

        let result = self.executor
            .search(args)
            .await
            .map_err(|e| McpError::invalid_request(format!("search failed: {e}"), None))?;

        // Format as plain text for better readability
        let mut text = format!("Query: \"{}\" | Category: {} | Found: {} results | Showing: {}\n\n",
            result.query,
            args.category,
            result.total,
            result.returned
        );

        // Add results
        for (i, item) in result.results.iter().enumerate() {
            text.push_str(&format!("[{}] {}\n", i + 1, item.title));
            text.push_str(&format!("    URL: {}\n", item.url));
            text.push_str(&format!("    Engine: {}\n", item.engine));
            if !item.content.is_empty() {
                text.push_str(&format!("    Content: {}\n", item.content));
            }
            text.push('\n');
        }

        // Add answers if any
        if !result.answers.is_empty() {
            text.push_str("** Direct Answers:\n");
            for answer in &result.answers {
                text.push_str(&format!("    {}\n", answer));
            }
            text.push('\n');
        }

        // Add infoboxes if any
        if !result.infoboxes.is_empty() {
            text.push_str("** Infoboxes:\n");
            for info in &result.infoboxes {
                text.push_str(&format!("    {}\n", info));
            }
            text.push('\n');
        }

        // Add suggestions if any
        if !result.suggestions.is_empty() {
            text.push_str("** Suggestions:\n");
            for sug in &result.suggestions {
                text.push_str(&format!("    - {}\n", sug));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// NuFetch - Fetch web content with format conversion
    ///
    /// Use this to fetch and convert web content (HTML to Markdown, JSON as-is, etc.).
    ///
    /// Args:
    ///   url: URL to fetch
    ///   format: Response format (auto/json/markdown/text, default: auto)
    ///   headers: Optional HTTP headers as key-value pairs
    ///   timeout: Request timeout in seconds (default: 30)
    ///
    /// Returns:
    ///   {url, status, content_type, content, format, error?}
    ///
    /// Examples:
    ///   url: "https://example.com" format: "markdown" -> Fetch HTML and convert to Markdown
    ///   url: "https://api.example.com/data.json" format: "json" -> Fetch JSON API
    ///   url: "https://httpbin.org/headers" headers: {"Accept": "application/json"}
    #[tool(
        name = "nu.fetch",
        description = r#"Fetch web content with browser-like headers and automatic format conversion.

FORMAT CONVERSION:
- HTML → Markdown (automatic)
- JSON/Text → As-is

BROWSER FINGERPRINTING:
- Automatically adds Chrome-like User-Agent header
- Mimics real browser to avoid bot detection

USAGE EXAMPLES:
1. Fetch webpage: url="https://example.com"
2. Fetch API: url="https://api.github.com/users/octocat"
3. Custom headers: url="https://httpbin.org/headers" headers={"Authorization": "Bearer token"}

RESPONSE STRUCTURE:
- url: The fetched URL
- status: HTTP status code (200, 404, etc.)
- content_type: Response content-type header
- content: Response content (HTML converted to Markdown)
- format: Actual format returned (markdown/text)
- error: Error message if status >= 400, null otherwise

NOTES:
- HTML to Markdown conversion uses html2md library
- Timeout prevents hanging (default: 30 seconds)
- Custom User-Agent can be provided via headers"#
    )]
    pub async fn nu_fetch(&self, args: Parameters<NuFetchArgs>) -> Result<CallToolResult, McpError> {
        let args = &args.0;

        let result = self.executor
            .fetch(args)
            .await
            .map_err(|e| McpError::invalid_request(format!("fetch failed: {e}"), None))?;

        let mut text = format!("URL: {}\nStatus: {}\nContent-Type: {}\nFormat: {}\n\n{}",
            result.url,
            result.status,
            result.content_type,
            result.format,
            result.content
        );

        if let Some(err) = result.error {
            text.push_str(&format!("\nError: {}", err));
        }

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

#[tool_handler]
impl rmcp::ServerHandler for NuServer {
    fn get_info(&self) -> ServerInfo {
        let instructions = "Nushell execution server with 6 tools: nu.exec (run commands), nu.output (read bg process output), nu.kill (kill bg process), nu.apply (fast code edits), nu.search (web/packages search), nu.fetch (fetch web content).";

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
            instructions: Some(instructions.to_string()),
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

    let server = NuServer::new();

    let service = server
        .serve(stdio())
        .await
        .inspect_err(|e| error!("Error starting server: {}", e))?;

    info!("nu-mcp server started");
    service.waiting().await?;
    Ok(())
}
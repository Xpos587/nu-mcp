# nu-mcp

A minimal MCP (Model Context Protocol) server that exposes Nushell shell execution and code editing capabilities to AI coding agents. Built with Rust using the `rmcp` SDK.

**Philosophy:** Consolidation — fewer, more general-purpose tools. Instead of 34+ specialized tools, `nu-mcp` provides just 4 powerful tools that leverage Nushell's rich built-in commands.

## Features

- **Stateful Shell Sessions**: Persistent working directory across commands
- **Background Processes**: Run long-running tasks (servers, watchers) asynchronously
- **Active Pipe Draining**: Prevents subprocess hangs by draining stdout/stderr concurrently
- **Kill-on-Timeout**: Automatically terminates hung processes after timeout
- **Fast Code Edits**: Surgical file editing via external LLM API (OpenAI-compatible)

## Requirements

- **Rust** 1.70+ (for building)
- **Nushell** (`nu`) — the shell that powers all command execution

```bash
# macOS
brew install nushell

# Linux
cargo install nu

# Verify
nu --version
```

## Installation

```bash
# Clone the repository
git clone https://github.com/graves/awful_mcp
cd awful_mcp/nu-mcp

# Build release binary
cargo build --release

# Binary location: target/release/nu-mcp
```

## Setup

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or `~/.config/Claude/claude_desktop_config.json` (Linux):

```json
{
  "mcpServers": {
    "nu-mcp": {
      "command": "/absolute/path/to/nu-mcp/target/release/nu-mcp",
      "env": {
        "APPLY_API_KEY": "ollama",
        "APPLY_API_URL": "http://localhost:11434/v1",
        "APPLY_MODEL": "llama3.1"
      }
    }
  }
}
```

### Environment Variables

| Variable        | Default                       | Description                      |
| --------------- | ----------------------------- | -------------------------------- |
| `NU_PATH`       | `nu`                          | Path to Nushell binary           |
| `APPLY_API_URL` | `https://api.morphllm.com/v1` | LLM API endpoint for code edits  |
| `APPLY_API_KEY` | `ollama`                      | API key (use `ollama` for local) |
| `APPLY_MODEL`   | `morph-v3-fast`               | Model for code edits             |
| `RUST_LOG`      | `info`                        | Logging level                    |

## Tools

### `nu.exec`

Execute Nushell commands in a persistent session.

**Arguments:**
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `command` | string | Yes | Nushell pipeline to execute |
| `background` | boolean | No | Run in background (default: false) |
| `cwd` | string | No | Working directory override |
| `env` | object | No | Environment variables |
| `timeout` | number | No | Timeout in seconds (default: 60) |

**Blocking Example:**

```json
{
  "command": "ls src | where size > 1kb | to json"
}
```

**Background Example:**

```json
{
  "command": "cargo watch",
  "background": true
}
```

### `nu.output`

Retrieve output from a background process.

**Arguments:**
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Job ID from `nu.exec` |
| `block` | boolean | No | Wait for completion (default: false) |

### `nu.kill`

Terminate a background process.

**Arguments:**
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Job ID to kill |

### `nu.apply`

Surgically edit files using the "Fast Apply" pattern with `// ... existing code ...` markers.

**Arguments:**
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string | Yes | Absolute file path |
| `instructions` | string | Yes | Description of changes |
| `code_edit` | string | Yes | Code with markers |

**Example:**

```
instructions: "Add error handling"
code_edit: |
  // ... existing code ...

  fn main() -> Result<()> {
      do_work()?;
      Ok(())
  }

  // ... existing code ...
```

## Nushell Cheat Sheet

Since `nu-mcp` exposes Nushell directly, here are common patterns:

| Task             | Command                       |
| ---------------- | ----------------------------- |
| List files       | `ls`                          |
| List as JSON     | `ls \| to json`               |
| Search files     | `ug+ 'pattern' path/`         |
| Read file        | `open file.txt`               |
| Read JSON        | `open file.json \| from json` |
| Get system info  | `sys`                         |
| List processes   | `ps`                          |
| Change directory | `cd path/`                    |
| Create directory | `mkdir path/`                 |
| Run commands     | `cargo build`                 |

## Architecture

```
src/
├── main.rs    — MCP server setup, tool handlers
├── exec.rs    — Command execution, background processes
└── state.rs   — Global state, CWD tracking, process registry
```

**Key Design Decisions:**

1. **Consolidation**: 4 tools instead of 30+. Nushell has 100+ built-in commands.
2. **Stateful CWD**: Directory persists across commands (like a real shell).
3. **Active Draining**: Prevents pipe buffer deadlocks.
4. **Kill-on-Timeout**: No zombie processes.

## Development

```bash
# Build
cargo build --release

# Run with logging
RUST_LOG=debug cargo run

# Format
cargo fmt

# Lint
cargo clippy

# Test
cargo test
```

## License

MIT

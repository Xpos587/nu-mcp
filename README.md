# nu-mcp

A lightweight MCP server that gives AI coding agents access to a Nushell shell. Instead of bundling dozens of specialized tools, it exposes Nushell's rich command set directly through four simple tools.

## What it does

- **Stateful sessions**: Your working directory sticks around between commands
- **Background tasks**: Run watchers, servers, or build scripts without blocking
- **Smart pipe handling**: Actively drains stdout/stderr so subprocesses don't hang
- **Timeout protection**: Kills stuck processes automatically

## Requirements

Just two things:

- **Rust 1.70+** to build
- **Nushell** (`nu`) to run commands

Install Nushell:

```bash
# macOS
brew install nushell

# Linux
cargo install nu
```

## Installation

```bash
git clone https://github.com/Xpos587/nu-mcp
cd nu-mcp
cargo build --release
```

The binary ends up at `target/release/nu-mcp`.

## Setup

Add this to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `~/.config/Claude/claude_desktop_config.json` on Linux):

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

### Environment options

| Variable        | Default                       | What it does                          |
| --------------- | ----------------------------- | ------------------------------------- |
| `NU_PATH`       | `nu`                          | Path to the Nushell binary            |
| `APPLY_API_URL` | `https://api.morphllm.com/v1` | LLM endpoint for code editing         |
| `APPLY_API_KEY` | `ollama`                      | API key (use `ollama` for local LLMs) |
| `APPLY_MODEL`   | `morph-v3-fast`               | Which model to use for edits          |
| `RUST_LOG`      | `info`                        | How chatty the logs are               |

## Tools

### `nu.exec`

Run a Nushell command. Everything goes through here.

| Field        | Type    | Required | Notes                               |
| ------------ | ------- | -------- | ----------------------------------- |
| `command`    | string  | Yes      | The Nushell pipeline to run         |
| `background` | boolean | No       | Run asynchronously (default: false) |
| `cwd`        | string  | No       | Override working directory          |
| `env`        | object  | No       | Extra environment variables         |
| `timeout`    | number  | No       | Timeout in seconds (default: 60)    |

**Quick example:**

```json
{
  "command": "ls src | where size > 1kb | to json"
}
```

**Background task:**

```json
{
  "command": "cargo watch",
  "background": true
}
```

### `nu.output`

Grab output from a background process.

| Field   | Type    | Required | Notes                                        |
| ------- | ------- | -------- | -------------------------------------------- |
| `id`    | string  | Yes      | Job ID from `nu.exec`                        |
| `block` | boolean | No       | Wait for it to finish first (default: false) |

### `nu.kill`

Stop a background process.

| Field | Type   | Required | Notes               |
| ----- | ------ | -------- | ------------------- |
| `id`  | string | Yes      | Job ID to terminate |

### `nu.apply`

Edit files surgically using `// ... existing code ...` markers. Faster and more reliable than traditional file replacement.

| Field          | Type   | Required | Notes                     |
| -------------- | ------ | -------- | ------------------------- |
| `path`         | string | Yes      | Absolute path to the file |
| `instructions` | string | Yes      | What you're changing      |
| `code_edit`    | string | Yes      | The code with markers     |

**How to use it:**

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

## Nushell basics

Since this server just passes commands through to Nushell, you can use any Nushell built-in:

| What you want | How to do it                    |
| ------------- | ------------------------------- |
| List files    | `ls`                            |
| As JSON       | `ls \| to json`                 |
| Search files  | `ug+ 'pattern' path/`           |
| Read a file   | `open file.txt`                 |
| Parse JSON    | `open file.json \| from json`   |
| System info   | `sys`                           |
| Processes     | `ps`                            |
| Change dir    | `cd path/`                      |
| Make dir      | `mkdir path/`                   |
| Run stuff     | `cargo build`, `npm test`, etc. |

## How it's built

```
src/
├── main.rs    — MCP server, tool handlers
├── exec.rs    — Command execution, background jobs
└── state.rs   — CWD tracking, process registry
```

**Design choices:**

1. **Fewer tools** — Four tools instead of dozens. Nushell already has the commands you need.
2. **Persistent CWD** — The working directory stays where you left it, like a real shell.
3. **Active draining** — Reads stdout/stderr concurrently so pipes don't block.
4. **Kill-on-timeout** — No zombie processes left behind.

## Development

```bash
cargo build --release
RUST_LOG=debug cargo run
cargo fmt
cargo clippy
cargo test
```

## License

MIT

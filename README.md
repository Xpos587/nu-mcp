# nu-mcp

[![CICD](https://github.com/Xpos587/nu-mcp/actions/workflows/cicd.yml/badge.svg)](https://github.com/Xpos587/nu-mcp/actions/workflows/cicd.yml)
[![Version info](https://img.shields.io/crates/v/nu-mcp.svg)](https://crates.io/crates/nu-mcp)
[![License](https://img.shields.io/crates/l/nu-mcp.svg)](LICENSE)

Six tools. One shell. AI agents get Nushell—without Bash baggage.

---

## Why nu-mcp?

Most MCP servers ship 20+ tools. One for listing files, another for searching, three for JSON parsing. The agent spends half its time figuring out which tool does what.

Nushell already has 400+ commands that handle pipelines, structured data, and system operations. `nu-mcp` exposes those through six tools:

| Tool        | Purpose                                    |
| ----------- | ------------------------------------------ |
| `nu.exec`   | Run Nushell commands (blocking or bg)      |
| `nu.output` | Get output from background processes       |
| `nu.kill`   | Stop background tasks                      |
| `nu.apply`  | Edit files with Fast Apply                 |
| `nu.search` | Search web, packages, repos (SearXNG)      |
| `nu.fetch`  | Fetch web content (HTML → Markdown)        |

**Result:** Fewer tools to maintain, better performance, and agents that understand what they're doing.

---

## Quick Start

### Install

```bash
cargo install nu-mcp
```

Or build from source:

```bash
git clone https://github.com/Xpos587/nu-mcp
cd nu-mcp
cargo build --release
```

Binary ends up at `target/release/nu-mcp`.

### Configure

**Claude Code:**

```bash
claude mcp add nu-mcp \
  --transport stdio \
  --scope user \
  --env SEARXNG_URL=http://127.0.0.1:8888 \
  --env APPLY_API_KEY=ollama \
  --env APPLY_API_URL=http://localhost:11434/v1 \
  --env APPLY_MODEL=morph-v3-fast \
  -- /path/to/nu-mcp
```

**Codex:**

```bash
codex mcp add nu-mcp \
  --env SEARXNG_URL=http://127.0.0.1:8888 \
  --env APPLY_API_KEY=ollama \
  --env APPLY_API_URL=http://localhost:11434/v1 \
  --env APPLY_MODEL=morph-v3-fast \
  -- /path/to/nu-mcp
```

### Environment Variables

| Variable        | Default                       | Purpose                           |
| --------------- | ----------------------------- | --------------------------------- |
| `NU_PATH`       | `nu`                          | Path to Nushell                   |
| `SEARXNG_URL`   | `http://127.0.0.1:8888`       | SearXNG instance for web search   |
| `APPLY_API_URL` | `https://api.morphllm.com/v1` | LLM endpoint for code editing     |
| `APPLY_API_KEY` | `ollama`                      | API key (`ollama` for local)      |
| `APPLY_MODEL`   | `morph-v3-fast`               | Model for Fast Apply edits        |
| `RUST_LOG`      | `info`                        | Log verbosity                     |

---

## Tools

### nu.exec

Run Nushell commands.

**Blocking:**

```
command: "ls src | where size > 1kb"
```

Returns:
```
Exit code: 0
Time: 23ms

file1.rs
file2.rs
```

**Background:**

```
command: "cargo watch"
background: true
```

Returns job ID. Use `nu.output` to get results.

**Options:**

| Field        | Type    | Notes                                  |
| ------------ | ------- | -------------------------------------- |
| `command`    | string  | Nushell pipeline to run                |
| `background` | boolean | Run async (default: `false`)           |
| `cwd`        | string  | Override working directory             |
| `env`        | object  | Extra environment variables            |
| `timeout`    | number  | Timeout in seconds (default: `60`)     |

---

### nu.output

Get output from background process.

```
id: "job_abc123"
```

Returns:
```
ID: job_abc123
Status: running
Running for: 15s
Exit code: (running)

Finished dev [unoptimized + debuginfo] target(s) in 0.52s
```

---

### nu.kill

Stop background process.

```
id: "job_abc123"
```

Returns:
```
ID: job_abc123
Status: killed
Command: cargo watch
```

---

### nu.apply

Edit files with Fast Apply markers.

```
path: "/path/to/file.rs"
instructions: "Add error handling"
code_edit: |
  // ... existing code ...

  fn main() -> Result<()> {
      do_work()?;
      Ok(())
  }

  // ... existing code ...
```

Returns:
```
Path: /path/to/file.rs
Status: applied
Message: Code edit applied to /path/to/file.rs
```

**Why this matters:** Closed Fast Apply services lock you into their infrastructure. Local models (Ollama, vLLM) give you privacy and control.

---

### nu.search

Search web, packages, and code via SearXNG.

```
query: "tokio"
category: "cargo"
```

Returns:
```
Query: "tokio" | Category: cargo | Found: 150 | Showing: 3

[1] tokio - Rust asynchronous runtime
    URL: https://crates.io/crates/tokio
    Engine: crates.io
    Content: An event-driven, non-blocking I/O platform...

[2] async-std - Async standard library
    URL: https://crates.io/crates/async-std
    Engine: crates.io
```

**Categories:** `general`, `cargo`, `packages`, `it`, `repos`, `code`, `skills`, `science`, `news`, `videos`, `images`, `books`, `files`

**Options:**

| Field     | Type   | Notes                                          |
| --------- | ------ | ---------------------------------------------- |
| `query`   | string | Search query                                   |
| `category` | string | Category (default: `general`)                  |
| `limit`   | number | Max results (default: `10`)                    |
| `engines` | string | Specific engines: `"npm,pypi"` (optional)      |

---

### nu.fetch

Fetch web content. HTML → Markdown automatic.

```
url: "https://example.com"
```

Returns:
```
URL: https://example.com
Status: 200
Content-Type: text/html
Format: markdown

# Example Domain
...
```

**Options:**

| Field    | Type   | Notes                                  |
| -------- | ------ | -------------------------------------- |
| `url`    | string | URL to fetch                           |
| `headers` | object | Custom HTTP headers (optional)          |
| `timeout` | number | Timeout in seconds (default: `30`)      |

---

## Nushell Quick Reference

| Task             | Command                          |
| ---------------- | -------------------------------- |
| List files       | `ls`                             |
| As JSON          | `ls \| to json`                  |
| Search files     | `ug+ 'pattern' path/`            |
| Read file        | `open file.txt`                  |
| Parse JSON       | `open data.json \| from json`    |
| System info      | `sys`                            |
| Processes        | `ps`                             |
| Change directory | `cd path/`                       |
| Make directory   | `mkdir path/` (creates parents)  |

**Syntax notes:**
- Use `;` for chaining, not `&&`
- Pipes work with structured data
- Variables: `$name`, `$env.PATH`
- External commands: `^git status`

---

## Architecture

```
src/
├── main.rs    — MCP server, tool handlers
├── exec.rs    — Command execution, background jobs, Fast Apply, search, fetch
└── state.rs   — CWD tracking, process registry
```

**Design choices:**

1. **Stateful sessions** — Working directory persists between commands
2. **Active pipe draining** — Reads stdout/stderr concurrently to prevent hangs
3. **Kill-on-timeout** — No zombie processes
4. **Markdown stripping** — `nu.apply` removes code fences before writing
5. **Plain text output** — All tools return human-readable text, not JSON

---

## Development

```bash
cargo build --release
RUST_LOG=debug cargo run
cargo fmt
cargo clippy
cargo test
```

---

## License

[MIT](./LICENSE)

---

## Related

- [Nushell](https://www.nushell.sh/) — Modern shell for structured data
- [SearXNG](https://docs.searxng.org/) — Privacy-respecting metasearch engine
- [MorphLLM](https://www.morphllm.com/) — Original Fast Apply implementation
- [Model Context Protocol](https://modelcontextprotocol.io/) — AI tool integration standard
- [Claude Code](https://github.com/anthropics/claude-code) — Claude CLI with MCP
- [Codex](https://github.com/codex-ai/codex) — AI terminal with MCP

# nu-mcp

[![CICD](https://github.com/Xpos587/nu-mcp/actions/workflows/cicd.yml/badge.svg)](https://github.com/Xpos587/nu-mcp/actions/workflows/cicd.yml)
[![Version info](https://img.shields.io/crates/v/nu-mcp.svg)](https://crates.io/crates/nu-mcp)
[![License](https://img.shields.io/crates/l/nu-mcp.svg)](LICENSE)

**Four tools. Unlimited possibilities.** A lightweight MCP server that gives AI coding agents access to Nushell—without the Bash baggage.

---

## Why nu-mcp?

Most MCP servers ship with dozens of specialized tools. One for listing files, another for searching, three more for JSON parsing, and so on. This creates tooling bloat and forces agents to learn custom APIs.

Nushell already has 400+ commands that handle pipelines, structured data, and system operations. `nu-mcp` exposes those capabilities through just four tools:

| Tool        | Purpose                                                     |
| ----------- | ----------------------------------------------------------- |
| `nu.exec`   | Run any Nushell command (blocking or background)            |
| `nu.output` | Grab output from background processes                       |
| `nu.kill`   | Terminate background tasks                                  |
| `nu.apply`  | Edit files with Fast Apply (small models, surgical patches) |

**Result:** Fewer tools to maintain, better performance, and agents that understand what they're doing—because Nushell syntax is consistent and composable.

---

## Fast Apply: Local Models, Real Edits

`nu.apply` brings FOSS Fast Apply to MCP. Instead of sending your entire codebase to a closed API, you run small models locally (Ollama, vLLM) that apply surgical edits using `// ... existing code ...` markers.

```
instruction: "Add error handling"
code_edit: |
  // ... existing code ...

  fn main() -> Result<()> {
      do_work()?;
      Ok(())
  }

  // ... existing code ...
```

The model merges your patch with the existing file and returns complete source code—no markdown fences, no conversational filler. Works with any OpenAI-compatible endpoint.

**Why this matters:** Closed Fast Apply services lock you into their infrastructure. Local models give you privacy, speed, and control.

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
  --env APPLY_API_KEY=ollama \
  --env APPLY_API_URL=http://localhost:11434/v1 \
  --env APPLY_MODEL=morph-v3-fast \
  -- /home/michael/.config/claude/nushell/target/release/nu-mcp
```

**Codex:**

```bash
codex mcp add nu-mcp \
  --env APPLY_API_KEY=ollama \
  --env APPLY_API_URL=http://localhost:11434/v1 \
  --env APPLY_MODEL=morph-v3-fast \
  -- /home/michael/.config/claude/nushell/target/release/nu-mcp
```

### Environment Variables

| Variable        | Default                       | Purpose                       |
| --------------- | ----------------------------- | ----------------------------- |
| `NU_PATH`       | `nu`                          | Path to Nushell binary        |
| `APPLY_API_URL` | `https://api.morphllm.com/v1` | LLM endpoint for code editing |
| `APPLY_API_KEY` | `ollama`                      | API key (`ollama` for local)  |
| `APPLY_MODEL`   | `morph-v3-fast`               | Model for Fast Apply edits    |
| `RUST_LOG`      | `info`                        | Log verbosity                 |

---

## Tools

### nu.exec

Run Nushell commands. Everything goes through here.

```json
{
  "command": "ls src | where size > 1kb | to json"
}
```

**Background tasks** (won't block):

```json
{
  "command": "cargo watch",
  "background": true
}
```

Returns a job ID. Use `nu.output` to see results.

**Options:**

| Field        | Type    | Required | Notes                              |
| ------------ | ------- | -------- | ---------------------------------- |
| `command`    | string  | Yes      | Nushell pipeline to run            |
| `background` | boolean | No       | Run async (default: `false`)       |
| `cwd`        | string  | No       | Override working directory         |
| `env`        | object  | No       | Extra environment variables        |
| `timeout`    | number  | No       | Timeout in seconds (default: `60`) |

---

### nu.output

Grab output from a background process.

```json
{
  "id": "job_id_from_exec",
  "block": true
}
```

**Options:**

| Field   | Type    | Required | Notes                                  |
| ------- | ------- | -------- | -------------------------------------- |
| `id`    | string  | Yes      | Job ID from `nu.exec`                  |
| `block` | boolean | No       | Wait for completion (default: `false`) |

---

### nu.kill

Stop a background process.

```json
{
  "id": "job_id_to_kill"
}
```

---

### nu.apply

Edit files with Fast Apply markers.

```json
{
  "path": "/path/to/file.rs",
  "instructions": "Add error handling to main",
  "code_edit": "// ... existing code ...\n\nfn main() -> Result<()> {\n    do_work()?;\n    Ok(())\n}\n\n// ... existing code ..."
}
```

**Response handling:** The tool automatically strips markdown code blocks and rejects conversational responses. If the model returns malformed output, it fails fast instead of corrupting your file.

---

## Nushell Quick Reference

Since `nu-mcp` passes commands directly to Nushell, you get access to its entire command set:

| Task             | Command                         |
| ---------------- | ------------------------------- |
| List files       | `ls`                            |
| As JSON          | `ls \| to json`                 |
| Search files     | `ug+ 'pattern' path/`           |
| Read file        | `open file.txt`                 |
| Parse JSON       | `open file.json \| from json`   |
| System info      | `sys`                           |
| Processes        | `ps`                            |
| Change directory | `cd path/`                      |
| Make directory   | `mkdir path/` (creates parents) |
| Run builds       | `cargo build`, `npm test`, etc. |

**Note:** Nushell uses `;` for chaining, not `&&`. Pipes work with structured data, not plain text.

---

## Architecture

```
src/
├── main.rs    — MCP server, tool handlers
├── exec.rs    — Command execution, background jobs, Fast Apply
└── state.rs   — CWD tracking, process registry
```

**Design choices:**

1. **Stateful sessions** — Working directory persists between commands, like a real shell.
2. **Active pipe draining** — Reads stdout/stderr concurrently so subprocesses don't hang.
3. **Kill-on-timeout** — No zombie processes.
4. **Markdown stripping** — `nu.apply` removes code fences and validates output before writing.

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

- [Nushell](https://www.nushell.sh/) — A modern shell for structured data
- [MorphLLM](https://www.morphllm.com/) — Original Fast Apply implementation (closed source)
- [Model Context Protocol](https://modelcontextprotocol.io/) — Standard for AI tool integration
- [Claude Code](https://github.com/anthropics/claude-code) — CLI for Claude with MCP support
- [Codex](https://github.com/codex-ai/codex) — AI terminal with MCP support

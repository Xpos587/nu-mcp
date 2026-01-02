# nu-mcp

**Four tools. Unlimited possibilities.** A lightweight MCP server that gives AI coding agents access to Nushell—without the Bash baggage.

## Installation

### Via npm (recommended)

```bash
npm install -g nu-mcp
# or
npx nu-mcp
```

### Via bun

```bash
bun install -g nu-mcp
# or
bunx nu-mcp
```

## Quick Start

### Configure Claude Code

```bash
claude mcp add nu-mcp \
  --transport stdio \
  --scope user \
  --env APPLY_API_KEY=ollama \
  --env APPLY_API_URL=http://localhost:11434/v1 \
  --env APPLY_MODEL=morph-v3-fast \
  -- nu-mcp
```

### Configure Codex

```bash
codex mcp add nu-mcp \
  --env APPLY_API_KEY=ollama \
  --env APPLY_API_URL=http://localhost:11434/v1 \
  --env APPLY_MODEL=morph-v3-fast \
  -- nu-mcp
```

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `NU_PATH` | `nu` | Path to Nushell binary |
| `APPLY_API_URL` | `https://api.morphllm.com/v1` | LLM endpoint for code editing |
| `APPLY_API_KEY` | `ollama` | API key (`ollama` for local) |
| `APPLY_MODEL` | `morph-v3-fast` | Model for Fast Apply edits |
| `RUST_LOG` | `info` | Log verbosity |

## Supported Platforms

- macOS (x64, arm64)
- Linux (x64, arm64)
- Windows (x64)

## License

MIT

## Links

- [GitHub](https://github.com/Xpos587/nu-mcp)
- [Full Documentation](https://github.com/Xpos587/nu-mcp#readme)

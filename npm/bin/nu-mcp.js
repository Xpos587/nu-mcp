#!/usr/bin/env node

const { install } = require("binary-install");
const path = require("path");

const platform = process.platform;
const arch = process.arch;

// Map npm-style platform/arch to Rust target triples
const targets = {
  "darwin arm64": "aarch64-apple-darwin",
  "darwin x64": "x86_64-apple-darwin",
  "linux arm64": "aarch64-unknown-linux-gnu",
  "linux x64": "x86_64-unknown-linux-gnu",
  "win32 x64": "x86_64-pc-windows-msvc",
};

const target = targets[`${platform} ${arch}`];

if (!target) {
  console.error(`Unsupported platform: ${platform} ${arch}`);
  console.error("Supported platforms: macOS (x64/arm64), Linux (x64/arm64), Windows (x64)");
  process.exit(1);
}

// GitHub release URL pattern: https://github.com/Xpos587/nu-mcp/releases/download/v<version>/<binary>
const version = require("../package.json").version;
const baseUrl = `https://github.com/Xpos587/nu-mcp/releases/download/v${version}`;
const binaryName = platform === "win32" ? "nu-mcp.exe" : "nu-mcp";
const url = `${baseUrl}/${target}/${binaryName}`;

const binPath = install({
  name: "nu-mcp",
  path: path.join(__dirname, "..", "binaries"),
  url: url,
});

// Execute the binary
require("child_process").spawn(binPath, process.argv.slice(2), {
  stdio: "inherit",
}).on("close", (code) => process.exit(code ?? 0));

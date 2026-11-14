#!/usr/bin/env node
// Development launcher for the Rust build; the adapter itself is not JavaScript.
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
const binary = process.env.ZCODE_ACP_BIN || fileURLToPath(new URL(`../target/release/zcode-acp${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
const child = spawn(binary, process.argv.slice(2), {
  stdio: "inherit", windowsHide: true,
  env: { ...process.env },
});
child.on("error", () => { process.stderr.write("Build zcode-acp with cargo build --release first\n"); process.exitCode = 1; });
child.on("exit", code => { process.exitCode = code ?? 1; });

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import test from "node:test";
import { fileURLToPath } from "node:url";

for (const permission of ["full", "accept-edits"]) test(`independent CLI completes a synthetic turn with ${permission} and no BB installation`, async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-package-smoke-"));
  const entry = process.env.ZCODE_TEST_PACKAGED_CLI || fileURLToPath(new URL("../bin/zcode-acp.mjs", import.meta.url));
  const fixture = fileURLToPath(new URL("./support/fake-native-server.mjs", import.meta.url));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^BB_|^ZCODE_|TOKEN|SECRET|API_KEY|CREDENTIAL/.test(key)));
  Object.assign(env, { ZCODE_NATIVE_ENTRY: fixture, ZCODE_ACP_STATE_DIR: root, ZCODE_STORAGE_DIR: path.join(root, "account"), ZCODE_DATA_BASE_DIR: root });
  const child = spawn(process.execPath, [entry, `--permission=${permission}`], { cwd: root, env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  const exited = once(child, "close"), pending = new Map(); let id = 0, stderr = "";
  child.stderr.on("data", data => { stderr += data; }); child.stdin.on("error", () => {});
  readline.createInterface({ input: child.stdout }).on("line", line => {
    const m = JSON.parse(line), request = pending.get(m.id);
    if (request) { clearTimeout(request.timer); pending.delete(m.id); m.error ? request.reject(new Error(m.error.message)) : request.resolve(m.result); }
  });
  t.after(async () => {
    child.stdin.end(); const timer = setTimeout(() => child.kill(), 3000);
    const [code] = await exited; clearTimeout(timer);
    for (const request of pending.values()) clearTimeout(request.timer);
    fs.rmSync(root, { recursive: true, force: true });
    assert.equal(code, 0); assert.equal(stderr, "");
  });
  const request = (method, params = {}) => new Promise((resolve, reject) => {
    const current = ++id, timer = setTimeout(() => { pending.delete(current); reject(new Error("Package ACP timeout")); child.kill(); }, 15000);
    pending.set(current, { resolve, reject, timer }); child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: current, method, params }) + "\n");
  });
  assert.equal((await request("initialize", { protocolVersion: 1 })).protocolVersion, 1);
  const session = await request("session/new", { cwd: root, mcpServers: [] });
  assert.equal(session.modes.currentModeId, permission === "full" ? "yolo" : "edit");
  assert.equal((await request("session/prompt", { sessionId: session.sessionId, prompt: [{ type: "text", text: "synthetic fixture only" }] })).stopReason, "end_turn");
  await request("session/close", { sessionId: session.sessionId });
});

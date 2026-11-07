import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import { NativeClient, findNativeEntry, installedDesktopVersion } from "../lib/native-client.mjs";

const entry = fileURLToPath(new URL("./support/fake-native-server.mjs", import.meta.url));
test("version discovery supports Electron's virtual ASAR package path used by BB workers", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-asar-version-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "app.asar"));
  fs.writeFileSync(path.join(root, "app.asar", "package.json"), JSON.stringify({ version: "3.17.2" }));
  assert.equal(installedDesktopVersion(path.join(root, "glm", "zcode.cjs")), "3.17.2");
  fs.writeFileSync(path.join(root, "app.asar", "package.json"), JSON.stringify({ version: "unsafe arbitrary value" }));
  assert.equal(installedDesktopVersion(path.join(root, "glm", "zcode.cjs")), "");
});
function start(t, mode = "normal", options = {}) {
  const client = new NativeClient({ entry, env: { ...process.env, FAKE_NATIVE_MODE: mode }, ...options });
  t.after(() => client.close());
  return client;
}
test("native frames omit jsonrpc, correlate out-of-order replies and preserve split UTF-8", async t => {
  const client = start(t);
  const values = await Promise.all([client.request("fixture/reverse", { n: 1 }), client.request("fixture/reverse", { n: 2 })]);
  assert.deepEqual(values, [{ n: 1 }, { n: 2 }]);
  assert.equal(await client.request("fixture/utf8"), "Привет 🌍");
});
test("a timed-out request does not prevent later calls", async t => {
  const client = start(t);
  await client.request("ready");
  await assert.rejects(client.request("fixture/hang", {}, { timeoutMs: 20 }), /timed out/);
  assert.deepEqual(await client.request("echo", { ok: true }), { ok: true });
});
test("unsupported server interactions are rejected with a native error envelope", async t => {
  const client = start(t);
  const reply = once(client, "notification");
  await client.request("fixture/peer");
  const [notification] = await reply;
  assert.equal(notification.params.error.code, -32601);
  assert.equal(Object.hasOwn(notification.params, "jsonrpc"), false);
});
for (const mode of ["malformed", "oversize", "exit"]) {
  test(`pending requests fail when the server is ${mode}`, async t => {
    const client = start(t, mode, { maxFrameBytes: 1024 });
    await assert.rejects(client.request("request"), mode === "malformed" ? /Invalid JSON/ : mode === "oversize" ? /size limit/ : /exited/);
  });
}
test("spawn errors reject pending calls", async t => {
  const client = start(t, "normal", { node: path.join(os.tmpdir(), "zcode-no-such-node.exe") });
  await assert.rejects(client.request("request"), /ENOENT/);
});
test("explicit entry override is authoritative, and Windows discovery uses LocalAppData", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-discovery-"));
  assert.equal(path.dirname(path.resolve(root)), path.resolve(os.tmpdir()));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = path.join(root, "Programs", "ZCode", "resources", "glm", "zcode.cjs");
  fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, "");
  assert.equal(findNativeEntry({ platform: "win32", env: { LOCALAPPDATA: root }, home: root }), file);
  assert.throws(() => findNativeEntry({ platform: "win32", env: { LOCALAPPDATA: root, ZCODE_NATIVE_ENTRY: path.join(root, "missing") }, home: root }), /not found/);
});

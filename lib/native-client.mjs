// ZCode Protocol uses newline-delimited JSON, without the JSON-RPC `jsonrpc` key.
import { execFile, spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export function findNativeEntry({ env = process.env, platform = process.platform, home = os.homedir() } = {}) {
  const candidates = env.ZCODE_NATIVE_ENTRY ? [env.ZCODE_NATIVE_ENTRY] : platform === "win32" ? [
    path.join(env.LOCALAPPDATA || path.join(home, "AppData", "Local"), "Programs", "ZCode", "resources", "glm", "zcode.cjs"),
    ...[env.ProgramFiles, env["ProgramFiles(x86)"]].filter(Boolean).map(root => path.join(root, "ZCode", "resources", "glm", "zcode.cjs")),
  ] : [
    "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs",
    path.join(home, "Applications", "ZCode.app", "Contents", "Resources", "glm", "zcode.cjs"),
  ];
  for (const candidate of candidates) {
    try { if (fs.statSync(candidate).isFile()) return path.resolve(candidate); } catch {}
  }
  throw new Error("ZCode's bundled CLI was not found. Set ZCODE_NATIVE_ENTRY to resources/glm/zcode.cjs.");
}

export function nativeEnvironment(entry, env = process.env) {
  const builtin = path.resolve(path.dirname(entry), "..", "config", "provider", "zcode-builtin.json");
  const storage = env.ZCODE_STORAGE_DIR || path.join(env.ZCODE_DATA_BASE_DIR || os.homedir(), ".zcode");
  const result = {
    ...env,
    ...(!env.ZCODE_BUILTIN_PROVIDER_CONFIG_FILE && fs.existsSync(builtin) ? { ZCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtin } : {}),
    ZCODE_PERSONAL_PROVIDER_CONFIG_FILE: env.ZCODE_PERSONAL_PROVIDER_CONFIG_FILE || path.join(storage, "v2", "provider_config.json"),
  };
  delete result.ELECTRON_RUN_AS_NODE;
  if (!result.ZCODE_APP_VERSION) result.ZCODE_APP_VERSION = installedDesktopVersion(entry);
  return result;
}

export function workspaceRef(cwd) {
  const workspacePath = path.resolve(cwd);
  return { workspacePath, workspaceKey: workspacePath };
}

export function installedDesktopVersion(entry) {
  const resources = path.resolve(path.dirname(entry), "..");
  try {
    // Electron's patched fs exposes ASAR members but cannot open the archive
    // as a raw file. BB host workers use that fs; standalone Node uses below.
    const packed = path.join(resources, "app.asar", "package.json");
    const stat = fs.statSync(packed);
    if (stat.isFile() && stat.size > 1 && stat.size <= 1024 * 1024) {
      const version = JSON.parse(fs.readFileSync(packed, "utf8")).version;
      if (typeof version === "string" && /^\d+\.\d+\.\d+(?:[-+][\w.-]+)?$/.test(version)) return version;
    }
  } catch { /* Plain Node does not expose files within app.asar. */ }
  let file;
  try {
    // Windows packages Electron's package.json in app.asar. Read just the
    // bounded header and that one packed file, rather than the entire archive.
    file = fs.openSync(path.join(resources, "app.asar"), "r");
    const prefix = Buffer.alloc(16);
    if (fs.readSync(file, prefix, 0, 16, 0) !== 16) return "";
    const size = prefix.readUInt32LE(12), dataOffset = 8 + prefix.readUInt32LE(4);
    if (size < 2 || size > 64 * 1024 * 1024 || dataOffset < size + 16) return "";
    const buffer = Buffer.alloc(size);
    if (fs.readSync(file, buffer, 0, size, 16) !== size) return "";
    const packed = JSON.parse(buffer.toString("utf8")).files?.["package.json"];
    const offset = Number(packed?.offset);
    if (!packed || packed.unpacked || !Number.isSafeInteger(packed.size) || packed.size < 2 || packed.size > 1024 * 1024 || !Number.isSafeInteger(offset) || offset < 0) return "";
    const content = Buffer.alloc(packed.size);
    if (fs.readSync(file, content, 0, content.length, dataOffset + offset) !== content.length) return "";
    const version = JSON.parse(content.toString("utf8")).version;
    if (typeof version === "string" && /^\d+\.\d+\.\d+(?:[-+][\w.-]+)?$/.test(version)) return version;
  } catch { /* macOS or a custom standalone CLI */ }
  finally { if (file !== undefined) fs.closeSync(file); }
  try {
    const plist = fs.readFileSync(path.resolve(resources, "..", "Info.plist"), "utf8");
    return plist.match(/<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/)?.[1] || "";
  } catch { return ""; }
}

export class NativeProtocolError extends Error {
  constructor(message, code = -32603, data) { super(message); this.code = code; this.data = data; }
}

export function publicErrorMessage(error) {
  if (error.fromNativeServer) return `ZCode runtime reported an error (code ${Number.isInteger(error.code) ? error.code : -32603}); native error contents are omitted because they can contain credentials`;
  if (/(?:Bearer\s|enc:v1:)/i.test(error.message || "")) return "ZCode request failed; sensitive diagnostic details omitted";
  return error.message || String(error);
}

// Owns one separate app-server process. It does not attach to the desktop's runtime.
export class NativeClient extends EventEmitter {
  constructor({ entry, cwd = process.cwd(), env = process.env, node = process.execPath, nodeArgs = [], timeoutMs = 30_000, onRequest, maxFrameBytes = 16 * 1024 * 1024 } = {}) {
    super();
    this.entry = entry || findNativeEntry({ env });
    this.env = nativeEnvironment(this.entry, env);
    this.timeoutMs = timeoutMs;
    this.onRequest = onRequest;
    this.maxFrameBytes = maxFrameBytes;
    this.pending = new Map();
    this.counter = 0;
    this.buffer = Buffer.alloc(0);
    this.closed = false;
    this.closing = false;
    this.stderrTail = "";
    this.child = spawn(node, [...nodeArgs, this.entry, "app-server", "--stdio", "--cwd", cwd], {
      cwd, env: this.env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true,
    });
    this.exited = new Promise(resolve => { this.resolveExit = resolve; });
    this.child.stdout.on("data", chunk => this.receive(chunk));
    this.child.stderr.on("data", chunk => {
      this.stderrTail = (this.stderrTail + chunk.toString("utf8")).slice(-8192);
      this.emit("diagnostic", chunk.toString("utf8"));
    });
    this.child.stdin.on("error", error => this.fail(error));
    this.child.on("error", error => this.fail(error));
    this.child.on("close", (code, signal) => {
      this.fail(new Error(`ZCode app-server exited (${signal || code})`));
      this.resolveExit({ code, signal });
      this.emit("exit", { code, signal });
    });
  }

  fail(error) {
    if (this.closed) return;
    this.closed = true;
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(error); }
    this.pending.clear();
    this.emit("fault", error);
  }

  receive(chunk) {
    if (this.closed) return;
    this.buffer = Buffer.concat([this.buffer, chunk]);
    let end;
    while ((end = this.buffer.indexOf(10)) >= 0) {
      if (end > this.maxFrameBytes) return this.abortProtocol("ZCode frame exceeds the size limit");
      const line = this.buffer.subarray(0, end).toString("utf8").trim();
      this.buffer = this.buffer.subarray(end + 1);
      if (!line) continue;
      let message;
      try { message = JSON.parse(line); } catch { return this.abortProtocol("Invalid JSON from ZCode app-server"); }
      if (!message || typeof message !== "object" || Array.isArray(message)) return this.abortProtocol("Invalid ZCode message");
      if (typeof message.method === "string") {
        if (message.id !== undefined) void this.handleRequest(message);
        else this.emit("notification", message);
      } else if (message.id !== undefined) {
        const pending = this.pending.get(message.id);
        if (!pending) continue; // a late response after a timeout
        this.pending.delete(message.id);
        clearTimeout(pending.timer);
        if (message.error) {
          const error = new NativeProtocolError(message.error.message, message.error.code, message.error.data);
          error.fromNativeServer = true;
          pending.reject(error);
        }
        else if (Object.hasOwn(message, "result")) pending.resolve(message.result);
        else pending.reject(new NativeProtocolError("ZCode response has no result or error"));
      }
    }
    if (this.buffer.length > this.maxFrameBytes) this.abortProtocol("ZCode frame exceeds the size limit");
  }

  killTree() {
    if (process.platform === "win32" && this.child.pid) execFile("taskkill.exe", ["/PID", String(this.child.pid), "/T", "/F"], { windowsHide: true }, () => {});
    else this.child.kill();
  }

  abortProtocol(message) { this.fail(new NativeProtocolError(message)); this.killTree(); }

  async handleRequest(message) {
    let response;
    try {
      if (!this.onRequest) throw new NativeProtocolError(`Unsupported interaction: ${message.method}`, -32601);
      response = { id: message.id, result: await this.onRequest(message.method, message.params || {}) };
    } catch (error) {
      response = { id: message.id, error: { code: error.code || -32603, message: error.message || String(error) } };
    }
    if (!this.closed && !this.closing) this.write(response);
  }

  write(message) {
    if (this.closed || this.closing) throw new Error("ZCode app-server is closed");
    this.child.stdin.write(JSON.stringify(message) + "\n");
  }

  request(method, params = {}, { timeoutMs = this.timeoutMs } = {}) {
    if (this.closed || this.closing) return Promise.reject(new Error("ZCode app-server is closed"));
    const id = `bridge-${++this.counter}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`ZCode request timed out: ${method}`)); }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      try { this.write({ id, method, params }); }
      catch (error) { this.pending.delete(id); clearTimeout(timer); reject(error); }
    });
  }

  async close({ graceMs = 2000 } = {}) {
    if (!this.closing) {
      this.closing = true;
      this.child.stdin.end();
    }
    const timer = setTimeout(() => this.killTree(), graceMs);
    try { return await this.exited; } finally { clearTimeout(timer); }
  }
}

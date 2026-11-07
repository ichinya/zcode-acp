import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export function prepareSessionState(id, cwd, resume, env = process.env) {
  if (!/^bb-native-[0-9a-f-]{36}$/.test(id)) throw new Error("Invalid native ACP session ID");
  const base = path.resolve(env.ZCODE_ACP_STATE_DIR || path.join(os.homedir(), ".zcode-bridge", "native-acp"));
  const directory = path.join(base, id);
  const manifest = path.join(directory, "binding.json");
  const canonicalCwd = fs.realpathSync(cwd);
  let saved;
  if (resume) {
    try { saved = JSON.parse(fs.readFileSync(manifest, "utf8")); } catch { throw new Error("Native ACP session binding is missing or unreadable; no new session was created"); }
    if (saved.id !== id || saved.cwd !== canonicalCwd || saved.ready !== true) throw new Error("Native ACP session binding does not match this workspace or creation is incomplete");
  } else {
    fs.mkdirSync(base, { recursive: true, mode: 0o700 });
    fs.mkdirSync(directory, { mode: 0o700 });
  }
  if (fs.lstatSync(directory).isSymbolicLink()) throw new Error("Native ACP session state cannot be a symbolic link");
  if (process.platform === "win32") {
    try {
      execFileSync("powershell.exe", ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", fileURLToPath(new URL("../native/protect-state.ps1", import.meta.url)), "-Directory", directory], { stdio: ["ignore", "ignore", "pipe"], windowsHide: true, timeout: 15_000 });
    } catch { throw new Error("Cannot protect the native ACP session directory with a current-user Windows ACL"); }
  } else fs.chmodSync(directory, 0o700);
  saved ||= { version: 1, id, cwd: canonicalCwd, ready: false, inputSubmitted: false };
  const save = patch => {
    const next = { ...saved, ...patch }, temporary = path.join(directory, "binding.tmp");
    const fd = fs.openSync(temporary, "w", 0o600);
    try { fs.writeFileSync(fd, JSON.stringify(next) + "\n"); fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
    fs.renameSync(temporary, manifest); saved = next;
  };
  if (!resume) save({});
  return {
    directory,
    env: { ...env, ZCODE_SESSION_DB_PATH: path.join(directory, "db.sqlite"), ZCODE_LOG_DIR: path.join(directory, "logs") },
    get inputSubmitted() { return saved.inputSubmitted; },
    get preferences() { return saved.preferences || {}; },
    markInput: () => save({ inputSubmitted: true }),
    markReady: () => save({ ready: true }),
    savePreferences: snapshot => save({ preferences: { mode: snapshot.settings.mode.current, ...(snapshot.settings.model.current ? { model: snapshot.settings.model.current } : {}) } }),
  };
}

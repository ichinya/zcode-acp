// Shared by the standalone command and the explicit stdio compatibility entry.
// CLI overrides are scoped to this agent; never mutate the parent's environment.
export function cliEnvironment(args = process.argv.slice(2), env = process.env) {
  let nativeEntry;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg !== "--native-entry" && !arg.startsWith("--native-entry=")) continue;
    if (nativeEntry !== undefined) throw new Error("--native-entry may be specified only once");
    const value = arg === "--native-entry" ? args[++i] : arg.slice("--native-entry=".length);
    if (!value?.trim() || value.startsWith("--") || value.includes("\0")) {
      throw new Error("--native-entry requires a path to ZCode's resources/glm/zcode.cjs");
    }
    nativeEntry = value.trim();
  }
  return { ...env, ...(nativeEntry === undefined ? {} : { ZCODE_NATIVE_ENTRY: nativeEntry }) };
}

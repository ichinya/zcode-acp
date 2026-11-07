#!/usr/bin/env node
// Standalone ACP entry point. No BB dependency. Saved Desktop login is reused.
import { cliEnvironment } from "../lib/cli-options.mjs";
const args = process.argv.slice(2);
if (args.includes("--help")) {
    process.stdout.write("zcode-acp\n  zcode-acp                              ACP over stdin/stdout\n  zcode-acp --doctor                     account/model check, no generation\n  --native-entry PATH                   bundled resources/glm/zcode.cjs\n  ZCODE_NATIVE_ENTRY                    environment alternative; CLI flag wins\n  --permission=accept-edits|full          permission policy\n  --bb-permission=...                    compatibility alias\n");
} else if (args.includes("--doctor")) {
  try {
    const { nativeDoctor } = await import("../lib/native-doctor.mjs");
    const result = await nativeDoctor(undefined, { env: cliEnvironment(args) });
    process.stdout.write(JSON.stringify(result, null, 2) + "\n");
    if (!result.ok) process.exitCode = 1;
  } catch (error) {
    const { publicErrorMessage } = await import("../lib/native-client.mjs");
    process.stdout.write(JSON.stringify({ ok: false, error: publicErrorMessage(error), modelRequestsSent: 0 }) + "\n");
    process.exitCode = 1;
  }
} else await import("../lib/stdio.mjs");

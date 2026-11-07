// Opt-in ACP entry point. The existing desktop accessibility provider is unchanged.
import readline from "node:readline";
import { NativeAcpAgent } from "./native-acp.mjs";
import { publicErrorMessage } from "./native-client.mjs";

const pending = new Map();
let counter = 0;
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n");
const agent = new NativeAcpAgent({
  permissionMode: process.argv.find(arg => arg.startsWith("--bb-permission="))?.split("=")[1] ?? process.argv.find(arg => arg.startsWith("--permission="))?.split("=")[1],
  update: (sessionId, update) => send({ method: "session/update", params: { sessionId, update } }),
  requestClient(method, params, signal) {
    const id = `native-client-${++counter}`;
    return new Promise((resolve, reject) => {
      const cleanup = () => { clearTimeout(timer); pending.delete(id); signal?.removeEventListener("abort", abort); };
      const abort = () => { cleanup(); reject(new Error("Client interaction cancelled")); };
      const timer = setTimeout(abort, 10 * 60 * 1000);
      pending.set(id, { resolve: value => { cleanup(); resolve(value); }, reject: error => { cleanup(); reject(error); } });
      if (signal?.aborted) return abort();
      signal?.addEventListener("abort", abort, { once: true });
      send({ id, method, params });
    });
  },
});
const input = readline.createInterface({ input: process.stdin });
input.on("line", line => {
  let message;
  try { message = JSON.parse(line); } catch { send({ id: null, error: { code: -32700, message: "Invalid JSON" } }); return; }
  if (!message || typeof message !== "object" || Array.isArray(message)) return;
  if (!message.method) {
    const request = pending.get(message.id);
    if (request) message.error ? request.reject(new Error(message.error.message)) : request.resolve(message.result);
    return;
  }
  if (message.id === undefined && message.method !== "session/cancel") return;
  agent.handle(message.method, message.params).then(
    result => { if (message.id !== undefined) send({ id: message.id, result }); },
    error => { if (message.id !== undefined) send({ id: message.id, error: { code: error.code || -32603, message: publicErrorMessage(error) } }); },
  );
});
let closing = false;
function close() {
  if (closing) return;
  closing = true;
  const timer = setTimeout(() => process.exit(1), 35_000);
  timer.unref();
  agent.close().finally(() => { for (const p of pending.values()) p.reject(new Error("Agent closed")); clearTimeout(timer); input.close(); });
}
input.on("close", close);
process.on("SIGINT", close);
process.on("SIGTERM", close);

import readline from "node:readline";

const mode = process.env.FAKE_NATIVE_MODE || "normal";
const write = m => process.stdout.write(JSON.stringify(m) + "\n");
const held = [];
const input = readline.createInterface({ input: process.stdin });
let sessionId, eventSeq = 0, pendingPermission;
const model = { providerId: "fake", modelId: "offline" };
let selectedModel = { ...model, options: { reasoningLevel: "high" } }, selectedMode = "build";
const snapshot = () => ({
  session: { sessionId, mode: selectedMode }, messages: [], runtime: { eventSeq },
  settings: { model: { current: selectedModel, available: [{ ref: model, label: "Offline fixture", reasoning: { levels: [{ value: "low", label: "Low" }, { value: "high", label: "High" }, { value: "max", label: "Max" }], defaultLevel: "high" } }] }, mode: { current: selectedMode }, thoughtLevel: { enabled: false, available: [] } },
});
const event = (type, payload = {}) => write({ method: "session/event", params: { sessionId, seq: ++eventSeq, type, payload } });
input.on("line", line => {
  const m = JSON.parse(line);
  if (Object.hasOwn(m, "jsonrpc")) { write({ id: m.id, error: { code: -32600, message: "No jsonrpc field in ZCode Protocol" } }); return; }
  if (!m.method) {
    if (pendingPermission) { write({ method: "fixture/permission", params: m }); event("turn.completed", { response: m.result?.decision || "error", resultType: "success" }); pendingPermission = false; }
    else write({ method: "fixture/reply", params: m });
    return;
  }
  if (mode === "malformed") { process.stdout.write("not-json\n"); return; }
  if (mode === "oversize") { process.stdout.write("x".repeat(2048)); return; }
  if (mode === "exit") { process.exit(3); }
  if (m.method === "fixture/hang") return;
  if (m.method === "fixture/peer") { write({ id: m.id, result: {} }); write({ id: "peer-1", method: "unknown/interaction", params: {} }); return; }
  if (m.method === "fixture/utf8") {
    const buffer = Buffer.from(JSON.stringify({ id: m.id, result: "Привет 🌍" }) + "\n");
    const split = buffer.indexOf(Buffer.from("П")) + 1;
    process.stdout.write(buffer.subarray(0, split));
    setTimeout(() => process.stdout.write(buffer.subarray(split)), 10);
    return;
  }
  if (m.method === "fixture/reverse") {
    held.push(m);
    if (held.length === 2) for (const request of held.reverse()) write({ id: request.id, result: request.params });
    return;
  }
  if (m.method === "session/create" || m.method === "session/resume") {
    sessionId = m.params.sessionId;
    selectedMode = m.params.mode || "build";
    if (m.params.model) selectedModel = m.params.model;
    if (process.env.FAKE_NATIVE_DELAY) { setTimeout(() => write({ id: m.id, result: snapshot() }), 80); return; }
    write({ id: m.id, result: snapshot() }); return;
  }
  if (m.method === "session/read") { write({ id: m.id, result: snapshot() }); return; }
  if (m.method === "session/setModel") {
    if (!m.params.model.options?.reasoningLevel) { write({ id: m.id, error: { code: -32603, message: "Reasoning level is required" } }); return; }
    selectedModel = m.params.model; write({ id: m.id, result: snapshot() }); return;
  }
  if (m.method === "session/setMode") { selectedMode = m.params.mode; write({ id: m.id, result: snapshot() }); return; }
  if (m.method === "session/subscribe") { write({ id: m.id, result: { sessionId, eventSeq, events: [] } }); return; }
  if (m.method === "session/send") {
    if (mode === "error") { write({ id: m.id, error: { code: -32000, message: "do-not-leak-fixture-secret" } }); return; }
    write({ id: m.id, result: { accepted: true, sessionId, stateRevision: 1 } });
    if (mode === "wait") return;
    if (mode === "permission") {
      pendingPermission = true;
      write({ id: "approval", method: "interaction/requestPermission", params: { sessionId, toolCallId: "tool1", toolName: "write", input: { path: "x" }, options: [
        { optionId: "once", name: "Allow once", kind: "allow_once" }, { optionId: "always", name: "Always", kind: "allow_always" }, { optionId: "deny", name: "Deny", kind: "deny" },
      ] } }); return;
    }
    event("message.upserted", { type: "user", content: "must not be echoed as assistant" });
    event("model.streaming", { kind: "reasoning_delta", delta: "Thinking" });
    event("model.streaming", { kind: "text_delta", delta: "Привет" });
    event("model.streaming", { kind: "text_delta", delta: " 🌍" });
    event("tool.updated", { kind: "scheduled", toolCallId: "t1", toolName: "read", input: { path: "AGENTS.md" } });
    event("tool.updated", { kind: "result", toolCallId: "t1", result: { content: "ok" } });
    event("turn.completed", { response: "Привет 🌍", resultType: "success" }); return;
  }
  if (m.method === "session/stop") { event("turn.completed", { response: "", resultType: "cancelled" }); }
  write({ id: m.id, result: m.params });
});
input.on("close", () => process.exit(0));

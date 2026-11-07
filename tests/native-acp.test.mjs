import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { NativeAcpAgent } from "../lib/native-acp.mjs";
import { NativeClient } from "../lib/native-client.mjs";

const entry = fileURLToPath(new URL("./support/fake-native-server.mjs", import.meta.url));
const cwd = path.resolve(path.dirname(entry), "../..");
function setup(t, mode = "normal", requestClient = async () => ({ outcome: { outcome: "cancelled" } }), extraEnv = {}) {
  const updates = [], calls = [];
  const stateRoot = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-native-acp-"));
  assert.equal(path.dirname(path.resolve(stateRoot)), path.resolve(os.tmpdir()));
  const env = { ...process.env, ZCODE_ACP_STATE_DIR: stateRoot, ZCODE_NATIVE_MODE: "build", FAKE_NATIVE_MODE: mode, ...extraEnv };
  const agent = new NativeAcpAgent({
    env, update: (sessionId, update) => updates.push({ sessionId, ...update }), requestClient,
    accountFactory: () => ({ initialize: async () => {}, close() {}, authorize: async () => ({ headersApplied: false, errorMessage: "No fixture account" }), authorizeOfficialMcp: async () => ({ ok: false }) }),
    clientFactory: options => {
      const client = new NativeClient({ ...options, entry });
      const request = client.request.bind(client);
      client.request = (method, params, ...rest) => { calls.push({ method, params }); return request(method, params, ...rest); };
      return client;
    },
  });
  t.after(async () => { await agent.close(); fs.rmSync(stateRoot, { recursive: true, force: true }); });
  const open = () => agent.handle("session/new", { cwd, mcpServers: [] });
  const prompt = sessionId => agent.handle("session/prompt", { sessionId, prompt: [{ type: "text", text: "offline fixture only" }] });
  return { agent, updates, calls, open, prompt };
}
test("ACP streams native text, thoughts and tools without duplicating final response or user text", async t => {
  const d = setup(t);
  assert.equal((await d.agent.handle("initialize")).protocolVersion, 1);
  const session = await d.open();
  assert.equal(session.modes.currentModeId, "build");
  assert.deepEqual(await d.prompt(session.sessionId), { stopReason: "end_turn" });
  assert.equal(d.updates.filter(u => u.sessionUpdate === "agent_message_chunk").map(u => u.content.text).join(""), "Привет 🌍");
  assert.equal(d.updates.filter(u => u.sessionUpdate === "agent_thought_chunk").map(u => u.content.text).join(""), "Thinking");
  assert.deepEqual(d.updates.filter(u => u.toolCallId).map(u => [u.sessionUpdate, u.status]), [["tool_call", "pending"], ["tool_call_update", "completed"]]);
  assert.equal(d.calls.filter(c => c.method === "session/send").length, 1);
});

test("grouped model choices route the selected plan/provider and preserve reasoning", async t => {
  const d = setup(t); const session = await d.open();
  const s = d.agent.get(session.sessionId);
  const base = s.snapshot.settings.model.available[0];
  s.snapshot.settings.model.available = [
    { ...base, ref: { providerId: "account:zai-individual-coding-plan", modelId: "same" }, label: "Same model" },
    { ...base, ref: { providerId: "account:zai-start-plan", modelId: "same" }, label: "Same model" },
    { ...base, ref: { providerId: "personal", modelId: "same" }, label: "Same model", providerLabel: "Personal API" },
  ];
  const grouped = d.agent.state(s);
  const groups = grouped.configOptions.find(o => o.id === "model").options;
  assert.deepEqual(groups.map(g => g.name), ["Coding Plan · Z.ai", "Start Plan · Z.ai", "Personal API"]);
  for (const group of groups) {
    const choice = group.options[0];
    const result = await d.agent.handle("session/set_config_option", { sessionId: s.id, configId: "model", value: choice.value });
    const selected = d.calls.filter(c => c.method === "session/setModel").at(-1).params.model;
    assert.equal(selected.providerId, group.group);
    assert.equal(selected.modelId, "same");
    assert.equal(selected.options.reasoningLevel, "high");
    assert.equal(d.agent.state(s).models.currentModelId, choice.value);
    assert.equal(result.configOptions.find(o => o.id === "effort").currentValue, "high");
  }
});
for (const choice of ["once", "always", "deny"]) {
  test(`ACP permission selection ${choice} permits only allow-once`, async t => {
    const d = setup(t, "permission", async (method, params) => {
      assert.equal(method, "session/request_permission");
      assert.deepEqual(params.options.map(o => o.optionId), ["once", "deny"]);
      return { outcome: { outcome: "selected", optionId: choice } };
    });
    const { sessionId } = await d.open();
    await d.prompt(sessionId);
    assert.equal(d.updates.at(-1).content.text, choice === "once" ? "allow" : "deny");
  });
}
test("cancel stops the native session once and completes the pending ACP turn", async t => {
  const d = setup(t, "wait");
  const { sessionId } = await d.open();
  const turn = d.prompt(sessionId);
  await Promise.all([d.agent.handle("session/cancel", { sessionId }), d.agent.handle("session/cancel", { sessionId })]);
  assert.deepEqual(await turn, { stopReason: "cancelled" });
  assert.equal(d.calls.filter(c => c.method === "session/stop").length, 1);
});
test("closing a session cancels a pending client permission request", async t => {
  let permissionStarted;
  const started = new Promise(resolve => { permissionStarted = resolve; });
  const d = setup(t, "permission", (_method, _params, signal) => new Promise((_resolve, reject) => {
    permissionStarted(); signal.addEventListener("abort", () => reject(new Error("cancelled")), { once: true });
  }));
  const { sessionId } = await d.open();
  const turn = d.prompt(sessionId);
  await started;
  await d.agent.handle("session/close", { sessionId });
  assert.equal((await turn).stopReason, "cancelled");
  assert.equal(d.agent.sessions.size, 0);
});
test("MCP settings are passed in memory to the native session, without editing the project", async t => {
  const d = setup(t);
  await d.agent.handle("session/new", { cwd, mcpServers: [{ name: "fixture", command: "node", args: [], env: [{ name: "TOKEN", value: "dummy-offline-token" }] }] });
  const create = d.calls.find(c => c.method === "session/create");
  assert.equal(create.params.mcpServers[0].isolation, "session");
  assert.equal(create.params.mcpServers[0].env[0].value, "dummy-offline-token");
});
test("adapter refuses desktop session IDs, malformed modes, unsupported content and concurrent prompts", async t => {
  const d = setup(t, "wait");
  await assert.rejects(d.agent.handle("session/load", { cwd, sessionId: "desktop-task" }), /Only sessions created/);
  const { sessionId } = await d.open();
  await assert.rejects(d.agent.handle("session/set_mode", { sessionId, modeId: "invalid" }), /Unknown mode/);
  await assert.rejects(d.agent.handle("session/prompt", { sessionId, prompt: [{ type: "image", data: "" }] }), /Unsupported prompt/);
  const turn = d.prompt(sessionId);
  await assert.rejects(d.prompt(sessionId), /already running/);
  await d.agent.handle("session/cancel", { sessionId }); await turn;
});
test("agent shutdown also waits for sessions that are still being created", async t => {
  const d = setup(t, "normal", undefined, { FAKE_NATIVE_DELAY: "1" });
  const creation = d.open();
  await d.agent.close();
  await assert.rejects(creation, /Agent is closed/);
  assert.equal(d.agent.sessions.size, 0);
});
test("account header requests are handled by the account host, and session identity is checked", async t => {
  const d = setup(t);
  const { sessionId } = await d.open();
  assert.equal((await d.agent.interaction(d.agent.get(sessionId), "interaction/requestProviderRuntimeHeaders", { sessionId })).headersApplied, false);
  await assert.rejects(d.agent.interaction(d.agent.get(sessionId), "interaction/requestProviderRuntimeHeaders", { sessionId: "another-session" }), /another session/);
});

test("BB accept-edits cannot be escalated to full access through ACP mode selection", async t => {
  const d = setup(t);
  d.agent.permissionMode = "accept-edits";
  const session = await d.open();
  assert.equal(session.modes.availableModes.some(m => m.id === "yolo"), false);
  await assert.rejects(d.agent.handle("session/set_mode", { sessionId: session.sessionId, modeId: "yolo" }), /requires BB full/);
});

test("a native session binding survives cold resume and refuses another workspace", async t => {
  const d = setup(t);
  const { sessionId } = await d.open();
  const s = d.agent.get(sessionId);
  const file = path.join(s.state.directory, "binding.json");
  assert.equal(JSON.parse(fs.readFileSync(file)).ready, true);
  assert.equal(s.client.env.ZCODE_SESSION_DB_PATH, path.join(s.state.directory, "db.sqlite"));
  await d.agent.handle("session/close", { sessionId });
  await d.agent.handle("session/load", { sessionId, cwd, mcpServers: [] });
  await d.agent.handle("session/close", { sessionId });
  await assert.rejects(d.agent.handle("session/load", { sessionId, cwd: path.dirname(cwd), mcpServers: [] }), /does not match/);
});

test("native startup runtime preferences are answered explicitly without automatic memory requests", async t => {
  const d = setup(t); const { sessionId } = await d.open();
  const s = d.agent.get(sessionId);
  const preferences = await d.agent.interaction(s, "session/requestRuntimePreferences", { sessionId });
  assert.equal(preferences.memoryEnabled, false);
  assert.equal(preferences.subagentRuntimeConfigEnabled, true);
  assert.deepEqual(await d.agent.interaction(s, "subagents/readRuntimeConfig", { sessionId }), { kind: "built-in-fallback" });
});

test("model-native reasoning levels work when the legacy thought-level setting is disabled", async t => {
  const d = setup(t); const initial = await d.open();
  assert.deepEqual(initial.configOptions.find(option => option.id === "effort").options.map(option => option.value), ["low", "high", "max"]);
  await d.agent.handle("session/set_config_option", { sessionId: initial.sessionId, configId: "model", value: initial.models.currentModelId });
  assert.equal(d.calls.filter(call => call.method === "session/setModel").at(-1).params.model.options.reasoningLevel, "high");
  const changed = await d.agent.handle("session/set_config_option", { sessionId: initial.sessionId, configId: "effort", value: "max" });
  assert.equal(changed.configOptions.find(option => option.id === "effort").currentValue, "max");
  assert.equal(d.agent.state(d.agent.get(initial.sessionId)).models.currentModelId, initial.models.currentModelId);
  assert.equal(d.calls.filter(call => call.method === "session/setModel").at(-1).params.model.options.reasoningLevel, "max");
});

test("missing native history after a submitted input never creates a replacement session", async t => {
  const d = setup(t); const { sessionId } = await d.open();
  await d.prompt(sessionId);
  await d.agent.handle("session/close", { sessionId });
  await assert.rejects(d.agent.handle("session/load", { sessionId, cwd, mcpServers: [] }), /nothing was resent/);
  assert.equal(d.calls.filter(call => call.method === "session/create").length, 1);
});

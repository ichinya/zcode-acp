import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { NativeClient, NativeProtocolError, workspaceRef } from "./native-client.mjs";
import { NativeAccountHost } from "./native-account.mjs";
import { prepareSessionState } from "./native-state.mjs";
import { modelCatalog, nativeModelId as modelId } from "./model-catalog.mjs";

const modes = [
  { id: "build", name: "Ask before changes" }, { id: "plan", name: "Plan" },
  { id: "edit", name: "Accept edits" }, { id: "yolo", name: "Full access" },
];
const invalid = message => new NativeProtocolError(message, -32602);
function selectionFor(model) {
  const levels = model.reasoning?.levels || [];
  if (!levels.length) return model.ref;
  const requested = model.ref.options?.reasoningLevel || model.reasoning.defaultLevel;
  const value = levels.some(level => level.value === requested) ? requested : levels[0].value;
  return { ...model.ref, options: { reasoningLevel: value } };
}
function reasoning(s) {
  const settings = s.snapshot.settings, current = settings.model.current;
  const model = current && settings.model.available.find(item => modelId(item.ref) === modelId(current));
  const levels = model?.reasoning?.levels || (settings.thoughtLevel.enabled ? settings.thoughtLevel.available : []);
  return { levels, current: current?.options?.reasoningLevel || settings.thoughtLevel.current || model?.reasoning?.defaultLevel || settings.thoughtLevel.defaultLevel || levels[0]?.value, modelSelection: Boolean(model?.reasoning?.levels?.length) };
}

export class NativeAcpAgent {
  constructor({ update, requestClient, env = process.env, clientFactory = options => new NativeClient(options), accountFactory = client => new NativeAccountHost({ env: client.env }), turnTimeoutMs = 4 * 60 * 60 * 1000, permissionMode } = {}) {
    this.update = update;
    this.requestClient = requestClient;
    this.env = env;
    this.clientFactory = clientFactory;
    this.accountFactory = accountFactory;
    this.permissionMode = permissionMode;
    this.turnTimeoutMs = turnTimeoutMs;
    this.sessions = new Map();
    this.creating = new Set();
    this.opening = new Set();
    this.closed = false;
  }

  get(id) { const s = this.sessions.get(id); if (!s) throw invalid("Unknown native session"); return s; }
  async acceptSnapshot(s, changed) {
    if (!changed?.settings) { s.snapshot = await s.client.request("session/read", { sessionId: s.id, messageLimit: 1 }); return; }
    // Configuration commands return a "current" availability snapshot. Keep
    // the session's complete catalog while incorporating its updated entries.
    const available = new Map(s.snapshot.settings.model.available.map(item => [modelId(item.ref), item]));
    for (const item of changed.settings.model.available) available.set(modelId(item.ref), item);
    s.snapshot = { ...changed, settings: { ...changed.settings, model: { ...changed.settings.model, available: [...available.values()] } } };
  }
  state(s) {
    const settings = s.snapshot.settings;
    const models = (settings.model.available || []).filter(m => !m.disabledReason);
    const current = settings.model.current;
    const effort = reasoning(s);
    const catalog = modelCatalog(models);
    return {
      modes: { currentModeId: settings.mode.current, availableModes: modes.filter(m => this.permissionMode !== "accept-edits" || m.id !== "yolo") },
      ...(models.length ? { models: { currentModelId: current ? modelId(current) : modelId(models[0].ref), availableModels: catalog.models } } : {}),
      configOptions: [
        ...(models.length ? [{ id: "model", name: "Model", category: "model", type: "select", currentValue: current ? modelId(current) : modelId(models[0].ref), options: catalog.options }] : []),
        ...(effort.levels.length ? [{
        id: "effort", name: "Reasoning", category: "thought_level", type: "select",
        currentValue: effort.current,
        options: effort.levels.map(e => ({ value: e.value, name: e.label })),
      }] : []),
      ],
    };
  }

  open(params, resume = false) {
    const operation = this.openSession(params, resume);
    this.opening.add(operation);
    operation.then(() => this.opening.delete(operation), () => this.opening.delete(operation));
    return operation;
  }

  async openSession(params, resume = false) {
    if (this.closed) throw new Error("Agent is closed");
    if (typeof params.cwd !== "string" || !path.isAbsolute(params.cwd) || !fs.statSync(params.cwd).isDirectory()) throw invalid("cwd must be an existing absolute directory");
    const id = resume ? params.sessionId : `bb-native-${randomUUID()}`;
    // A separate app-server must never take over a task owned by the desktop.
    if (typeof id !== "string" || !/^bb-native-[0-9a-f-]{36}$/.test(id)) throw invalid("Only sessions created by this native adapter can be resumed");
    if (this.sessions.has(id) || this.creating.has(id)) throw invalid("Session is already open");
    this.creating.add(id);
    let s;
    try {
      const state = prepareSessionState(id, params.cwd, resume, this.env);
      s = { id, cwd: params.cwd, state, client: null, account: null, active: null, snapshot: null, eventSeq: 0, closing: false, cancelling: null };
      s.client = this.clientFactory({ cwd: params.cwd, env: state.env, onRequest: (method, p) => this.interaction(s, method, p) });
      s.client.on("notification", m => { if (m.method === "session/event") this.event(s, m.params); });
      s.client.on("fault", error => this.finish(s, { error }));
      s.account = this.accountFactory(s.client);
      await s.account.initialize(s.client);
      const workspace = workspaceRef(params.cwd);
      const mcpServers = (params.mcpServers || []).map(server => server.type === "http" || server.type === "sse"
        ? { name: server.name, type: server.type, url: server.url, headers: server.headers || [], isolation: "session" }
        : { name: server.name, command: server.command, args: server.args || [], env: server.env || [], isolation: "session" });
      const mode = this.env.ZCODE_NATIVE_MODE || (this.permissionMode === "full" ? "yolo" : this.permissionMode === "accept-edits" ? "edit" : "build");
      if (!modes.some(m => m.id === mode)) throw invalid("ZCODE_NATIVE_MODE must be build, plan, edit or yolo");
      if (this.permissionMode === "accept-edits" && mode === "yolo") throw invalid("Full access requires BB full permissions");
      let restorePersisted = resume;
      if (resume) {
        const native = await s.client.request("session/list", { sessionIds: [id] });
        restorePersisted = native.sessions?.some(item => item.sessionId === id) === true;
        if (!restorePersisted && state.inputSubmitted !== false) throw new Error("Native history is missing after an input was attempted or its outcome is unknown; no new session was created and nothing was resent");
      }
      const initialMode = resume ? state.preferences.mode || mode : mode;
      s.snapshot = await s.client.request(restorePersisted ? "session/resume" : "session/create", {
        sessionId: id, workspace, mcpServers,
        ...(!restorePersisted ? { mode: this.permissionMode === "accept-edits" && initialMode === "yolo" ? "edit" : initialMode, ...(resume && state.preferences.model ? { model: state.preferences.model } : {}), persistence: "immediate", titleGenerationEnabled: false } : {}),
      });
      if (s.snapshot.session?.sessionId !== id) throw new Error("Native server returned a different session identity");
      if (!s.snapshot.settings.model.current) {
        const model = s.snapshot.settings.model.available.find(item => !item.disabledReason);
        if (model) {
          await this.acceptSnapshot(s, await s.client.request("session/setModel", { sessionId: id, model: selectionFor(model) }));
        }
      }
      if (this.permissionMode === "accept-edits" && s.snapshot.settings.mode.current === "yolo") {
        await this.acceptSnapshot(s, await s.client.request("session/setMode", { sessionId: id, mode: "edit" }));
      }
      state.markReady();
      state.savePreferences(s.snapshot);
      if (this.closed) throw new Error("Agent is closed");
      this.sessions.set(id, s);
      const subscribed = await s.client.request("session/subscribe", { sessionId: id, deliveryKind: "desktop-continuous", includeSnapshot: false });
      s.eventSeq = subscribed.eventSeq;
      if (resume) this.replay(s);
      return { ...(!resume ? { sessionId: id } : {}), ...this.state(s) };
    } catch (error) {
      this.sessions.delete(id);
      s?.account?.close();
      await s?.client?.close();
      throw error;
    } finally { this.creating.delete(id); }
  }

  replay(s) {
    for (const message of s.snapshot.messages || []) {
      if (message.info.role !== "assistant" && message.info.role !== "user") continue;
      for (const part of message.parts || []) {
        if (part.type === "text" || part.type === "reasoning") this.update(s.id, {
          sessionUpdate: part.type === "reasoning" ? "agent_thought_chunk" : message.info.role === "user" ? "user_message_chunk" : "agent_message_chunk",
          content: { type: "text", text: part.text },
        });
      }
    }
  }

  async interaction(s, method, params) {
    if (params.sessionId && params.sessionId !== s.id) throw invalid("Interaction belongs to another session");
    if (method === "session/requestRuntimePreferences") return {
      nativeSearchEnhancementsEnabled: false,
      toolSearchMode: "off",
      subagentRuntimeConfigEnabled: true,
      memoryEnabled: this.env.ZCODE_NATIVE_MEMORY === "1",
      askUserQuestionAutoResolutionEnabled: false,
      modelContextBudgetStrategy: "preflight-v1",
    };
    if (method === "subagents/readRuntimeConfig") return { kind: "built-in-fallback" };
    if (method === "interaction/requestPermission") {
      if (!s.active || s.closing) return { decision: "deny", reason: "No active client turn" };
      const options = (params.options || []).filter(o => o.kind === "allow_once" || o.kind === "deny");
      try {
        const result = await this.requestClient("session/request_permission", {
          sessionId: s.id,
          toolCall: { toolCallId: params.toolCallId, title: params.toolName, status: "pending", rawInput: params.input },
          options: options.map(o => ({ optionId: o.optionId, name: o.name, kind: o.kind === "deny" ? "reject_once" : "allow_once" })),
        }, s.active.abort.signal);
        const selected = result?.outcome?.outcome === "selected" && options.find(o => o.optionId === result.outcome.optionId);
        return { decision: selected?.kind === "allow_once" ? "allow" : "deny", reason: "BB permission decision" };
      } catch { return { decision: "deny", reason: "BB permission request cancelled or timed out" }; }
    }
    if (method === "interaction/requestProviderRuntimeHeaders") return s.account.authorize(params, s.id, s.cwd);
    if (method === "interaction/requestOfficialMcpAuthHeaders") return s.account.authorizeOfficialMcp(params, s.cwd);
    throw new NativeProtocolError(`Unsupported ZCode interaction: ${method}`, -32601);
  }

  event(s, event) {
    if (!event || event.sessionId !== s.id || !s.active) return;
    if (Number.isInteger(event.seq)) { if (event.seq <= s.eventSeq) return; s.eventSeq = event.seq; }
    const p = event.payload || {};
    const active = s.active;
    if (event.turnId && active.turnId && event.turnId !== active.turnId) return;
    if (p.inputId && p.inputId !== active.inputId) return;
    if (event.type === "turn.started" && event.turnId) active.turnId = event.turnId;
    // The legacy stdio subscription delivers model.streaming. Projection part
    // events can also contain user text, so only the model stream is forwarded.
    if (event.type === "model.streaming") {
      if (p.kind === "text_delta") this.chunk(s, "text", p.delta || "");
      else if (p.kind === "reasoning_delta") this.chunk(s, "reasoning", p.delta || "");
    } else if (event.type === "tool.updated" && p.toolCallId) {
      const key = p.toolCallId;
      const first = !active.tools.has(key);
      active.tools.add(key);
      const status = p.kind === "result" ? "completed" : p.kind === "error" ? "failed" : p.kind === "scheduled" ? "pending" : "in_progress";
      this.update(s.id, {
        sessionUpdate: first ? "tool_call" : "tool_call_update", toolCallId: key, status,
        ...(first ? { title: p.toolName || "ZCode tool", kind: "other" } : {}),
        ...(p.input !== undefined ? { rawInput: p.input } : {}),
        ...(p.result !== undefined || p.error !== undefined ? { rawOutput: p.result ?? p.error } : {}),
      });
    } else if (event.type === "turn.completed") {
      if (!active.emittedText && p.response) this.chunk(s, "text", p.response);
      if (p.resultType && !["success", "cancelled"].includes(p.resultType)) this.finish(s, { error: new Error(`ZCode turn ended: ${p.resultType}`) });
      else this.finish(s, { stopReason: p.resultType === "cancelled" || active.cancelled ? "cancelled" : "end_turn" });
    } else if (event.type === "turn.failed") {
      const error = new NativeProtocolError("ZCode turn failed", -32603);
      error.fromNativeServer = true;
      this.finish(s, { error });
    }
    else if (event.type === "session.closed") this.finish(s, { error: new Error("ZCode session closed during a turn") });
  }

  chunk(s, type, text) {
    if (!text) return;
    if (type === "text") s.active.emittedText = true;
    this.update(s.id, { sessionUpdate: type === "reasoning" ? "agent_thought_chunk" : "agent_message_chunk", content: { type: "text", text } });
  }

  finish(s, result) {
    const active = s.active;
    if (!active) return;
    clearTimeout(active.timer);
    active.abort.abort();
    s.active = null;
    active.resolve(result);
  }

  async prompt(s, params) {
    if (s.closing) throw invalid("Session is closing");
    if (s.cancelling) throw invalid("Session cancellation is still in progress");
    if (s.active) throw invalid("A prompt is already running");
    if (!Array.isArray(params.prompt)) throw invalid("prompt must contain text blocks");
    const content = params.prompt.map(block => {
      if (block.type === "text" && typeof block.text === "string") return block.text;
      if (block.type === "resource" && typeof block.resource?.text === "string") return block.resource.text;
      throw invalid(`Unsupported prompt block: ${block.type}`);
    }).join("\n");
    if (!content.trim()) throw invalid("Prompt is empty");
    if (!s.snapshot.settings.model.current && !s.snapshot.settings.model.available.length) throw new NativeProtocolError("No available model. Sign in to an individual Coding Plan in ZCode Desktop and open Coding Plan there once, or configure a personal API provider in ZCode.");
    const active = { inputId: randomUUID(), abort: new AbortController(), tools: new Set(), emittedText: false, cancelled: false };
    const done = new Promise(resolve => { active.resolve = resolve; });
    s.active = active;
    active.timer = setTimeout(() => {
      active.cancelled = true;
      void this.cancel(s).catch(() => {});
      this.finish(s, { error: new Error("ZCode turn timed out") });
    }, this.turnTimeoutMs);
    try {
      s.state.markInput();
      active.sending = s.client.request("session/send", { sessionId: s.id, content, inputId: active.inputId });
      await active.sending;
      const result = await done;
      if (result.error) throw result.error;
      return result;
    } catch (error) { this.finish(s, { error }); throw error; }
  }

  cancel(s) {
    if (s.cancelling) return s.cancelling;
    if (!s.active) return Promise.resolve();
    const operation = this.stopTurn(s);
    s.cancelling = operation;
    operation.then(() => { s.cancelling = null; }, () => { s.cancelling = null; });
    return operation;
  }

  async stopTurn(s) {
    const active = s.active;
    if (!active) return;
    active.cancelled = true;
    active.abort.abort();
    await active.sending?.catch(() => {});
    try { await s.client.request("session/stop", { sessionId: s.id }); }
    catch (error) { this.finish(s, { error }); await s.client.close(); throw error; }
    if (s.active === active) this.finish(s, { stopReason: "cancelled" });
  }

  async end(s) {
    if (s.closing) return s.closing;
    s.closing = (async () => {
      try { await this.cancel(s); await s.client.request("session/close", { sessionId: s.id }); }
      finally { s.account.close(); await s.client.close(); this.sessions.delete(s.id); }
    })();
    return s.closing;
  }

  async handle(method, p = {}) {
    if (method === "initialize") return {
      protocolVersion: 1, agentInfo: { name: "zcode-cli-acp", title: "ZCode CLI (ACP)", version: "0.2.0" },
      agentCapabilities: { loadSession: true, promptCapabilities: { image: false, audio: false, embeddedContext: true }, mcpCapabilities: { http: true, sse: true } }, authMethods: [],
    };
    if (method === "session/new") return this.open(p);
    if (method === "session/load") return this.open(p, true);
    if (!["session/prompt", "session/cancel", "session/close", "session/set_mode", "session/set_model", "session/set_config_option"].includes(method)) throw new NativeProtocolError(`Method not found: ${method}`, -32601);
    const s = this.get(p.sessionId);
    if (method === "session/prompt") return this.prompt(s, p);
    if (method === "session/cancel") { await this.cancel(s); return {}; }
    if (method === "session/close") { await this.end(s); return {}; }
    if (s.active || s.closing || s.cancelling) throw invalid("Cannot change configuration during a turn or shutdown");
    let changed;
    if (method === "session/set_mode") {
      if (!modes.some(m => m.id === p.modeId)) throw invalid("Unknown mode");
      if (this.permissionMode === "accept-edits" && p.modeId === "yolo") throw invalid("Full access requires BB full permissions");
      changed = await s.client.request("session/setMode", { sessionId: s.id, mode: p.modeId });
    } else if (method === "session/set_model" || (method === "session/set_config_option" && p.configId === "model")) {
      const model = s.snapshot.settings.model.available.find(m => modelId(m.ref) === (p.modelId ?? p.value) && !m.disabledReason);
      if (!model) throw invalid("Unknown model");
      changed = await s.client.request("session/setModel", { sessionId: s.id, model: selectionFor(model) });
    } else if (method === "session/set_config_option" && p.configId === "effort") {
      const effort = reasoning(s);
      if (!effort.levels.some(e => e.value === p.value)) throw invalid("Unknown reasoning level");
      if (effort.modelSelection) changed = await s.client.request("session/setModel", { sessionId: s.id, model: { ...s.snapshot.settings.model.current, options: { reasoningLevel: p.value } } });
      else changed = await s.client.request("session/setThoughtLevel", { sessionId: s.id, thoughtLevel: p.value });
    } else throw new NativeProtocolError(`Method not found: ${method}`, -32601);
    await this.acceptSnapshot(s, changed);
    s.state.savePreferences(s.snapshot);
    return method === "session/set_config_option" ? { configOptions: this.state(s).configOptions } : {};
  }

  async close() {
    this.closed = true;
    await Promise.allSettled([...this.opening]);
    await Promise.allSettled([...this.sessions.values()].map(s => this.end(s)));
  }
}

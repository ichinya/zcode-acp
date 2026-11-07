import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { record, text } from "./account-schema.mjs";
import { workspaceRef } from "./native-client.mjs";

const error = message => new Error(`ZCode Start Plan: ${message}`);
const seconds = value => (typeof value === "number" || typeof value === "string" && value.trim()) && Number.isFinite(Number(value)) && Number(value) > 0 ? Number(value) * 1000 : undefined;
const positive = value => (typeof value === "number" || typeof value === "string" && value.trim()) && Number.isFinite(Number(value)) && Number(value) > 0;
const printable = value => typeof value === "string" && /^[\x20-\x7e]+$/.test(value.trim()) ? value.trim() : undefined;

// Only currently usable, positive model balances grant access. The native
// catalog and the official balance response must agree on the model identity.
export function startEntitlement(data, nativeIds, now = Date.now(), serverDate) {
  const p = record(data);
  if (!Array.isArray(p.plans) || !Array.isArray(p.balances)) throw error("invalid balance response.");
  const server = Number.isFinite(serverDate) ? serverDate : seconds(p.server_time) || now;
  const ids = new Set(), boundaries = [];
  for (const raw of p.balances) {
    const b = record(raw);
    if (b.unit_type !== "token" || b.meter !== "model_usage" || !positive(b.available_units ?? b.remaining_units)) continue;
    const linked = p.plans.map(record).filter(plan => b.user_plan_id && plan.user_plan_id ? b.user_plan_id === plan.user_plan_id : b.plan_id && b.plan_id === plan.plan_id);
    for (const plan of linked) {
      if (text(plan.status).trim().toLowerCase() !== "active") continue;
      const entitlement = (Array.isArray(plan.entitlements) ? plan.entitlements : []).map(record).find(e => b.entitlement_id && b.entitlement_id === e.entitlement_id);
      const starts = [plan.starts_at, b.period_start, b.effective_at, entitlement?.effective_at].map(seconds).filter(Boolean);
      const ends = [plan.ends_at, b.period_end, b.expires_at].map(seconds).filter(Boolean);
      if (starts.some(t => t > server) || ends.some(t => t <= server)) continue;
      const capabilities = Array.isArray(b.capabilities) ? b.capabilities.filter(c => typeof c === "string" && c.toLowerCase().startsWith("model:")).map(c => c.slice(6).trim()) : [];
      const requested = capabilities.length ? capabilities : [text(b.show_name).trim()];
      for (const id of nativeIds) if (requested.some(value => value.toLowerCase() === id.toLowerCase())) ids.add(id);
      boundaries.push(...ends);
    }
  }
  return { modelIds: [...ids], validUntil: now + Math.max(0, Math.min(60_000, ...boundaries.map(end => end - server))) };
}

export class StartPlanHost {
  constructor({ env, root, readLogin, fetcher = fetch, now = Date.now }) {
    Object.assign(this, { env, root, readLogin, fetcher, now });
    this.controller = new AbortController();
  }
  async current() {
    if (this.controller.signal.aborted) throw error("session closed.");
    const current = await this.readLogin();
    if (!current?.zcodeJwt || current.fingerprint !== this.login?.fingerprint) { this.close(); throw error("desktop sign-in changed. Start a new BB session."); }
    if (this.controller.signal.aborted) throw error("session closed.");
    return current;
  }
  async balance() {
    const login = await this.current();
    let telemetry;
    try { telemetry = JSON.parse(await fs.readFile(path.join(this.root, "telemetry-state.json"), "utf8")); } catch { throw error("Desktop device identity is unavailable. Open ZCode once."); }
    const mid = printable(telemetry.deviceMid), version = printable(this.env.ZCODE_APP_VERSION);
    if (!mid || !version) throw error("Desktop device identity or version is unavailable.");
    let locale = {}; try { locale = Intl.DateTimeFormat().resolvedOptions(); } catch {}
    try {
      const response = await this.fetcher(`https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version=${encodeURIComponent(version)}`, {
        headers: { Authorization: `Bearer ${login.zcodeJwt}`, "X-Device-Mid": mid, "X-ZCode-App-Version": version,
          "User-Agent": `ZCode/${version}`, "HTTP-Referer": "https://zcode.z.ai", "X-Title": "Z Code@electron",
          "X-Platform": `${process.platform}-${process.arch}`, "X-Release-Channel": "stable",
          "X-Client-Language": printable(locale.locale) || "unknown", "X-Client-Timezone": printable(locale.timeZone) || "unknown",
          "X-Os-Category": process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : "linux", "X-Os-Version": printable(os.version()) || "unknown" },
        redirect: "error", signal: AbortSignal.any([this.controller.signal, AbortSignal.timeout(15_000)]),
      });
      if (!response.ok) { await response.body?.cancel(); throw error(`balance service returned HTTP ${response.status}. Reconnect in Desktop.`); }
      const reader = response.body?.getReader(); if (!reader) throw new Error();
      let size = 0; const chunks = [];
      try { for (;;) { const { done, value } = await reader.read(); if (done) break; size += value.length; if (size > 1024 * 1024) throw new Error(); chunks.push(value); } } finally { await reader.cancel(); }
      const envelope = record(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      if (envelope.success === false || ![0, 200, "0", "200"].includes(envelope.code)) throw new Error();
      await this.current();
      const serverDate = Date.parse(response.headers.get("date") || "");
      this.entitlement = startEntitlement(envelope.data, this.provider.config.builtinModelIds, this.now(), serverDate);
      this.balanceSnapshot = { data: envelope.data, serverDate };
      return this.entitlement;
    } catch (e) {
      // Network stacks and response bodies may contain credentials; never relay.
      if (e.message?.startsWith("ZCode Start Plan:")) throw e;
      throw error("balance unavailable or sign-in rejected. Check Desktop and retry.");
    }
  }
  async initialize(builtin) {
    this.login = await this.readLogin();
    if (!this.login?.zcodeJwt) return;
    this.provider = (builtin.config?.providerConfigRules?.providerRules || []).find(p => p.providerId === `account:${this.login.family}-start-plan` && p.config?.access?.type === "zhipu-account" && p.config.access.mode === "start-plan" && p.config.access.accountType === this.login.family && Array.isArray(p.config.builtinModelIds) && p.config.builtinModelIds.every(id => typeof id === "string" && id));
    if (!this.provider) return;
    await this.balance();
    return this.entitlement.modelIds.length ? this.provider.providerId : undefined;
  }
  async authorize(raw, sessionId, cwd) {
    try {
      const p = record(raw), access = record(p.accountAccess), model = record(p.modelSelection), workspace = record(p.workspace), expected = workspaceRef(cwd);
      if (!this.provider || p.sessionId !== sessionId || p.providerId !== this.provider.providerId || model.providerId !== p.providerId || access.type !== "zhipu-account" || access.mode !== "start-plan" || access.accountType !== this.login?.family || access.entitled !== true || p.reason !== "model-request" || !text(p.requestId) || workspace.workspaceKey !== expected.workspaceKey || workspace.workspacePath !== expected.workspacePath || p.expectedAccountScope !== undefined || p.rejectedProjectTokenFingerprint !== undefined) throw error("mismatched account, plan, session or workspace.");
      const login = await this.current();
      if (!this.entitlement || this.entitlement.validUntil <= this.now()) await this.balance();
      if (!this.entitlement.modelIds.includes(model.modelId)) throw error("model balance is unavailable, expired or exhausted.");
      return { headersApplied: true, requestAuth: { apiKey: login.zcodeJwt } };
    } catch (e) { return { headersApplied: false, errorMessage: e.message?.startsWith("ZCode Start Plan:") ? e.message : "ZCode Start Plan authentication failed." }; }
  }
  close() { this.controller.abort(); this.login = undefined; this.entitlement = undefined; this.balanceSnapshot = undefined; }
}

import fs from "node:fs/promises";
import { CodingPlanHost } from "./coding-plan.mjs";
import { StartPlanHost } from "./start-plan.mjs";
import { findNativeEntry, nativeEnvironment } from "./native-client.mjs";
import { record } from "./account-schema.mjs";

const base = planLabel => ({ plan: null, accountEmail: null, planLabel });
const failed = planLabel => ({ status: "error", ...base(planLabel), message: "Квоты недоступны. Проверьте вход в ZCode и повторите обновление." });
const numeric = value => (typeof value === "number" || typeof value === "string" && value.trim()) && Number.isFinite(Number(value)) && Number(value) >= 0 ? Number(value) : undefined;
const epoch = value => { const n = numeric(value); return n && n * 1000; };
const iso = value => { const date = new Date(value); return Number.isFinite(date.getTime()) ? date.toISOString() : null; };
const amount = value => value.toLocaleString("ru-RU", { maximumFractionDigits: 0 });

export function codingQuotaUsage(data, planLabel = "Coding Plan") {
  if (!Array.isArray(data?.limits)) return failed(planLabel);
  const windows = [], seen = new Set();
  for (const row of data.limits.map(record)) {
    const model = ["TOKENS_LIMIT", "CREDIT_LIMIT"].includes(row.type);
    const spec = model && row.unit === 3 && row.number === 5 ? ["five-hour", "model-5h", "Модели · 5 часов"]
      : model && row.unit === 6 ? ["weekly", "model-week", "Модели · неделя"]
      : row.type === "TIME_LIMIT" && row.unit === 5 && row.number === 1 ? ["custom", "tools-month", "MCP/tools · месяц"] : null;
    if (!spec || seen.has(spec[1])) continue;
    if (typeof row.percentage !== "number" || !Number.isFinite(row.percentage) || row.percentage < 0) return failed(planLabel);
    const resetsAt = row.nextResetTime == null ? null : typeof row.nextResetTime === "number" && row.nextResetTime > 0 ? iso(row.nextResetTime) : null;
    if (row.nextResetTime != null && row.nextResetTime > 0 && !resetsAt) return failed(planLabel);
    windows.push({ kind: spec[0], id: spec[1], label: spec[2], usedPercent: row.percentage, resetsAt, model: null, cost: null });
    seen.add(spec[1]);
  }
  const order = ["model-5h", "model-week", "tools-month"];
  windows.sort((a, b) => order.indexOf(a.id) - order.indexOf(b.id));
  return windows.length ? { status: "ok", ...base(planLabel), windows } : failed(planLabel);
}

export function startQuotaUsage(data, now = Date.now(), serverDate) {
  if (!Array.isArray(data?.plans) || !Array.isArray(data?.balances)) return failed("Start Plan");
  const server = Number.isFinite(serverDate) ? serverDate : epoch(data.server_time) || now;
  const windows = []; let expired = false, pending = false;
  for (const [index, row] of data.balances.map(record).entries()) {
    const plans = data.plans.map(record).filter(p => row.user_plan_id && p.user_plan_id ? row.user_plan_id === p.user_plan_id : row.plan_id && row.plan_id === p.plan_id);
    if (!plans.length) continue;
    const active = plans.filter(p => String(p.status).toLowerCase() === "active");
    if (!active.length) { expired ||= plans.some(p => String(p.status).toLowerCase() === "expired"); continue; }
    const starts = [row.period_start, row.effective_at, ...active.flatMap(p => [p.starts_at, ...(p.entitlements || []).filter(e => e.entitlement_id === row.entitlement_id).map(e => e.effective_at)])].map(epoch).filter(Boolean);
    const ends = [row.period_end, row.expires_at, ...active.map(p => p.ends_at)].map(epoch).filter(Boolean);
    if (ends.some(t => t <= server)) { expired = true; continue; }
    if (starts.some(t => t > server)) { pending = true; continue; }
    if (row.unit_type !== "token" || row.meter !== "model_usage") continue;
    const total = numeric(row.total_units), used = numeric(row.used_units), remaining = numeric(row.remaining_units), available = numeric(row.available_units ?? row.remaining_units);
    if (!total || used === undefined || remaining === undefined || available === undefined || used > total || remaining > total || available > total) return failed("Start Plan");
    const models = (Array.isArray(row.capabilities) ? row.capabilities : []).filter(c => typeof c === "string" && c.toLowerCase().startsWith("model:")).map(c => c.slice(6));
    const deadline = ends.length ? Math.min(...ends) : undefined;
    const expiry = deadline ? ` · до ${new Date(deadline).toISOString().slice(0, 16).replace("T", " ")} UTC` : "";
    windows.push({ kind: "custom", id: `start-${index}`, label: `${amount(available)} / ${amount(total)} токенов${models.length ? ` · ${models.join(", ")}` : ""}${expiry}`, usedPercent: Math.round(used / total * 10000) / 100, resetsAt: deadline ? iso(deadline) : null, model: models.join(", ") || null, cost: null });
  }
  return { status: "ok", ...base(windows.length ? "Start Plan" : pending ? "Start Plan · ещё не активен" : expired ? "Start Plan · срок истёк" : "Start Plan · нет активного баланса"), windows };
}

// Collect exactly one plan. No ACP server, inference, claim or connectivity test.
/** @param {string} plan
 * @param {{env?: NodeJS.ProcessEnv, fetcher?: typeof fetch, now?: () => number, signal?: AbortSignal}} options */
export async function collectPlanUsage(plan, { env = process.env, fetcher = fetch, now = Date.now, signal } = {}) {
  if (!["coding", "start"].includes(plan)) throw new Error("Unknown quota source");
  const label = plan === "coding" ? "Coding Plan" : "Start Plan";
  let native;
  try { native = nativeEnvironment(findNativeEntry({ env }), env); } catch { return { status: "not_installed", ...base(label) }; }
  const coding = new CodingPlanHost({ env: native }, { fetch: fetcher, now });
  const start = new StartPlanHost({ env: native, root: coding.root, readLogin: () => coding.readLogin(), fetcher, now });
  const close = () => { coding.close(); start.close(); };
  signal?.addEventListener("abort", close, { once: true });
  try {
    if (signal?.aborted) return failed(label);
    const login = await coding.readLogin();
    if (!login || plan === "coding" && !login.token || plan === "start" && !login.zcodeJwt) return { status: "unauthenticated", ...base(label) };
    if (plan === "coding") {
      await coding.initialize({ request: async () => ({}) });
      if (!coding.provider) return failed(label);
      const material = await coding.getMaterial();
      const data = await coding.request("/api/monitor/usage/quota/limit", material.token);
      return codingQuotaUsage(data, coding.planLabel || label);
    }
    const builtin = JSON.parse(await fs.readFile(native.ZCODE_BUILTIN_PROVIDER_CONFIG_FILE, "utf8"));
    await start.initialize(builtin);
    return start.balanceSnapshot ? startQuotaUsage(start.balanceSnapshot.data, now(), start.balanceSnapshot.serverDate) : failed(label);
  } catch { return failed(label); }
  finally { signal?.removeEventListener("abort", close); close(); }
}

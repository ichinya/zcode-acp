import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { codingQuotaUsage, startQuotaUsage, collectPlanUsage } from "../lib/quota-usage.mjs";

test("Coding quotas use reported percentages and deadlines, never window duration as a token denominator", () => {
  const result = codingQuotaUsage({ limits: [
    { type: "TIME_LIMIT", unit: 5, number: 1, percentage: 45 },
    { type: "TOKENS_LIMIT", unit: 6, number: 1, percentage: 28 },
    { type: "CREDIT_LIMIT", unit: 3, number: 5, percentage: 1, nextResetTime: 1794030000000 },
  ] });
  assert.equal(result.status, "ok");
  assert.deepEqual(result.windows.map(w => w.usedPercent), [1, 28, 45]);
  assert.deepEqual(result.windows.map(w => w.kind), ["five-hour", "weekly", "custom"]);
  assert.equal(result.windows[0].resetsAt, new Date(1794030000000).toISOString());
  assert.equal(codingQuotaUsage({ limits: [{ type: "TOKENS_LIMIT", unit: 3, number: 5, percentage: "1" }] }).status, "error");
  assert.equal(codingQuotaUsage({ limits: [] }).status, "error");
});

function startData() { return { server_time: 100, plans: [{ user_plan_id: "plan", status: "active", starts_at: 80, ends_at: 200, entitlements: [{ entitlement_id: "grant", effective_at: 90 }] }], balances: [{ bucket_id: "bucket", user_plan_id: "plan", entitlement_id: "grant", meter: "model_usage", unit_type: "token", total_units: "500", used_units: "85", remaining_units: "415", available_units: "410", capabilities: ["model:glm"], expires_at: 180 }] }; }
test("Start shows available tokens, actual consumption and expiry without inventing a renewal", () => {
  const result = startQuotaUsage(startData(), 100000);
  assert.equal(result.status, "ok"); assert.match(result.windows[0].label, /410 \/ 500 токенов/);
  assert.equal(result.windows[0].usedPercent, 17); assert.equal(result.windows[0].resetsAt, new Date(180000).toISOString());
  const exhausted = startData(); exhausted.balances[0].used_units = 500; exhausted.balances[0].remaining_units = 0; exhausted.balances[0].available_units = 0;
  assert.equal(startQuotaUsage(exhausted).windows[0].usedPercent, 100);
});
test("Start missing, expired, pending and invalid balances never fabricate zero usage", () => {
  assert.equal(startQuotaUsage({}).status, "error");
  assert.deepEqual(startQuotaUsage({ plans: [], balances: [] }).windows, []);
  const expired = startData(); expired.plans[0].ends_at = 99;
  assert.match(startQuotaUsage(expired).planLabel, /срок истёк/);
  const pending = startData(); pending.plans[0].entitlements[0].effective_at = 101;
  assert.match(startQuotaUsage(pending).planLabel, /ещё не активен/);
  const invalid = startData(); invalid.balances[0].used_units = "unknown";
  assert.equal(startQuotaUsage(invalid).status, "error");
});
test("single-plan collection contacts only that plan and emits no credentials", async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-quota-test-")); t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const store = path.join(root, "v2"); fs.mkdirSync(store);
  fs.writeFileSync(path.join(store, "credentials.json"), JSON.stringify({ "oauth:active_provider": "zai", "oauth:zai:access_token": "private-oauth", zcodejwttoken: "private-jwt" }));
  fs.writeFileSync(path.join(store, "telemetry-state.json"), JSON.stringify({ deviceMid: "test-device" }));
  const builtin = path.join(root, "builtin.json");
  fs.writeFileSync(builtin, JSON.stringify({ revision: 1, config: { providerConfigRules: { providerRules: ["individual-coding-plan", "start-plan"].map(mode => ({ providerId: `account:zai-${mode}`, config: { access: { type: "zhipu-account", mode, accountType: "zai" }, builtinModelIds: ["glm"] } })) } } }));
  const env = { ...process.env, ZCODE_NATIVE_ENTRY: builtin, ZCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtin, ZCODE_STORAGE_DIR: root, ZCODE_APP_VERSION: "3.17.2" };
  const fetcher = async url => {
    const route = new URL(url); let data;
    if (route.origin === "https://zcode.z.ai") { assert.equal(expected, "start"); return Response.json({ code: 0, data: startData() }); }
    assert.equal(expected, "coding");
    if (route.pathname.endsWith("getCustomerInfo")) data = { organizations: [{ organizationId: "org", projects: [{ projectId: "project" }] }] };
    else if (route.pathname.endsWith("/api_keys")) data = [{ name: "zcode-api-key", apiKey: "key" }];
    else if (route.pathname.endsWith("/access_tokens")) data = { accessToken: "private-project", tokenType: "Bearer", expiresIn: 3600, expiresAt: 3700 };
    else if (route.pathname.endsWith("/subscription/list")) data = [{ productName: "Coding Test", status: "VALID", inCurrentPeriod: true }];
    else if (route.pathname.endsWith("/quota/limit")) data = { limits: [{ type: "TOKENS_LIMIT", unit: 3, number: 5, percentage: 1 }] };
    else assert.fail("Unexpected endpoint");
    return Response.json({ code: 200, data });
  };
  let expected;
  for (const plan of ["coding", "start"]) { expected = plan; const result = await collectPlanUsage(plan, { env, fetcher, now: () => 100000 }); assert.equal(result.status, "ok"); assert.doesNotMatch(JSON.stringify(result), /private-/); }
  const result = await collectPlanUsage("start", { env, fetcher: async () => new Response("private-jwt", { status: 401 }) });
  assert.equal(result.status, "error"); assert.doesNotMatch(JSON.stringify(result), /private-/);
});

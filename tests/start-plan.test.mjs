import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { NativeAccountHost } from "../lib/native-account.mjs";
import { startEntitlement } from "../lib/start-plan.mjs";

const now = 1_000_000_000;
function balance() { return { server_time: now / 1000, plans: [{ user_plan_id: "plan", status: "active", starts_at: now / 1000 - 100, ends_at: now / 1000 + 100, entitlements: [{ entitlement_id: "grant", effective_at: now / 1000 - 50 }] }], balances: [{ user_plan_id: "plan", entitlement_id: "grant", unit_type: "token", meter: "model_usage", available_units: "12", capabilities: ["model:glm-flash"], expires_at: now / 1000 + 90 }] }; }
function fixture(t, { family = "zai", codingRejected = false, jwtOnly = false, telemetry = true } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-start-test-"));
  const accountRoot = path.join(root, "v2"); fs.mkdirSync(accountRoot);
  const credentials = path.join(accountRoot, "credentials.json"), settings = path.join(accountRoot, "setting.json");
  const saveLogin = (jwt = "fixture-desktop-jwt") => fs.writeFileSync(credentials, JSON.stringify({ "oauth:active_provider": family, ...(jwtOnly ? {} : { [`oauth:${family}:access_token`]: "fixture-oauth" }), zcodejwttoken: jwt }));
  saveLogin(); fs.writeFileSync(settings, JSON.stringify({ providerFamilyConnectionSelections: { [family]: { kind: "start-plan" } } }));
  if (telemetry) fs.writeFileSync(path.join(accountRoot, "telemetry-state.json"), JSON.stringify({ deviceMid: "fixture-device" }));
  const builtin = path.join(root, "builtin.json"), codingId = `account:${family}-individual-coding-plan`, startId = `account:${family}-start-plan`;
  fs.writeFileSync(builtin, JSON.stringify({ revision: 1, config: { providerConfigRules: { providerRules: [
    { providerId: codingId, config: { access: { type: "zhipu-account", mode: "individual-coding-plan", accountType: family }, builtinModelIds: ["GLM-Flash"] } },
    { providerId: startId, config: { access: { type: "zhipu-account", mode: "start-plan", accountType: family }, builtinModelIds: ["GLM-Flash", "not-granted"] } },
  ] } } }));
  const requests = [], overlays = []; let currentTime = now, data = balance(), failStart = false;
  const host = new NativeAccountHost({ env: { ZCODE_STORAGE_DIR: root, ZCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtin, ZCODE_APP_VERSION: "3.17.2" } }, {
    now: () => currentTime, fetch: async (url, options) => {
      requests.push({ url, options }); assert.equal(options.redirect, "error");
      const parsed = new URL(url); let result;
      if (parsed.origin === "https://zcode.z.ai") {
        assert.equal(parsed.pathname, "/api/v1/zcode-plan/billing/balance");
        assert.equal(options.headers.Authorization, "Bearer fixture-desktop-jwt");
        assert.equal(options.headers["X-Device-Mid"], "fixture-device");
        if (failStart) return new Response("secret fixture-desktop-jwt", { status: 401 });
        return Response.json({ code: 0, data });
      }
      assert.equal(parsed.origin, family === "zai" ? "https://api.z.ai" : "https://bigmodel.cn");
      if (codingRejected) return new Response("sensitive fixture-oauth", { status: 403 });
      if (parsed.pathname.endsWith("getCustomerInfo")) result = { organizations: [{ organizationId: "org", projects: [{ projectId: "project" }] }] };
      else if (parsed.pathname.endsWith("/api_keys")) result = [{ name: "zcode-api-key", apiKey: "key-id" }];
      else if (parsed.pathname.endsWith("/access_tokens")) result = { accessToken: "fixture-project-token", tokenType: "Bearer", expiresIn: 3600, expiresAt: currentTime / 1000 + 3600 };
      else if (parsed.pathname.endsWith("/subscription/list")) result = [{ productName: "Coding Plan", status: "VALID", inCurrentPeriod: true }];
      else throw new Error("Unexpected account endpoint");
      return Response.json({ code: 200, data: result });
    },
  });
  t.after(() => { host.close(); fs.rmSync(root, { recursive: true, force: true }); });
  const client = { request: async (method, params) => { overlays.push({ method, params }); return {}; } };
  const params = { requestId: "fixture-request", reason: "model-request", sessionId: "own-session", providerId: startId, accountAccess: { type: "zhipu-account", mode: "start-plan", accountType: family, entitled: true }, modelSelection: { providerId: startId, modelId: "GLM-Flash" }, workspace: { workspacePath: root, workspaceKey: root } };
  return { host, client, params, root, credentials, settings, requests, overlays, startId, codingId, saveLogin, setData: value => { data = value; }, advance: ms => { currentTime += ms; }, failStart: () => { failStart = true; } };
}

for (const family of ["zai", "bigmodel"]) test(`${family} exposes Coding and Start independently of Desktop selection with separate credentials`, async t => {
  const d = fixture(t, { family }); const before = [d.credentials, d.settings].map(p => fs.readFileSync(p));
  await d.host.initialize(d.client);
  assert.equal(d.overlays.length, 1);
  assert.deepEqual(Object.keys(d.overlays[0].params.providers), [d.codingId, d.startId]);
  assert.deepEqual(d.overlays[0].params.providers[d.startId].builtinModelIds, ["GLM-Flash"]);
  assert.equal((await d.host.authorize(d.params, "own-session", d.root)).requestAuth.apiKey, "fixture-desktop-jwt");
  const coding = { ...d.params, providerId: d.codingId, accountAccess: { ...d.params.accountAccess, mode: "individual-coding-plan" }, modelSelection: { providerId: d.codingId, modelId: "GLM-Flash" } };
  assert.equal((await d.host.authorize(coding, "own-session", d.root)).requestAuth.apiKey, "fixture-project-token");
  [d.credentials, d.settings].forEach((p, i) => assert.deepEqual(fs.readFileSync(p), before[i]));
});

test("Start remains usable when Coding Plan is rejected or only ZCode JWT is saved", async t => {
  for (const options of [{ codingRejected: true }, { jwtOnly: true }]) {
    const d = fixture(t, options); await d.host.initialize(d.client);
    assert.equal(d.host.codingReady, false); assert.equal(d.host.startId, d.startId);
    assert.equal((await d.host.authorize(d.params, "own-session", d.root)).headersApplied, true);
    assert.deepEqual(Object.keys(d.overlays[0].params.providers), [d.startId]);
  }
});

test("Start credentials are denied for another plan, model, family, session, workspace or project-token scope", async t => {
  const d = fixture(t); await d.host.initialize(d.client);
  for (const patch of [{ sessionId: "other" }, { providerId: "other" }, { modelSelection: { providerId: d.startId, modelId: "not-granted" } }, { workspace: { workspacePath: d.root, workspaceKey: "other" } }, { accountAccess: { ...d.params.accountAccess, mode: "individual-coding-plan" } }, { accountAccess: { ...d.params.accountAccess, accountType: "bigmodel" } }, { expectedAccountScope: "scope" }, { rejectedProjectTokenFingerprint: "fingerprint" }]) {
    const result = await d.host.authorize({ ...d.params, ...patch }, "own-session", d.root);
    assert.equal(result.headersApplied, false); assert.equal(result.requestAuth, undefined);
  }
});

test("Start denies exhausted/expired/future balances, unknown models and disconnected buckets", () => {
  assert.deepEqual(startEntitlement(balance(), ["GLM-Flash"], now).modelIds, ["GLM-Flash"]);
  for (const change of [d => { d.balances[0].available_units = 0; }, d => { d.plans[0].status = "expired"; }, d => { d.balances[0].expires_at = now / 1000; }, d => { d.plans[0].starts_at = now / 1000 + 1; }, d => { d.plans[0].entitlements[0].effective_at = now / 1000 + 1; }, d => { d.balances[0].capabilities = ["model:other"]; }, d => { d.balances[0].user_plan_id = "other"; }, d => { delete d.balances[0].available_units; }]) {
    const data = balance(); change(data); assert.deepEqual(startEntitlement(data, ["GLM-Flash"], now).modelIds, []);
  }
  assert.equal(startEntitlement(balance(), ["GLM-Flash"], now, now + 200_000).modelIds.length, 0);
});

test("Start checks fresh balance at expiry, detects logout, and never leaks response bodies", async t => {
  const d = fixture(t); await d.host.initialize(d.client);
  d.advance(61_000); const data = balance(); data.balances[0].available_units = 0; d.setData(data);
  assert.equal((await d.host.authorize(d.params, "own-session", d.root)).headersApplied, false);
  d.failStart(); d.advance(61_000);
  const failed = await d.host.authorize(d.params, "own-session", d.root);
  assert.equal(failed.headersApplied, false); assert.doesNotMatch(failed.errorMessage, /fixture|secret/);
  d.saveLogin("different-jwt");
  const changed = await d.host.authorize(d.params, "own-session", d.root);
  assert.equal(changed.headersApplied, false); assert.match(changed.errorMessage, /sign-in changed/);
});

test("missing Desktop device identity never enables Start and does not block Coding", async t => {
  const d = fixture(t, { telemetry: false }); await d.host.initialize(d.client);
  assert.equal(d.host.startId, undefined); assert.equal(d.host.codingReady, true);
  assert.deepEqual(Object.keys(d.overlays[0].params.providers), [d.codingId]);
});

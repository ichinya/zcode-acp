import assert from "node:assert/strict";
import { createCipheriv } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { CodingPlanHost } from "../lib/coding-plan.mjs";
import { createHash } from "node:crypto";

function fixture(t, { family = "zai", serviceError = false } = {}) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-account-test-"));
  assert.equal(path.dirname(path.resolve(home)), path.resolve(os.tmpdir()));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));
  const root = path.join(home, ".zcode", "v2"); fs.mkdirSync(root, { recursive: true });
  const credentialFile = path.join(root, "credentials.json");
  const secret = "synthetic-account-cipher";
  const key = createHash("sha256").update(secret).digest();
  function encrypt(text) {
    const iv = Buffer.alloc(12, 1), cipher = createCipheriv("aes-256-gcm", key, iv);
    const result = Buffer.concat([cipher.update(text), cipher.final()]);
    return "enc:v1:" + [iv, cipher.getAuthTag(), result].map(b => b.toString("base64url")).join(".");
  }
  const saveLogin = (token = "fixture-oauth") => fs.writeFileSync(credentialFile, JSON.stringify({
    "oauth:active_provider": encrypt(family), [`oauth:${family}:access_token`]: encrypt(token),
    [`oauth:${family}:user_info`]: encrypt("fixture-identity"), zcodejwttoken: encrypt("fixture-desktop-jwt"),
  }));
  saveLogin();
  fs.writeFileSync(path.join(root, "setting.json"), JSON.stringify({ providerFamilyConnectionSelections: { [family]: { kind: "individual-coding-plan" } } }));
  const providerId = `account:${family}-individual-coding-plan`;
  const builtin = path.join(home, "builtin.json");
  fs.writeFileSync(builtin, JSON.stringify({ revision: "fixture-revision", config: { providerConfigRules: { providerRules: [{ providerId, config: { access: { type: "zhipu-account", mode: "individual-coding-plan", accountType: family }, builtinModelIds: ["fixture-model"] } }] } } }));
  const env = { HOME: home, ZCODE_STORAGE_DIR: path.dirname(root), ZCODE_CREDENTIAL_SECRET: secret, ZCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtin, ZCODE_APP_VERSION: "3.17.2" };
  let now = 1_000_000, tokenCount = 0;
  const requests = [], overlays = [];
  const host = new CodingPlanHost({ env }, { now: () => now, fetch: async (url, options) => {
    requests.push({ url, options });
    assert.equal(new URL(url).origin, family === "zai" ? "https://api.z.ai" : "https://bigmodel.cn");
    assert.equal(options.redirect, "error");
    if (serviceError) return new Response("sensitive-response-must-not-escape", { status: 401 });
    const route = new URL(url).pathname;
    let data;
    if (route.endsWith("getCustomerInfo")) data = { organizations: [{ organizationId: "fixture-org", projects: [{ projectId: "fixture-project" }] }] };
    else if (route.endsWith("/api_keys")) data = [{ name: "zcode-api-key", apiKey: "fixture-key-id" }];
    else if (route.endsWith("/access_tokens")) { tokenCount++; data = { accessToken: `fixture-project-token-${tokenCount}`, tokenType: "Bearer", expiresIn: 3600, expiresAt: now / 1000 + 3600, enable: true }; }
    else if (route.endsWith("/subscription/list")) data = [{ productName: "Coding fixture", status: "VALID", inCurrentPeriod: true }];
    else throw new Error("Unexpected fixture endpoint");
    return Response.json({ code: 200, data });
  } });
  t.after(() => host.close());
  const client = { request: async (method, params) => { overlays.push({ method, params }); return {}; } };
  const params = { sessionId: "owned-session", providerId, accountAccess: { type: "zhipu-account", mode: "individual-coding-plan", accountType: family }, modelSelection: { providerId, modelId: "fixture-model" }, reason: "model-request" };
  return { host, client, params, requests, overlays, root, credentialFile, saveLogin, advance: ms => { now += ms; } };
}

for (const family of ["zai", "bigmodel"]) test(`${family} Desktop login supplies a scoped temporary token and account overlay`, async t => {
  const d = fixture(t, { family });
  const before = fs.readFileSync(d.credentialFile);
  await d.host.initialize(d.client);
  assert.equal(d.requests.length, 4);
  assert.equal(d.overlays[0].method, "provider/updateAccountConfig");
  assert.match(d.overlays[0].params.basedOnZCodeBuiltinRevision, /^zcode-builtin:fixture-revision:/);
  const result = await d.host.authorize(d.params, "owned-session");
  assert.equal(result.headersApplied, true);
  assert.equal(result.requestAuth.apiKey, "fixture-project-token-1");
  assert.equal(d.requests.length, 4);
  assert.deepEqual(fs.readFileSync(d.credentialFile), before);
});

test("temporary tokens refresh without rewriting Desktop credentials", async t => {
  const d = fixture(t); await d.host.initialize(d.client); d.advance(6 * 60_000);
  const result = await d.host.authorize(d.params, "owned-session");
  assert.equal(result.requestAuth.apiKey, "fixture-project-token-2");
});

test("wrong session, provider, model and account scopes never receive a token", async t => {
  const d = fixture(t); await d.host.initialize(d.client);
  for (const patch of [{ sessionId: "other" }, { providerId: "other" }, { modelSelection: { providerId: d.params.providerId, modelId: "not-entitled" } }, { expectedAccountScope: "wrong-scope" }]) {
    const result = await d.host.authorize({ ...d.params, ...patch }, "owned-session");
    assert.equal(result.headersApplied, false);
    assert.equal(result.requestAuth, undefined);
  }
  assert.equal(d.requests.length, 4);
});

test("changed Desktop sign-in invalidates the running account host", async t => {
  const d = fixture(t); await d.host.initialize(d.client); d.saveLogin("different-fixture-oauth");
  const result = await d.host.authorize(d.params, "owned-session");
  assert.equal(result.headersApplied, false); assert.match(result.errorMessage, /sign-in changed/);
});

test("account service errors never expose response bodies or account credentials", async t => {
  const d = fixture(t, { serviceError: true });
  await assert.rejects(d.host.initialize(d.client), error => /HTTP 401/.test(error.message) && !/sensitive-response|fixture-oauth/.test(error.message));
});

test("corrupt encrypted credentials produce a safe error", async t => {
  const d = fixture(t);
  fs.writeFileSync(d.credentialFile, JSON.stringify({ "oauth:active_provider": "enc:v1:private-sentinel" }));
  await assert.rejects(d.host.initialize(d.client), error => /cannot decrypt/.test(error.message) && !error.message.includes("private-sentinel"));
});

test("official MCP account headers are restricted to the official origin and bound workspace", async t => {
  const d = fixture(t); await d.host.initialize(d.client);
  const base = { requestId: "fixture-request", pluginId: "fixture-plugin", mcpKey: "fixture-server", workspace: { workspacePath: d.root, workspaceKey: d.root } };
  assert.equal((await d.host.authorizeOfficialMcp({ ...base, targetOrigin: "https://attacker.example" }, d.root)).ok, false);
  assert.equal((await d.host.authorizeOfficialMcp({ ...base, targetOrigin: "https://zcode.z.ai", workspace: { ...base.workspace, workspaceKey: "wrong" } }, d.root)).ok, false);
  assert.equal((await d.host.authorizeOfficialMcp({ ...base, targetOrigin: "https://zcode.z.ai" }, d.root)).ok, true);
});

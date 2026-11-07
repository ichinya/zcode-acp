import fs from "node:fs/promises";
import path from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { CodingPlanHost } from "./coding-plan.mjs";
import { StartPlanHost } from "./start-plan.mjs";

// Independent overlays expose both plans in the same ACP session. No Desktop
// connection selection or credentials are written, and tokens remain in memory.
export class NativeAccountHost {
  constructor(launch, dependencies = {}) {
    this.launch = launch;
    this.coding = new CodingPlanHost(launch, dependencies);
    this.start = new StartPlanHost({ env: launch.env, root: this.coding.root, readLogin: () => this.coding.readLogin(), fetcher: dependencies.fetch, now: dependencies.now });
    this.diagnostics = [];
    this.codingReady = false;
  }
  get planLabel() { return this.codingReady ? this.coding.planLabel : null; }
  get provider() { return this.codingReady ? this.coding.provider : undefined; }
  async initialize(client) {
    const file = this.launch.env.ZCODE_BUILTIN_PROVIDER_CONFIG_FILE;
    if (!file) return;
    let builtin;
    try { builtin = JSON.parse(await fs.readFile(file, "utf8")); } catch { throw new Error("ZCode account catalog cannot be read."); }
    const providers = {}, states = {};
    try {
      await this.coding.initialize({ request: async (method, p) => {
        if (method !== "provider/updateAccountConfig") throw new Error("Unexpected account method");
        Object.assign(providers, p.providers); Object.assign(states, p.states);
      } });
      this.codingReady = Object.keys(providers).length > 0;
    } catch { this.diagnostics.push("Coding Plan could not be verified; reconnect it in ZCode Desktop."); }
    try {
      this.startId = await this.start.initialize(builtin);
      if (this.startId) {
        providers[this.startId] = { access: { type: "zhipu-account", entitled: true }, builtinModelIds: this.start.entitlement.modelIds };
        states[this.startId] = { availability: "available", entitled: true, current: true };
      }
    } catch { this.diagnostics.push("Start Plan balance could not be verified; check ZCode Desktop sign-in and device identity."); }
    if (Object.keys(providers).length) await client.request("provider/updateAccountConfig", {
      revision: `bb-accounts:${randomUUID()}`,
      basedOnZCodeBuiltinRevision: `zcode-builtin:${builtin.revision}:${createHash("sha256").update(path.resolve(file)).digest("hex")}`,
      providers, states,
    });
  }
  authorize(raw, sessionId, cwd) {
    if (this.startId && raw?.providerId === this.startId) return this.start.authorize(raw, sessionId, cwd);
    if (this.codingReady) return this.coding.authorize(raw, sessionId);
    return { headersApplied: false, errorMessage: "ZCode account source is unavailable; reconnect that plan in Desktop." };
  }
  authorizeOfficialMcp(raw, cwd) { return this.codingReady ? this.coding.authorizeOfficialMcp(raw, cwd) : { ok: false, reason: "official_auth_plan_required" }; }
  close() { this.start.close(); this.coding.close(); }
}

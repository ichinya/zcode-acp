import { NativeAcpAgent } from "./native-acp.mjs";

// Account/model discovery only. No session/send or connectivity test is used.
export async function nativeDoctor(cwd = process.cwd()) {
  const agent = new NativeAcpAgent({ update() {}, requestClient: async () => ({ outcome: { outcome: "cancelled" } }), env: { ...process.env, ZCODE_NATIVE_MODE: "plan" } });
  try {
    const result = await agent.handle("session/new", { cwd, mcpServers: [] });
    const session = agent.get(result.sessionId);
    const models = result.models?.availableModels || [];
    return {
      ok: models.length > 0,
      platform: process.platform,
      nodeVersion: process.version,
      desktopVersion: session.client.env.ZCODE_APP_VERSION || null,
      transport: "ACP -> ZCode app-server stdio",
      desktopCodingPlanVerified: Boolean(session.account.planLabel),
      planLabel: session.account.planLabel || null,
      startPlanVerified: Boolean(session.account.startId),
      accountDiagnostics: session.account.diagnostics || [],
      models: models.map(model => ({ id: model.modelId, name: model.name })),
      permissions: "plan",
      sessionStorage: "separate-from-desktop",
      modelRequestsSent: 0,
      generationVerified: false,
    };
  } finally { await agent.close(); }
}

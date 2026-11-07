// Narrow equivalents of the three Zod schemas used by the vendored account
// host. This keeps the standalone ACP agent dependency-free at runtime.
export const record = value => value && typeof value === "object" && !Array.isArray(value) ? value : {};
export const text = value => typeof value === "string" ? value : "";
const string = value => typeof value === "string" && value.length > 0;
const number = value => typeof value === "number" && Number.isFinite(value) && value > 0;
function schema(parse) {
  return { safeParse(value) { try { return { success: true, data: parse(value) }; } catch { return { success: false }; } } };
}
function access(value) {
  const p = record(value);
  if (p.type !== "zhipu-account" || p.mode !== "individual-coding-plan" || !["zai", "bigmodel"].includes(p.accountType)) throw new Error("Invalid account access");
  return { type: p.type, mode: p.mode, accountType: p.accountType };
}
export const accessSchema = schema(access);
export const providerSchema = schema(value => {
  const p = record(value), c = record(p.config);
  if (!string(p.providerId) || !Array.isArray(c.builtinModelIds) || !c.builtinModelIds.every(string)) throw new Error("Invalid provider");
  return { providerId: p.providerId, config: { access: access(c.access), builtinModelIds: [...c.builtinModelIds] } };
});
export const tokenSchema = schema(value => {
  const p = record(value);
  if ((p.enable !== undefined && typeof p.enable !== "boolean") || !text(p.accessToken).trim() || p.tokenType !== "Bearer" || !number(p.expiresIn) || !number(p.expiresAt)) throw new Error("Invalid temporary project token");
  return { ...(p.enable !== undefined ? { enable: p.enable } : {}), accessToken: p.accessToken.trim(), tokenType: "Bearer", expiresIn: p.expiresIn, expiresAt: p.expiresAt };
});

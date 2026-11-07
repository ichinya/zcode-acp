import { CodingPlanHost } from "./coding-plan.mjs";
import { findNativeEntry, nativeEnvironment } from "./native-client.mjs";

// A private in-memory cache scope, never a credential or an account identifier.
export async function getAccountFingerprint({ env = process.env } = {}) {
  let native;
  try { native = nativeEnvironment(findNativeEntry({ env }), env); } catch { return "not-installed"; }
  const reader = new CodingPlanHost({ env: native });
  try { return (await reader.readLogin())?.fingerprint || "signed-out"; }
  catch { return "unreadable"; }
  finally { reader.close(); }
}

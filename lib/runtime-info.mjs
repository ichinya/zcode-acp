import fs from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";

export function getRuntimeInfo() {
  const root = new URL("../", import.meta.url);
  const revision = createHash("sha256");
  for (const relative of ["package.json", "bin/zcode-acp.mjs", ...fs.readdirSync(new URL("lib/", root)).filter(name => name.endsWith(".mjs")).sort().map(name => `lib/${name}`), "native/protect-state.ps1"]) revision.update(fs.readFileSync(new URL(relative, root)));
  return { version: JSON.parse(fs.readFileSync(new URL("package.json", root), "utf8")).version, cliPath: fileURLToPath(new URL("bin/zcode-acp.mjs", root)), revision: revision.digest("hex").slice(0, 16) };
}

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

test("Windows session ACLs ignore incompatible inherited PowerShell modules", { skip: process.platform !== "win32" }, t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-state-modules-"));
  assert.equal(path.dirname(path.resolve(root)), path.resolve(os.tmpdir()));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const modules = path.join(root, "modules");
  const security = path.join(modules, "Microsoft.PowerShell.Security");
  fs.mkdirSync(security, { recursive: true });
  // Reproduce a PowerShell 7 module shadowing the Windows PowerShell module.
  fs.writeFileSync(path.join(security, "Microsoft.PowerShell.Security.psd1"), "@{ ModuleVersion = '1.0.0'; RootModule = 'incompatible.psm1'; FunctionsToExport = @('Set-Acl') }");
  fs.writeFileSync(path.join(security, "incompatible.psm1"), "function Set-Acl { throw 'Incompatible inherited module must not load' }; throw 'Incompatible inherited module must not load'");
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toLowerCase() !== "psmodulepath"));
  const modulePath = `${modules};${path.join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "Modules")}`;
  Object.assign(env, { PSModulePath: modulePath, ZCODE_ACP_STATE_DIR: path.join(root, "state") });
  const id = `bb-native-${randomUUID()}`;
  const stateModule = new URL("../lib/native-state.mjs", import.meta.url).href;
  const result = JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e", `
    import { prepareSessionState } from ${JSON.stringify(stateModule)};
    const state = prepareSessionState(${JSON.stringify(id)}, process.cwd(), false);
    state.markReady();
    console.log(JSON.stringify({ directory: state.directory, parentModules: process.env.PSModulePath, nativeModules: state.env.PSModulePath }));
  `], { env, encoding: "utf8", windowsHide: true, timeout: 25_000 }));
  assert.equal(result.parentModules, modulePath);
  assert.equal(result.nativeModules, modulePath);
  assert.equal(JSON.parse(fs.readFileSync(path.join(result.directory, "binding.json"), "utf8")).ready, true);
  // Check the resulting ACL independently, without loading any modules.
  execFileSync("powershell.exe", ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", `
    $ErrorActionPreference = 'Stop'
    $acl = [IO.Directory]::GetAccessControl($env:ZCODE_TEST_DIRECTORY)
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $rules = $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
    if (-not $acl.AreAccessRulesProtected -or $acl.GetOwner([Security.Principal.SecurityIdentifier]) -ne $identity -or $rules.Count -ne 1) { throw 'Unexpected session ACL' }
    $rule = $rules[0]
    if ($rule.IdentityReference -ne $identity -or $rule.IsInherited -or $rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or $rule.FileSystemRights -ne [Security.AccessControl.FileSystemRights]::FullControl -or $rule.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit') { throw 'Unexpected session access rule' }
  `], { env: { ...env, ZCODE_TEST_DIRECTORY: result.directory }, stdio: "pipe", windowsHide: true, timeout: 15_000 });
});

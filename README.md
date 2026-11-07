# zcode-acp

Standalone ACP agent for the native runtime bundled with ZCode Desktop. Supports
Windows and macOS discovery; live execution has been verified on Windows only.
Requires Node 24+ and installed ZCode. No BB SDK or runtime npm dependencies.

```powershell
node bin/zcode-acp.mjs --help
node bin/zcode-acp.mjs --doctor
node bin/zcode-acp.mjs --permission=accept-edits
```

Without `--help` or `--doctor`, the process serves newline-delimited ACP JSON-RPC
on stdin/stdout, for an ACP client. It is not an interactive terminal chat.
The npm package exposes the `zcode-acp` binary. `--permission=full` selects full
native permissions; standalone default mode asks before changes. The old
`--bb-permission` flag is retained as an authoritative compatibility alias.

Desktop sign-in is reused for independent Coding Plan and Start Plan routes.
Personal API connections remain separate sources. Models and reasoning options
come from the native catalog. Doctor checks account/catalog only; no generation
test, promotion claim or Desktop plan switch runs during discovery.

## JavaScript API

```js
import { NativeAcpAgent, collectPlanUsage, getAccountFingerprint, getRuntimeInfo } from "zcode-acp";

const usage = await collectPlanUsage("start"); // or "coding"; quota APIs only
const runtime = getRuntimeInfo();             // installed CLI path and revision
```

Imports have no process, stdio or network side effects. The root API also exposes
`findNativeEntry`, `nativeDoctor`, `codingQuotaUsage` and `startQuotaUsage`.
`getAccountFingerprint` supplies an opaque, in-memory account-generation scope
for invalidating a caller's cache. It is not an account ID or a credential.
Type declarations are included. The CLI/stdio subpaths intentionally start I/O
when explicitly imported by compatibility launchers.

The native host implementation owns account authentication, streaming, tool and
permission events, cancellation, model/reasoning selection and session restore.
Session IDs (`bb-native-*`) and the existing `~/.zcode-bridge/native-acp` storage
format are retained for compatibility. Desktop-owned tasks are never resumed.
Windows state uses a current-user ACL; POSIX state uses restrictive modes.
Real inference and macOS execution have not been qualified here; synthetic
turns, account discovery, configuration and quota checks have been verified.

Image/audio input, Team Plan, custom subagent profiles, browser interactions,
native interactive questions and manual compaction remain unsupported.

## Development

```powershell
node scripts/build-native-auth.mjs --check
node --test tests/*.test.mjs
npm pack
```

Tests use synthetic native servers and account responses. Native Windows ACL
tests require PowerShell. Packaged CLI smoke tests can target an independently
installed tarball through `ZCODE_TEST_PACKAGED_CLI`. The MIT account adapter's
source and notices are included; see NOTICE and LICENSES.

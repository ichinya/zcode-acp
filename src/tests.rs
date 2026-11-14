use crate::{
    account::{self, Account},
    common::*,
    config::Config,
    models, quota,
    rpc::{Peer, read_frame},
    state::State,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("zcode-rust-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn env(&self) -> Env {
        let mut env: Env = std::env::vars().collect();
        env.retain(|k, _| !k.starts_with("ZCODE_"));
        env.insert("ZCODE_STORAGE_DIR".into(), path_string(&self.0));
        env.insert(
            "ZCODE_ACP_STATE_DIR".into(),
            path_string(self.0.join("state")),
        );
        env
    }
    fn write(&self, name: &str, v: &Value) {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_vec(v).unwrap()).unwrap();
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}
fn runtime_bundle(t: &Temp, os: &str, mac_name: &str) -> (PathBuf, PathBuf) {
    let prefix = if os == "darwin" {
        "Z Code.app/Contents"
    } else {
        "Z Code"
    };
    let resources = t.0.join(prefix).join(if os == "darwin" {
        "Resources"
    } else {
        "resources"
    });
    let glm = resources.join("glm");
    std::fs::create_dir_all(&glm).unwrap();
    let entry = glm.join("zcode.cjs");
    std::fs::write(&entry, "fixture").unwrap();
    std::fs::write(
        glm.join(".node-bundle-meta.json"),
        json!({"runtime":"electron-node","entry":"zcode.cjs","platform":format!("{os}-x64")})
            .to_string(),
    )
    .unwrap();
    let root = resources.parent().unwrap();
    let exe = if os == "darwin" {
        std::fs::create_dir_all(root.join("MacOS")).unwrap();
        std::fs::write(root.join("Info.plist"),format!("<plist><dict><key>CFBundleExecutable</key><string>{mac_name}</string></dict></plist>")).unwrap();
        root.join("MacOS").join(mac_name)
    } else {
        root.join("ZCode.exe")
    };
    std::fs::write(&exe, "fixture executable").unwrap();
    (entry, exe)
}
#[test]
fn embedded_runtime_belongs_to_selected_windows_bundle() {
    let t = Temp::new();
    let (entry, exe) = runtime_bundle(&t, "win32", "");
    let r = crate::runtime::Runtime::for_platform(&entry, &Env::new(), "win32");
    assert_eq!(r.executable, exe);
    assert!(r.electron);
    assert_eq!(r.source, "bundle");
    assert_eq!(r.info()["kind"], "zcode-electron");
    std::fs::remove_file(exe).unwrap();
    let r = crate::runtime::Runtime::for_platform(&entry, &Env::new(), "win32");
    assert!(!r.electron);
    assert_eq!(r.source, "path");
    assert_eq!(r.executable, PathBuf::from("node"));
}
#[test]
fn mac_runtime_uses_bundle_executable_and_rejects_traversal() {
    let t = Temp::new();
    let (entry, exe) = runtime_bundle(&t, "darwin", "ZCode Test");
    let r = crate::runtime::Runtime::for_platform(&entry, &Env::new(), "darwin");
    assert!(r.electron);
    assert_eq!(r.executable, exe);
    let contents = entry.parent().unwrap().parent().unwrap().parent().unwrap();
    std::fs::write(
        contents.join("Info.plist"),
        "<key>CFBundleExecutable</key><string>../../other</string>",
    )
    .unwrap();
    assert!(!crate::runtime::Runtime::for_platform(&entry, &Env::new(), "darwin").electron);
}
#[test]
fn explicit_runtime_is_authoritative_and_can_select_own_electron() {
    let t = Temp::new();
    let (entry, exe) = runtime_bundle(&t, "win32", "");
    let mut env = Env::new();
    let missing = t.0.join("missing-node.exe");
    env.insert("ZCODE_NODE".into(), path_string(&missing));
    let r = crate::runtime::Runtime::for_platform(&entry, &env, "win32");
    assert_eq!(r.source, "override");
    assert_eq!(r.executable, missing);
    assert!(!r.electron);
    env.insert("ZCODE_NODE".into(), path_string(&exe));
    let r = crate::runtime::Runtime::for_platform(&entry, &env, "win32");
    assert!(r.electron);
    assert_eq!(r.source, "override");
    env.insert("ZCODE_NODE".into(), "".into());
    assert_eq!(
        crate::runtime::Runtime::for_platform(&entry, &env, "win32").source,
        "bundle"
    );
}
#[test]
fn runtime_metadata_must_match_entry_platform_and_declared_runtime() {
    let t = Temp::new();
    let (entry, _) = runtime_bundle(&t, "win32", "");
    let marker = entry.parent().unwrap().join(".node-bundle-meta.json");
    for data in [
        json!({}),
        json!({"runtime":"node","entry":"zcode.cjs","platform":"win32-x64"}),
        json!({"runtime":"electron-node","entry":"other.cjs","platform":"win32-x64"}),
        json!({"runtime":"electron-node","entry":"zcode.cjs","platform":"darwin-arm64"}),
    ] {
        std::fs::write(&marker, data.to_string()).unwrap();
        assert_eq!(
            crate::runtime::Runtime::for_platform(&entry, &Env::new(), "win32").source,
            "path"
        );
    }
    std::fs::write(&marker, " ".repeat(64 * 1024 + 1)).unwrap();
    assert_eq!(
        crate::runtime::Runtime::for_platform(&entry, &Env::new(), "win32").source,
        "path"
    );
    std::fs::remove_file(marker).unwrap();
    assert_eq!(
        crate::runtime::Runtime::for_platform(&entry, &Env::new(), "win32").source,
        "path"
    );
}
#[test]
fn runtime_flag_is_scoped_and_removed_for_external_node() {
    let t = Temp::new();
    let (entry, _) = runtime_bundle(&t, "win32", "");
    let mut env = Env::new();
    env.insert("ELECTRON_RUN_AS_NODE".into(), "parent-value".into());
    env.insert("electron_run_as_node".into(), "case-variant".into());
    let embedded = crate::runtime::Runtime::for_platform(&entry, &env, "win32");
    let child = embedded.environment(&env);
    assert_eq!(child["ELECTRON_RUN_AS_NODE"], "1");
    assert!(!child.contains_key("electron_run_as_node"));
    env.insert("ZCODE_NODE".into(), "node-override".into());
    let explicit = crate::runtime::Runtime::for_platform(&entry, &env, "win32");
    assert!(
        !explicit
            .environment(&env)
            .contains_key("ELECTRON_RUN_AS_NODE")
    );
    assert_eq!(env["ELECTRON_RUN_AS_NODE"], "parent-value");
    assert_eq!(env["electron_run_as_node"], "case-variant");
}
#[test]
fn cli_options_are_scoped_and_validate() {
    let mut env = Env::new();
    env.insert("ZCODE_NATIVE_ENTRY".into(), "old".into());
    let c = Config::parse(
        &args(&[
            "--native-entry",
            "D:/Z Code/zcode.cjs",
            "--node=node24",
            "--permission=full",
            "--bb-permission=accept-edits",
        ]),
        env.clone(),
    )
    .unwrap();
    assert_eq!(c.permission.as_deref(), Some("accept-edits"));
    assert_eq!(c.env["ZCODE_NATIVE_ENTRY"], "D:/Z Code/zcode.cjs");
    assert_eq!(env["ZCODE_NATIVE_ENTRY"], "old");
    for v in [
        vec!["--native-entry"],
        vec!["--native-entry="],
        vec!["--node", "x", "--node", "y"],
        vec!["--permission=yolo"],
        vec!["usage"],
        vec!["usage", "--plan", "wrong"],
    ] {
        assert!(Config::parse(&args(&v), Env::new()).is_err());
    }
}
#[test]
fn explicit_entry_does_not_fall_back() {
    let t = Temp::new();
    let mut env = t.env();
    env.insert(
        "ZCODE_NATIVE_ENTRY".into(),
        path_string(t.0.join("missing")),
    );
    assert!(crate::config::find_entry(&env).is_err());
}
#[test]
fn paths_match_node_lexical_identity() {
    let t = Temp::new();
    let path = absolute(t.0.join("resources/./glm/../config/provider/zcode-builtin.json")).unwrap();
    let rendered = path_string(path);
    assert!(!rendered.contains(".."));
    if cfg!(windows) {
        assert!(!rendered.contains('/'));
    }
    assert!(rendered.ends_with(if cfg!(windows) {
        r"resources\config\provider\zcode-builtin.json"
    } else {
        "resources/config/provider/zcode-builtin.json"
    }));
}
#[test]
fn mcp_translates_acp_stdio_and_http_without_extra_fields() {
    let result = crate::agent::mcp_servers(&json!([
        {"name":"stdio","type":"stdio","command":"tool","extra":"ignored"},
        {"name":"http","type":"http","url":"https://example.com/mcp"},
        {"name":"sse","type":"sse","url":"https://example.com/sse","headers":[{"name":"A","value":"B"}]}
    ])).unwrap();
    assert_eq!(
        result[0],
        json!({"name":"stdio","command":"tool","args":[],"env":[],"isolation":"session"})
    );
    assert_eq!(result[1]["headers"], json!([]));
    assert_eq!(result[2]["headers"][0]["value"], "B");
    assert!(crate::agent::mcp_servers(&json!({})).is_err());
}
#[test]
fn credentials_match_node_encryption_and_scope() {
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    let t = Temp::new();
    let mut env = t.env();
    env.insert("ZCODE_CREDENTIAL_SECRET".into(), "fixture-secret".into());
    let cipher = Aes256Gcm::new_from_slice(&Sha256::digest(b"fixture-secret")).unwrap();
    let iv = [7_u8; 12];
    let mut encrypted = cipher
        .encrypt(Nonce::from_slice(&iv), b"fixture-jwt".as_ref())
        .unwrap();
    let tag = encrypted.split_off(encrypted.len() - 16);
    let encoded = format!(
        "enc:v1:{}.{}.{}",
        URL_SAFE_NO_PAD.encode(iv),
        URL_SAFE_NO_PAD.encode(tag),
        URL_SAFE_NO_PAD.encode(encrypted)
    );
    t.write("v2/credentials.json",&json!({"oauth:active_provider":"zai","oauth:zai:access_token":"Bearer fixture-oauth","oauth:zai:user_info":"identity","zcodejwttoken":encoded}));
    let login = account::read_login(&env).unwrap().unwrap();
    assert_eq!(login.jwt, "fixture-jwt");
    assert_eq!(
        login.fingerprint,
        hash(
            serde_json::to_vec(&json!(["zai", "fixture-oauth", "identity", "fixture-jwt"]))
                .unwrap()
        )
    );
    env.insert("ZCODE_CREDENTIAL_SECRET".into(), "wrong".into());
    assert!(account::read_login(&env).is_err());
}
fn balance() -> Value {
    json!({"server_time":1000,"plans":[{"user_plan_id":"p","status":"active","starts_at":900,"ends_at":1100}],"balances":[{"user_plan_id":"p","unit_type":"token","meter":"model_usage","total_units":100,"used_units":25,"remaining_units":75,"available_units":75,"capabilities":["model:GLM-Flash"]}]})
}
#[test]
fn start_grants_only_active_matching_balances() {
    let data = balance();
    let ids = vec![json!("GLM-Flash"), json!("unrelated")];
    assert_eq!(
        account::start_entitlement(&data, &ids, 1_000_000.0, None)
            .unwrap()
            .ids,
        vec!["GLM-Flash"]
    );
    for (field, value) in [
        ("available_units", json!(0)),
        ("expires_at", json!(999)),
        ("effective_at", json!(1001)),
        ("user_plan_id", json!("other")),
        ("capabilities", json!(["model:unknown"])),
    ] {
        let mut b = data.clone();
        b["balances"][0][field] = value;
        assert!(
            account::start_entitlement(&b, &ids, 1_000_000.0, None)
                .unwrap()
                .ids
                .is_empty(),
            "{field}"
        );
    }
    let mut b = data.clone();
    b["plans"][0]["status"] = json!("expired");
    assert!(
        account::start_entitlement(&b, &ids, 1_000_000.0, None)
            .unwrap()
            .ids
            .is_empty()
    );
    assert!(account::start_entitlement(&json!({}), &ids, 0.0, None).is_err());
}
#[test]
fn server_clock_controls_entitlements() {
    let e = account::start_entitlement(
        &balance(),
        &[json!("GLM-Flash")],
        999_000_000.0,
        Some(1_000_000.0),
    )
    .unwrap();
    assert_eq!(e.ids, vec!["GLM-Flash"]);
    assert_eq!(e.until, 999_060_000.0);
}
#[test]
fn quota_windows_and_bad_data() {
    let r = quota::coding_usage(
        &json!({"limits":[{"type":"TIME_LIMIT","unit":5,"number":1,"percentage":45},{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":3,"nextResetTime":1234000}]}),
        "Coding Pro",
    );
    assert_eq!(r["windows"][0]["id"], "model-5h");
    assert_eq!(r["windows"][1]["usedPercent"], 45.0);
    assert_eq!(
        quota::coding_usage(
            &json!({"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":"3"}]}),
            "X"
        )["status"],
        "error"
    );
    let r = quota::start_usage(&balance(), 1_000_000.0, None);
    assert_eq!(r["windows"][0]["usedPercent"], 25.0);
    let mut b = balance();
    b["balances"][0]["used_units"] = json!(101);
    assert_eq!(quota::start_usage(&b, 1_000_000.0, None)["status"], "error");
}
#[test]
fn models_keep_plan_and_provider_identity() {
    let available = vec![
        json!({"ref":{"providerId":"personal","modelId":"same"},"label":"Same","providerLabel":"API"}),
        json!({"ref":{"providerId":"account:zai-start-plan","modelId":"same"},"label":"Same"}),
        json!({"ref":{"providerId":"account:zai-individual-coding-plan","modelId":"same"},"label":"Same"}),
    ];
    let (m, o) = models::catalog(&available);
    assert!(text(&m[0]["name"]).starts_with("Coding Plan"));
    assert!(text(&m[1]["name"]).starts_with("Start Plan"));
    assert_eq!(o.len(), 3);
    assert_ne!(m[0]["modelId"], m[1]["modelId"]);
    assert_eq!(
        models::model_id(&json!({"providerId":"fake","modelId":"offline"})),
        "eyJwcm92aWRlcklkIjoiZmFrZSIsIm1vZGVsSWQiOiJvZmZsaW5lIn0"
    );
}
#[tokio::test]
async fn framing_preserves_split_unicode_and_bounds() {
    let (mut tx, rx) = tokio::io::duplex(128);
    tokio::spawn(async move {
        tx.write_all(&[b'"', 0xd0]).await.unwrap();
        tx.write_all(&[0x9f, b'"', b'\n']).await.unwrap();
    });
    let mut r = BufReader::new(rx);
    let b = read_frame(&mut r, 100).await.unwrap().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&b).unwrap(), "П");
    let mut r = BufReader::new(&b"12345678\n"[..]);
    assert!(read_frame(&mut r, 4).await.is_err());
}
#[tokio::test]
async fn rpc_correlates_reordered_results_and_redacts_native_errors() {
    let (a, b) = tokio::io::duplex(4096);
    let (ar, aw) = tokio::io::split(a);
    let (br, mut bw) = tokio::io::split(b);
    let (peer, _) = Peer::start(ar, aw, false);
    tokio::spawn(async move {
        let mut r = BufReader::new(br).lines();
        let one: Value = serde_json::from_str(&r.next_line().await.unwrap().unwrap()).unwrap();
        let two: Value = serde_json::from_str(&r.next_line().await.unwrap().unwrap()).unwrap();
        let messages = [
            json!({"id":two["id"],"result":2}),
            json!({"id":one["id"],"error":{"code":-32000,"message":"fixture-secret"}}),
        ];
        for m in messages {
            bw.write_all(format!("{m}\n").as_bytes()).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    });
    let (a, b) = tokio::join!(
        peer.request("a", json!({}), Duration::from_secs(1), None),
        peer.request("b", json!({}), Duration::from_secs(1), None)
    );
    let e = a.unwrap_err();
    assert_eq!(e.code, -32000);
    assert!(!e.message.contains("fixture-secret"));
    assert_eq!(b.unwrap(), 2);
    peer.fail();
}
#[tokio::test]
async fn state_binding_prevents_wrong_workspace_and_records_unknown_delivery() {
    let t = Temp::new();
    let env = t.env();
    let id = format!("bb-native-{}", uuid::Uuid::new_v4());
    let mut s = State::prepare(&id, &t.0, false, &env).await.unwrap();
    s.save(json!({"ready":true})).unwrap();
    s.save(json!({"inputSubmitted":true})).unwrap();
    let loaded = State::prepare(&id, &t.0, true, &env).await.unwrap();
    assert_eq!(loaded.saved["inputSubmitted"], true);
    let other = t.0.join("other");
    std::fs::create_dir(&other).unwrap();
    assert!(State::prepare(&id, &other, true, &env).await.is_err());
    assert!(
        State::prepare("desktop-task", &t.0, true, &env)
            .await
            .is_err()
    );
}

async fn mock_service(
    reject_coding: bool,
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    let handle = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let log = log.clone();
            tokio::spawn(async move {
                let (mut read, mut write) = socket.into_split();
                let mut r = BufReader::new(&mut read);
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if line.to_ascii_lowercase().starts_with("content-length:") {
                        length = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                    }
                    head += &line;
                }
                let mut body = vec![0; length];
                r.read_exact(&mut body).await.unwrap();
                let path = head.split_whitespace().nth(1).unwrap();
                log.lock().unwrap().push(path.to_string());
                let start = path.starts_with("/api/v1/zcode-plan/");
                let denied = reject_coding && !start;
                let data = if start {
                    let mut b = balance();
                    b["server_time"] = json!(now() / 1000.0);
                    b["plans"][0]["starts_at"] = json!(now() / 1000.0 - 100.0);
                    b["plans"][0]["ends_at"] = json!(now() / 1000.0 + 100.0);
                    b
                } else if path.ends_with("getCustomerInfo") {
                    json!({"organizations":[{"organizationId":"org","projects":[{"projectId":"project"}]}]})
                } else if path.ends_with("/api_keys") {
                    json!([{"name":"zcode-api-key","apiKey":"key-id"}])
                } else if path.ends_with("/access_tokens") {
                    assert!(String::from_utf8_lossy(&body).contains("3.17.2"));
                    json!({"accessToken":"fixture-project-token","tokenType":"Bearer","expiresIn":3600,"expiresAt":now()/1000.0+3600.0})
                } else if path.ends_with("/subscription/list") {
                    json!([{"productName":"Coding Plan","status":"VALID","inCurrentPeriod":true}])
                } else {
                    json!({})
                };
                let body = json!({"code":200,"data":data}).to_string();
                let status = if denied { "403 Forbidden" } else { "200 OK" };
                let output = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                write.write_all(output.as_bytes()).await.unwrap();
            });
        }
    });
    (format!("http://{addr}"), seen, handle)
}
fn account_fixture(t: &Temp, family: &str, jwt_only: bool) -> Env {
    t.write("v2/credentials.json",&json!({"oauth:active_provider":family,format!("oauth:{family}:access_token"):if jwt_only{""}else{"fixture-oauth"},"zcodejwttoken":"fixture-jwt"}));
    t.write(
        "v2/telemetry-state.json",
        &json!({"deviceMid":"fixture-mid"}),
    );
    let rules:Vec<_>=["individual-coding-plan","start-plan"].iter().map(|mode|json!({"providerId":format!("account:{family}-{mode}"),"config":{"access":{"type":"zhipu-account","mode":mode,"accountType":family},"builtinModelIds":["GLM-Flash"]}})).collect();
    t.write(
        "builtin.json",
        &json!({"revision":43,"config":{"providerConfigRules":{"providerRules":rules}}}),
    );
    let mut env = t.env();
    env.insert(
        "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE".into(),
        path_string(t.0.join("builtin.json")),
    );
    env.insert("ZCODE_APP_VERSION".into(), "3.17.2".into());
    env
}
#[tokio::test]
async fn both_plan_families_authenticate_without_mixing_credentials() {
    for family in ["zai", "bigmodel"] {
        let t = Temp::new();
        let env = account_fixture(&t, family, false);
        let before = std::fs::read(t.0.join("v2/credentials.json")).unwrap();
        let mut a = Account::new(env).unwrap();
        let (origin, seen, server) = mock_service(false).await;
        a.test_origin = Some(origin);
        let overlay = a.initialize().await.unwrap().unwrap();
        assert!(text(&overlay["basedOnZCodeBuiltinRevision"]).starts_with("zcode-builtin:43:"));
        assert_eq!(overlay["providers"].as_object().unwrap().len(), 2);
        let id = format!("account:{family}-start-plan");
        let mut p = json!({"sessionId":"s","requestId":"r","reason":"model-request","providerId":id,"accountAccess":{"type":"zhipu-account","mode":"start-plan","accountType":family,"entitled":true},"modelSelection":{"providerId":id,"modelId":"GLM-Flash"},"workspace":workspace(&t.0)});
        assert_eq!(
            a.authorize(&p, "s", &t.0).await["requestAuth"]["apiKey"],
            "fixture-jwt"
        );
        for key in [
            "sessionId",
            "providerId",
            "workspace",
            "modelSelection",
            "accountAccess",
            "expectedAccountScope",
            "rejectedProjectTokenFingerprint",
        ] {
            let mut bad = p.clone();
            bad[key] = json!("wrong");
            let r = a.authorize(&bad, "s", &t.0).await;
            assert_eq!(r["headersApplied"], false, "{key}");
            assert!(r.get("requestAuth").is_none());
        }
        let coding = format!("account:{family}-individual-coding-plan");
        p["providerId"] = json!(coding);
        p["modelSelection"]["providerId"] = json!(coding);
        p["accountAccess"]["mode"] = json!("individual-coding-plan");
        assert_eq!(
            a.authorize(&p, "s", &t.0).await["requestAuth"]["apiKey"],
            "fixture-project-token"
        );
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|p| p.ends_with("/access_tokens"))
                .count(),
            1
        );
        assert_eq!(
            std::fs::read(t.0.join("v2/credentials.json")).unwrap(),
            before
        );
        t.write("v2/credentials.json", &json!({}));
        assert_eq!(a.authorize(&p, "s", &t.0).await["headersApplied"], false);
        server.abort();
    }
}
#[tokio::test]
async fn start_survives_rejected_coding_or_jwt_only_signin() {
    for jwt_only in [true, false] {
        let t = Temp::new();
        let mut a = Account::new(account_fixture(&t, "zai", jwt_only)).unwrap();
        let (origin, _, server) = mock_service(true).await;
        a.test_origin = Some(origin);
        let o = a.initialize().await.unwrap().unwrap();
        assert!(!a.coding_ready);
        assert!(a.start_ready);
        assert_eq!(o["providers"].as_object().unwrap().len(), 1);
        server.abort();
    }
}

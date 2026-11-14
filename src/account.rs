// Account flow adapted from dpolishuk/bb-zcode coding-plan.ts, MIT.
// Copyright (c) 2026 Dmitry Polishuk. See LICENSES/bb-zcode-MIT.txt and NOTICE.
use crate::{
    common::*,
    config::{find_entry, native_env},
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

#[derive(Clone)]
pub struct Login {
    pub family: String,
    pub token: String,
    pub jwt: String,
    pub fingerprint: String,
}
pub fn read_login(env: &Env) -> Result<Option<Login>> {
    let p = storage(env).join("credentials.json");
    let store = if p.exists() {
        read_json(&p)?
    } else {
        json!({})
    };
    let mut reader = crate::credentials::Reader::new(&p, env);
    let mut get = |key: &str| reader.decrypt(text(&store[key]));
    let family = get("oauth:active_provider")?.trim().to_owned();
    if family != "zai" && family != "bigmodel" {
        return Ok(None);
    }
    let token = strip_bearer(&get(&format!("oauth:{family}:access_token"))?);
    let jwt = strip_bearer(&get("zcodejwttoken")?);
    if token.is_empty() && jwt.is_empty() {
        return Ok(None);
    }
    let identity = get(&format!("oauth:{family}:user_info"))?;
    let fingerprint = hash(serde_json::to_vec(&json!([family, token, identity, jwt]))?);
    Ok(Some(Login {
        family,
        token,
        jwt,
        fingerprint,
    }))
}
fn strip_bearer(s: &str) -> String {
    let s = s.trim();
    if s.get(..7)
        .is_some_and(|v| v.eq_ignore_ascii_case("Bearer "))
    {
        s[7..].trim().to_owned()
    } else {
        s.to_owned()
    }
}
pub fn account_scope(env: &Env) -> String {
    let Ok(entry) = find_entry(env) else {
        return "not-installed".into();
    };
    match read_login(&native_env(&entry, env)) {
        Ok(Some(l)) => l.fingerprint,
        Ok(None) => "signed-out".into(),
        Err(_) => "unreadable".into(),
    }
}
#[derive(Clone)]
struct Material {
    token: String,
    key_id: String,
    expires: f64,
}
#[derive(Clone)]
pub struct Entitlement {
    pub ids: Vec<String>,
    pub until: f64,
}
pub fn start_entitlement(
    data: &Value,
    ids: &[Value],
    clock: f64,
    server_date: Option<f64>,
) -> Result<Entitlement> {
    if !data["plans"].is_array() || !data["balances"].is_array() {
        return Err(Error::new("ZCode Start Plan: invalid balance response"));
    }
    let server = server_date
        .or_else(|| epoch(&data["server_time"]))
        .unwrap_or(clock);
    let mut granted = vec![];
    let mut until = 60_000_f64;
    for b in list(&data["balances"]) {
        if b["unit_type"] != "token"
            || b["meter"] != "model_usage"
            || number(
                b.get("available_units")
                    .filter(|v| !v.is_null())
                    .unwrap_or(&b["remaining_units"]),
            )
            .unwrap_or(0.0)
                <= 0.0
        {
            continue;
        }
        for plan in list(&data["plans"]).iter().filter(|p| linked(b, p)) {
            if !text(&plan["status"]).trim().eq_ignore_ascii_case("active") {
                continue;
            }
            let e = list(&plan["entitlements"])
                .iter()
                .find(|e| {
                    !b["entitlement_id"].is_null() && e["entitlement_id"] == b["entitlement_id"]
                })
                .cloned()
                .unwrap_or(Value::Null);
            let starts = [
                &plan["starts_at"],
                &b["period_start"],
                &b["effective_at"],
                &e["effective_at"],
            ];
            let ends: Vec<_> = [&plan["ends_at"], &b["period_end"], &b["expires_at"]]
                .into_iter()
                .filter_map(epoch)
                .collect();
            if starts.into_iter().filter_map(epoch).any(|t| t > server)
                || ends.iter().any(|t| *t <= server)
            {
                continue;
            }
            let mut names: Vec<_> = list(&b["capabilities"])
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| s.to_ascii_lowercase().starts_with("model:"))
                .map(|s| s[6..].trim().to_owned())
                .collect();
            if names.is_empty() {
                names.push(text(&b["show_name"]).trim().into());
            }
            for id in ids.iter().filter_map(Value::as_str) {
                if names.iter().any(|n| n.eq_ignore_ascii_case(id))
                    && !granted.iter().any(|s| s == id)
                {
                    granted.push(id.to_owned());
                }
            }
            for end in ends {
                until = until.min(end - server);
            }
        }
    }
    Ok(Entitlement {
        ids: granted,
        until: clock + until.max(0.0),
    })
}
pub fn linked(b: &Value, p: &Value) -> bool {
    if !text(&b["user_plan_id"]).is_empty() && !text(&p["user_plan_id"]).is_empty() {
        b["user_plan_id"] == p["user_plan_id"]
    } else {
        !text(&b["plan_id"]).is_empty() && b["plan_id"] == p["plan_id"]
    }
}
pub struct Account {
    pub env: Env,
    login: Option<Login>,
    client: reqwest::Client,
    material: Option<Material>,
    scope: String,
    pub coding: Option<Value>,
    pub start: Option<Value>,
    pub entitlement: Option<Entitlement>,
    pub balance_data: Option<Value>,
    pub server_date: Option<f64>,
    pub coding_ready: bool,
    pub start_ready: bool,
    pub plan_label: Option<String>,
    pub diagnostics: Vec<String>,
    #[cfg(test)]
    pub test_origin: Option<String>,
}
impl Account {
    pub fn new(env: Env) -> Result<Self> {
        Ok(Self {
            env,
            login: None,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|_| Error::new("Cannot initialize account HTTP client"))?,
            material: None,
            scope: hash(uuid::Uuid::new_v4().to_string()),
            coding: None,
            start: None,
            entitlement: None,
            balance_data: None,
            server_date: None,
            coding_ready: false,
            start_ready: false,
            plan_label: None,
            diagnostics: vec![],
            #[cfg(test)]
            test_origin: None,
        })
    }
    fn current(&self) -> Result<Login> {
        let current =
            read_login(&self.env)?.ok_or_else(|| Error::new("Desktop sign-in unavailable"))?;
        if self
            .login
            .as_ref()
            .is_none_or(|p| p.fingerprint != current.fingerprint)
        {
            return Err(Error::new(
                "Desktop sign-in changed. Start a new ACP session",
            ));
        }
        Ok(current)
    }
    async fn request(
        &self,
        url: &str,
        auth: &str,
        body: Option<Value>,
        headers: Vec<(&str, String)>,
        optional: bool,
    ) -> Result<(Value, Option<f64>)> {
        #[cfg(test)]
        let test_url = self.test_origin.as_ref().map(|origin| {
            let u = url::Url::parse(url).unwrap();
            format!(
                "{}{}{}",
                origin,
                u.path(),
                u.query().map(|q| format!("?{q}")).unwrap_or_default()
            )
        });
        #[cfg(test)]
        let url = test_url.as_deref().unwrap_or(url);
        self.current()?;
        let mut r = if let Some(b) = body {
            self.client.post(url).json(&b)
        } else {
            self.client.get(url)
        };
        r = r
            .header("Authorization", auth)
            .header("Content-Type", "application/json");
        for (k, v) in headers {
            r = r.header(k, v);
        }
        let fail = || {
            Error::new(
                "Account request failed or returned an invalid response. Check ZCode sign-in and network access",
            )
        };
        let mut response = r.send().await.map_err(|error| {
            // Classify transport causes without forwarding reqwest's URL (it
            // can contain a project key) or an upstream response body.
            let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
            let mut reason = if error.is_timeout() {
                "timeout"
            } else if error.is_connect() {
                "connection"
            } else {
                "transport"
            };
            while let Some(current) = cause {
                let detail = current.to_string().to_ascii_lowercase();
                if detail.contains("certificate") || detail.contains("unknownissuer") {
                    reason = "TLS certificate validation";
                    break;
                }
                if detail.contains("dns")
                    || detail.contains("lookup address")
                    || detail.contains("no such host")
                {
                    reason = "DNS lookup";
                }
                if detail.contains("connection reset") {
                    reason = "connection reset";
                }
                if detail.contains("connection refused") {
                    reason = "connection refused";
                }
                cause = current.source();
            }
            Error::new(format!("ZCode account request failed: {reason}"))
        })?;
        if !response.status().is_success() {
            return Err(Error::new(format!(
                "ZCode account service returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let date = response
            .headers()
            .get("date")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
            .map(|d| d.timestamp_millis() as f64);
        let mut bytes = Vec::new();
        while let Some(c) = response.chunk().await.map_err(|_| fail())? {
            if bytes.len() + c.len() > 1024 * 1024 {
                return Err(fail());
            }
            bytes.extend_from_slice(&c);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::new("ZCode account service returned invalid JSON"))?;
        let code = &value["code"];
        if value["success"] == false
            || !([json!(0), json!(200), json!("0"), json!("200")].contains(code)
                || (optional && value.get("code").is_none()))
        {
            let code = code
                .as_i64()
                .map(|n| n.to_string())
                .or_else(|| {
                    code.as_str()
                        .filter(|s| {
                            s.len() <= 32
                                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        })
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "unavailable".into());
            return Err(Error::new(format!(
                "ZCode account request was rejected (code {code})"
            )));
        }
        self.current()?;
        Ok((value["data"].clone(), date))
    }
    fn origin(&self) -> &str {
        if self.login.as_ref().is_some_and(|l| l.family == "bigmodel") {
            "https://bigmodel.cn"
        } else {
            "https://api.z.ai"
        }
    }
    async fn coding_request(
        &self,
        path: &str,
        auth: &str,
        body: Option<Value>,
        optional: bool,
    ) -> Result<Value> {
        Ok(self
            .request(
                &format!("{}{path}", self.origin()),
                auth,
                body,
                vec![],
                optional,
            )
            .await?
            .0)
    }
    async fn obtain_material(&mut self) -> Result<Material> {
        let login = self.current()?;
        if login.token.is_empty() {
            return Err(Error::new("Coding Plan sign-in is unavailable"));
        }
        let auth = if login.family == "zai" {
            format!("Bearer {}", login.token)
        } else {
            login.token.clone()
        };
        let customer = self
            .coding_request("/api/biz/customer/getCustomerInfo", &auth, None, false)
            .await
            .map_err(|e| Error::new(format!("Coding Plan account lookup: {}", e.message)))?;
        let orgs: Vec<_> = list(&customer["organizations"])
            .iter()
            .filter(|o| {
                !text(&o["organizationId"]).is_empty()
                    && list(&o["projects"]).iter().any(valid_project)
            })
            .collect();
        let org = orgs
            .iter()
            .find(|o| text(&o["organizationName"]).contains("默认机构"))
            .or_else(|| orgs.first())
            .ok_or_else(|| Error::new("No personal Coding Plan project is available"))?;
        let projects: Vec<_> = list(&org["projects"])
            .iter()
            .filter(|p| valid_project(p))
            .collect();
        let project = projects
            .iter()
            .find(|p| text(&p["projectName"]).contains("默认项目"))
            .or_else(|| projects.first())
            .ok_or_else(|| Error::new("No personal Coding Plan project is available"))?;
        let path = format!(
            "/api/biz/v1/organization/{}/projects/{}/api_keys",
            encode(text(&org["organizationId"])),
            encode(text(&project["projectId"]))
        );
        let keys = self
            .coding_request(&path, &auth, None, false)
            .await
            .map_err(|e| Error::new(format!("Coding Plan project key lookup: {}", e.message)))?;
        let key = list(&keys)
            .iter()
            .find(|k| k["name"] == "zcode-api-key")
            .map(|k| text(&k["apiKey"]).trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Error::new("Open Coding Plan in ZCode once to prepare its project key")
            })?;
        let started = now();
        let data=self.coding_request(&format!("{path}/{}/access_tokens",encode(key)),&format!("Bearer {}",login.token),Some(json!({"clientType":"zcode","clientVersion":self.env.get("ZCODE_APP_VERSION").cloned().unwrap_or_default()})),false).await?;
        let expires = (started + data["expiresIn"].as_f64().unwrap_or(0.0) * 1000.0)
            .min(data["expiresAt"].as_f64().unwrap_or(0.0) * 1000.0);
        if text(&data["accessToken"]).trim().is_empty()
            || data["tokenType"] != "Bearer"
            || data["enable"] == false
            || data.get("enable").is_some_and(|v| !v.is_boolean())
            || data["expiresIn"].as_f64().unwrap_or(0.0) <= 0.0
            || expires <= now() + 5000.0
        {
            return Err(Error::new(
                "Short-lived Coding Plan project tokens are unavailable",
            ));
        }
        let token = text(&data["accessToken"]).trim().to_owned();
        let subscriptions = self
            .coding_request("/api/biz/subscription/list", &token, None, true)
            .await?;
        let active = list(&subscriptions)
            .iter()
            .find(|s| {
                s["status"] == "VALID"
                    && s["inCurrentPeriod"] == true
                    && [&s["productName"], &s["productId"]]
                        .iter()
                        .any(|v| text(v).to_lowercase().contains("coding"))
            })
            .ok_or_else(|| {
                Error::new("An active individual Coding Plan subscription could not be verified")
            })?;
        self.plan_label = Some(
            [
                text(&active["productName"]),
                text(&active["productId"]),
                "Coding Plan",
            ]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap()
            .to_owned(),
        );
        self.current()?;
        Ok(Material {
            token,
            key_id: key.into(),
            expires: (expires - 5000.0).min(now() + 300_000.0),
        })
    }
    async fn material(&mut self, rejected: Option<&str>) -> Result<Material> {
        self.current()?;
        if self
            .material
            .as_ref()
            .is_some_and(|m| rejected.is_some_and(|r| hash(&m.token) == r))
        {
            self.material = None;
        }
        if let Some(m) = &self.material
            && m.expires > now() + 30_000.0
        {
            return Ok(m.clone());
        }
        let m = self.obtain_material().await?;
        self.material = Some(m.clone());
        Ok(m)
    }
    pub async fn init_coding(&mut self, builtin: &Value) -> Result<()> {
        if self.login.is_none() {
            self.login = read_login(&self.env)?;
        }
        let Some(login) = self.login.as_ref().filter(|l| !l.token.is_empty()) else {
            return Ok(());
        };
        self.coding = list(&builtin["config"]["providerConfigRules"]["providerRules"])
            .iter()
            .find(|p| valid_provider(p, &login.family, "individual-coding-plan"))
            .cloned();
        if self.coding.is_none() {
            return Ok(());
        }
        if self
            .env
            .get("ZCODE_APP_VERSION")
            .is_none_or(|v| v.is_empty())
        {
            return Err(Error::new(
                "Cannot determine ZCode version. Set ZCODE_APP_VERSION for a custom bundle",
            ));
        }
        self.material(None).await?;
        self.coding_ready = true;
        Ok(())
    }
    pub async fn init_start(&mut self, builtin: &Value) -> Result<()> {
        if self.login.is_none() {
            self.login = read_login(&self.env)?;
        }
        let Some(login) = self.login.as_ref().filter(|l| !l.jwt.is_empty()) else {
            return Ok(());
        };
        self.start = list(&builtin["config"]["providerConfigRules"]["providerRules"])
            .iter()
            .find(|p| {
                p["providerId"] == format!("account:{}-start-plan", login.family)
                    && valid_provider(p, &login.family, "start-plan")
            })
            .cloned();
        if self.start.is_none() {
            return Ok(());
        }
        self.balance().await?;
        self.start_ready = self.entitlement.as_ref().is_some_and(|e| !e.ids.is_empty());
        Ok(())
    }
    pub async fn balance(&mut self) -> Result<()> {
        let login = self.current()?;
        let telemetry = read_json(storage(&self.env).join("telemetry-state.json"))
            .map_err(|_| Error::new("Desktop device identity is unavailable. Open ZCode once"))?;
        let mid = printable(text(&telemetry["deviceMid"]))
            .ok_or_else(|| Error::new("Desktop device identity is unavailable"))?;
        let version = self
            .env
            .get("ZCODE_APP_VERSION")
            .and_then(|v| printable(v))
            .ok_or_else(|| Error::new("ZCode version is unavailable"))?;
        let headers = vec![
            ("X-Device-Mid", mid),
            ("X-ZCode-App-Version", version.clone()),
            ("User-Agent", format!("ZCode/{version}")),
            ("HTTP-Referer", "https://zcode.z.ai".into()),
            ("X-Title", "Z Code@electron".into()),
            ("X-Platform", format!("{}-{}", platform(), arch())),
            ("X-Release-Channel", "stable".into()),
            ("X-Client-Language", "unknown".into()),
            ("X-Client-Timezone", "unknown".into()),
            (
                "X-Os-Category",
                if cfg!(windows) {
                    "windows"
                } else if cfg!(target_os = "macos") {
                    "macos"
                } else {
                    "linux"
                }
                .into(),
            ),
            ("X-Os-Version", "unknown".into()),
        ];
        let (data, date) = self
            .request(
                &format!(
                    "https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version={}",
                    encode(&version)
                ),
                &format!("Bearer {}", login.jwt),
                None,
                headers,
                false,
            )
            .await?;
        let ids = list(
            &self
                .start
                .as_ref()
                .ok_or_else(|| Error::new("Start Plan catalog is unavailable"))?["config"]["builtinModelIds"],
        );
        self.entitlement = Some(start_entitlement(&data, ids, now(), date)?);
        self.balance_data = Some(data);
        self.server_date = date;
        Ok(())
    }
    pub async fn initialize(&mut self) -> Result<Option<Value>> {
        let Some(file) = self.env.get("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE").cloned() else {
            return Ok(None);
        };
        let builtin = read_json(&file)?;
        if let Err(error) = self.init_coding(&builtin).await {
            self.diagnostics.push(format!(
                "Coding Plan could not be verified: {}",
                error.message
            ));
        }
        if let Err(error) = self.init_start(&builtin).await {
            self.diagnostics.push(format!(
                "Start Plan balance could not be verified: {}",
                error.message
            ));
        }
        let mut providers = json!({});
        let mut states = json!({});
        if self.coding_ready {
            let p = self.coding.as_ref().unwrap();
            providers[text(&p["providerId"])] = json!({"access":{"type":"zhipu-account","entitled":true},"builtinModelIds":p["config"]["builtinModelIds"]});
        }
        if self.start_ready {
            let p = self.start.as_ref().unwrap();
            providers[text(&p["providerId"])] = json!({"access":{"type":"zhipu-account","entitled":true},"builtinModelIds":self.entitlement.as_ref().unwrap().ids});
        }
        for id in providers.as_object().unwrap().keys() {
            states[id] = json!({"availability":"available","entitled":true,"current":true});
        }
        if providers.as_object().unwrap().is_empty() {
            return Ok(None);
        }
        let revision = builtin["revision"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| builtin["revision"].to_string());
        Ok(Some(
            json!({"revision":format!("bb-accounts:{}",uuid::Uuid::new_v4()),"basedOnZCodeBuiltinRevision":format!("zcode-builtin:{revision}:{}",hash(path_string(absolute(file)?))),"providers":providers,"states":states}),
        ))
    }
    pub async fn authorize(&mut self, p: &Value, id: &str, cwd: &Path) -> Value {
        match self.authorize_inner(p, id, cwd).await {
            Ok(v) => v,
            Err(e) => json!({"headersApplied":false,"errorMessage":e.message}),
        }
    }
    async fn authorize_inner(&mut self, p: &Value, id: &str, cwd: &Path) -> Result<Value> {
        let fail =
            || Error::new("Mismatched or unavailable account, plan, model, session or workspace");
        let login = self.current()?;
        let access = &p["accountAccess"];
        let model = &p["modelSelection"];
        if p["sessionId"] != id
            || model["providerId"] != p["providerId"]
            || access["type"] != "zhipu-account"
            || access["accountType"] != login.family
        {
            return Err(fail());
        }
        if self.start_ready
            && self
                .start
                .as_ref()
                .is_some_and(|s| s["providerId"] == p["providerId"])
        {
            if access["mode"] != "start-plan"
                || access["entitled"] != true
                || p["reason"] != "model-request"
                || text(&p["requestId"]).is_empty()
                || p["workspace"] != workspace(cwd)
                || p.get("expectedAccountScope").is_some()
                || p.get("rejectedProjectTokenFingerprint").is_some()
            {
                return Err(fail());
            }
            if self.entitlement.as_ref().is_none_or(|e| e.until <= now()) {
                self.balance().await?;
            }
            if !self
                .entitlement
                .as_ref()
                .unwrap()
                .ids
                .iter()
                .any(|id| id == text(&model["modelId"]))
            {
                return Err(Error::new(
                    "Start Plan model balance is unavailable, expired or exhausted",
                ));
            }
            return Ok(json!({"headersApplied":true,"requestAuth":{"apiKey":login.jwt}}));
        }
        let provider = self
            .coding
            .as_ref()
            .filter(|_| self.coding_ready)
            .ok_or_else(fail)?;
        let rejected = p.get("rejectedProjectTokenFingerprint").map(text);
        if provider["providerId"] != p["providerId"]
            || access["mode"] != "individual-coding-plan"
            || !list(&provider["config"]["builtinModelIds"]).contains(&model["modelId"])
            || p.get("expectedAccountScope")
                .is_some_and(|v| text(v) != self.scope)
            || rejected.is_some_and(|r| {
                p["expectedAccountScope"] != self.scope
                    || r.len() != 64
                    || !r
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err(fail());
        }
        let m = self.material(rejected).await?;
        self.current()?;
        Ok(
            json!({"headersApplied":true,"requestAuth":{"apiKey":m.token,"apiKeyId":m.key_id,"accountScope":self.scope}}),
        )
    }
    pub async fn official_mcp(&mut self, p: &Value, cwd: &Path) -> Value {
        let valid = url::Url::parse(text(&p["targetOrigin"]))
            .ok()
            .is_some_and(|u| {
                u.scheme() == "https"
                    && u.host_str() == Some("zcode.z.ai")
                    && u.port_or_known_default() == Some(443)
                    && u.username().is_empty()
                    && u.password().is_none()
                    && u.path() == "/"
                    && u.query().is_none()
                    && u.fragment().is_none()
            });
        if !valid {
            return json!({"ok":false,"reason":"official_mcp_origin_untrusted"});
        }
        if ["requestId", "pluginId", "mcpKey"]
            .iter()
            .any(|k| text(&p[k]).is_empty())
            || p["workspace"] != workspace(cwd)
        {
            return json!({"ok":false,"reason":"official_auth_unavailable"});
        }
        if !self.coding_ready {
            return json!({"ok":false,"reason":"official_auth_plan_required"});
        }
        if self.material(None).await.is_err() {
            return json!({"ok":false,"reason":"official_auth_unavailable"});
        }
        match self.current() {
            Ok(l) if !l.jwt.is_empty() => {
                json!({"ok":true,"headers":{"Authorization":format!("Bearer {}",l.jwt),"X-Bigmodel-Authorization":format!("Bearer {}",l.token),"Bigmodel-Target-Type":"PERSONAL"}})
            }
            _ => json!({"ok":false,"reason":"official_auth_unavailable"}),
        }
    }
    pub async fn coding_usage(&mut self) -> Result<Value> {
        let m = self.material(None).await?;
        self.coding_request("/api/monitor/usage/quota/limit", &m.token, None, false)
            .await
    }
}
fn valid_project(p: &Value) -> bool {
    !text(&p["projectId"]).is_empty() && p["projectType"] != "2" && p["projectType"] != 2
}
fn valid_provider(p: &Value, family: &str, mode: &str) -> bool {
    !text(&p["providerId"]).is_empty()
        && p["config"]["access"]["type"] == "zhipu-account"
        && p["config"]["access"]["mode"] == mode
        && p["config"]["access"]["accountType"] == family
        && p["config"]["builtinModelIds"].is_array()
        && list(&p["config"]["builtinModelIds"])
            .iter()
            .all(|v| !text(v).is_empty())
}
fn printable(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.bytes().all(|c| (0x20..=0x7e).contains(&c))).then(|| s.into())
}
fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

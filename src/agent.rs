use crate::{
    account::Account,
    common::*,
    config::Config,
    models,
    native::Native,
    rpc::{Event, Peer},
    state::{State, valid_id},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, oneshot};
use tokio_util::sync::CancellationToken;

pub struct Agent {
    config: Config,
    pub client: Option<Arc<Peer>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    creating: Mutex<HashSet<String>>,
    closed: AtomicBool,
}
pub struct Session {
    pub id: String,
    cwd: PathBuf,
    pub native: Native,
    pub account: Mutex<Account>,
    snapshot: Mutex<Value>,
    state: Mutex<State>,
    active: Mutex<Option<Arc<Turn>>>,
    seq: AtomicU64,
    closing: AtomicBool,
    cancelling: AtomicBool,
    cancel_operations: Mutex<()>,
    operations: Mutex<()>,
}
struct Turn {
    input: String,
    turn_id: StdMutex<Option<String>>,
    tools: StdMutex<HashSet<String>>,
    emitted: AtomicBool,
    cancelled: AtomicBool,
    cancel: CancellationToken,
    done: StdMutex<Option<oneshot::Sender<Result<Value>>>>,
    sending: AtomicBool,
    sent: Notify,
}
pub(crate) fn mcp_servers(value: &Value) -> Result<Vec<Value>> {
    if !value.is_null() && !value.is_array() {
        return Err(Error::invalid("mcpServers must be an array"));
    }
    Ok(list(value).iter().map(|server| {
        let array = |key: &str| server.get(key).filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!([]));
        if server["type"] == "http" || server["type"] == "sse" {
            json!({"name":server["name"],"type":server["type"],"url":server["url"],"headers":array("headers"),"isolation":"session"})
        } else {
            json!({"name":server["name"],"command":server["command"],"args":array("args"),"env":array("env"),"isolation":"session"})
        }
    }).collect())
}
impl Session {
    async fn finish(&self, result: Result<Value>) {
        if let Some(active) = self.active.lock().await.take() {
            active.cancel.cancel();
            if let Some(tx) = active.done.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        }
    }
    async fn accept(&self, mut changed: Value) -> Result<()> {
        if changed.get("settings").is_none() {
            changed = self
                .native
                .request(
                    "session/read",
                    json!({"sessionId":self.id,"messageLimit":1}),
                )
                .await?;
        }
        let mut snapshot = self.snapshot.lock().await;
        let mut available = list(&snapshot["settings"]["model"]["available"]).to_vec();
        for m in list(&changed["settings"]["model"]["available"]) {
            if let Some(old) = available
                .iter_mut()
                .find(|a| models::model_id(&a["ref"]) == models::model_id(&m["ref"]))
            {
                *old = m.clone();
            } else {
                available.push(m.clone());
            }
        }
        changed["settings"]["model"]["available"] = json!(available);
        *snapshot = changed;
        Ok(())
    }
}
impl Agent {
    pub fn new(config: Config, client: Option<Arc<Peer>>) -> Arc<Self> {
        Arc::new(Self {
            config,
            client,
            sessions: Mutex::new(HashMap::new()),
            creating: Mutex::new(HashSet::new()),
            closed: AtomicBool::new(false),
        })
    }
    fn restricted(&self) -> bool {
        self.config.permission.as_deref() == Some("accept-edits")
    }
    pub async fn handle(self: &Arc<Self>, method: &str, p: Value) -> Result<Value> {
        if method == "initialize" {
            return Ok(
                json!({"protocolVersion":1,"agentInfo":{"name":"zcode-cli-acp","title":"ZCode CLI (ACP)","version":env!("CARGO_PKG_VERSION")},"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":false,"audio":false,"embeddedContext":true},"mcpCapabilities":{"http":true,"sse":true}},"authMethods":[]}),
            );
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::new("Agent is closed"));
        }
        if method == "session/new" || method == "session/load" {
            return self.open(p, method == "session/load").await;
        }
        if ![
            "session/prompt",
            "session/cancel",
            "session/close",
            "session/set_mode",
            "session/set_model",
            "session/set_config_option",
        ]
        .contains(&method)
        {
            return Err(Error {
                code: -32601,
                message: format!("Method not found: {method}"),
            });
        }
        let id = text(&p["sessionId"]);
        let s = self
            .sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| Error::invalid("Unknown native session"))?;
        match method {
            "session/prompt" => self.prompt(s, p).await,
            "session/cancel" => {
                self.cancel(&s).await?;
                Ok(json!({}))
            }
            "session/close" => {
                self.end(&s).await?;
                Ok(json!({}))
            }
            _ => self.configure(&s, method, &p).await,
        }
    }
    async fn open(self: &Arc<Self>, p: Value, resume: bool) -> Result<Value> {
        let cwd = PathBuf::from(
            p["cwd"]
                .as_str()
                .ok_or_else(|| Error::invalid("cwd must be an existing absolute directory"))?,
        );
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err(Error::invalid("cwd must be an existing absolute directory"));
        }
        let cwd = absolute(cwd)?;
        let id = if resume {
            text(&p["sessionId"]).to_owned()
        } else {
            format!("bb-native-{}", uuid::Uuid::new_v4())
        };
        if !valid_id(&id) {
            return Err(Error::invalid(
                "Only sessions created by this adapter can be resumed",
            ));
        }
        {
            let mut creating = self.creating.lock().await;
            if self.sessions.lock().await.contains_key(&id) || !creating.insert(id.clone()) {
                return Err(Error::invalid("Session is already open"));
            }
        }
        let result = self.open_inner(&id, cwd, p, resume).await;
        self.creating.lock().await.remove(&id);
        result
    }
    async fn open_inner(
        self: &Arc<Self>,
        id: &str,
        cwd: PathBuf,
        p: Value,
        resume: bool,
    ) -> Result<Value> {
        let state = State::prepare(id, &cwd, resume, &self.config.env).await?;
        let (native, mut events) = Native::spawn(&cwd, &state.environment(&self.config.env))?;
        let account = Account::new(native.env.clone())?;
        let s = Arc::new(Session {
            id: id.into(),
            cwd,
            native,
            account: Mutex::new(account),
            snapshot: Mutex::new(Value::Null),
            state: Mutex::new(state),
            active: Mutex::new(None),
            seq: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            cancelling: AtomicBool::new(false),
            cancel_operations: Mutex::new(()),
            operations: Mutex::new(()),
        });
        let weak = Arc::downgrade(&s);
        let a = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(e) = events.recv().await {
                let (Some(s), Some(a)) = (weak.upgrade(), a.upgrade()) else {
                    break;
                };
                match e {
                    Event::Closed => {
                        s.finish(Err(Error::new("ZCode app-server connection closed")))
                            .await;
                        break;
                    }
                    Event::Message(m) => {
                        if let Some(id) = m.get("id") {
                            let s = s.clone();
                            let id = id.clone();
                            tokio::spawn(async move {
                                let result = a
                                    .interaction(&s, text(&m["method"]), m["params"].clone())
                                    .await;
                                let _ = match result {
                                    Ok(v) => s.native.peer.reply(id, v).await,
                                    Err(e) => s.native.peer.error(id, e).await,
                                };
                            });
                        } else if m["method"] == "session/event" {
                            a.event(&s, &m["params"]).await;
                        }
                    }
                }
            }
        });
        let result=async{
            if let Some(overlay)=s.account.lock().await.initialize().await?{s.native.request("provider/updateAccountConfig",overlay).await?;}
            let default=if self.config.permission.as_deref()==Some("full"){"yolo"}else if self.restricted(){"edit"}else{"build"};let mode=self.config.env.get("ZCODE_NATIVE_MODE").map(String::as_str).unwrap_or(default);
            if !["build","plan","edit","yolo"].contains(&mode){return Err(Error::invalid("ZCODE_NATIVE_MODE must be build, plan, edit or yolo"));}
            if self.restricted()&&mode=="yolo"{return Err(Error::invalid("Full access requires BB full permissions"));}
            let mut persisted=resume;if resume{let native=s.native.request("session/list",json!({"sessionIds":[id]})).await?;persisted=list(&native["sessions"]).iter().any(|v|v["sessionId"]==id);if !persisted&&s.state.lock().await.saved["inputSubmitted"]!=false{return Err(Error::new("Native history is missing after an input was attempted or its outcome is unknown; no new session was created and nothing was resent"));}}
            let mcp = mcp_servers(&p["mcpServers"])?;
            let mut params=json!({"sessionId":id,"workspace":workspace(&s.cwd),"mcpServers":mcp});if !persisted{
                let saved=s.state.lock().await;let initial=if resume{saved.saved["preferences"]["mode"].as_str().unwrap_or(mode)}else{mode};params["mode"]=json!(if self.restricted()&&initial=="yolo"{"edit"}else{initial});params["persistence"]=json!("immediate");params["titleGenerationEnabled"]=json!(false);if resume&&!saved.saved["preferences"]["model"].is_null(){params["model"]=saved.saved["preferences"]["model"].clone();}
            }
            let snapshot=s.native.request(if persisted{"session/resume"}else{"session/create"},params).await?;
            if snapshot["session"]["sessionId"]!=id{return Err(Error::new("Native server returned a different session identity"));}
            if !snapshot["settings"].is_object(){return Err(Error::new("Invalid native session snapshot"));}
            *s.snapshot.lock().await=snapshot;
            let current={let snapshot=s.snapshot.lock().await;if snapshot["settings"]["model"]["current"].is_null(){list(&snapshot["settings"]["model"]["available"]).iter().find(|m|m.get("disabledReason").is_none_or(Value::is_null)).cloned()}else{None}};
            if let Some(model)=current{s.accept(s.native.request("session/setModel",json!({"sessionId":id,"model":models::selection(&model)})).await?).await?;}
            let unsafe_mode=self.restricted()&&s.snapshot.lock().await["settings"]["mode"]["current"]=="yolo";if unsafe_mode{s.accept(s.native.request("session/setMode",json!({"sessionId":id,"mode":"edit"})).await?).await?;}
            {let snapshot=s.snapshot.lock().await;let mut state=s.state.lock().await;state.save(json!({"ready":true}))?;state.preferences(&snapshot)?;}
            if self.closed.load(Ordering::Acquire){return Err(Error::new("Agent is closed"));}self.sessions.lock().await.insert(id.into(),s.clone());
            let subscribed=s.native.request("session/subscribe",json!({"sessionId":id,"deliveryKind":"desktop-continuous","includeSnapshot":false})).await?;s.seq.store(subscribed["eventSeq"].as_u64().unwrap_or(0),Ordering::Release);
            if resume{let snapshot=s.snapshot.lock().await;for m in list(&snapshot["messages"]){if m["info"]["role"]!="user"&&m["info"]["role"]!="assistant"{continue;}for part in list(&m["parts"]){if part["type"]=="text"||part["type"]=="reasoning"{let kind=if part["type"]=="reasoning"{"agent_thought_chunk"}else if m["info"]["role"]=="user"{"user_message_chunk"}else{"agent_message_chunk"};self.update(id,json!({"sessionUpdate":kind,"content":{"type":"text","text":part["text"]}})).await;}}}}
            let mut response=models::state(&*s.snapshot.lock().await,self.restricted());if !resume{response["sessionId"]=json!(id);}Ok(response)
        }.await;
        if result.is_err() {
            self.sessions.lock().await.remove(id);
            s.native.close().await;
        }
        result
    }
    async fn update(&self, id: &str, update: Value) {
        if let Some(p) = &self.client {
            let _ = p
                .send(json!({"method":"session/update","params":{"sessionId":id,"update":update}}))
                .await;
        }
    }
    async fn interaction(&self, s: &Session, method: &str, p: Value) -> Result<Value> {
        if p.get("sessionId").is_some_and(|v| v != &s.id) {
            return Err(Error::invalid("Interaction belongs to another session"));
        }
        match method {
            "session/requestRuntimePreferences" => Ok(
                json!({"nativeSearchEnhancementsEnabled":false,"toolSearchMode":"off","subagentRuntimeConfigEnabled":true,"memoryEnabled":self.config.env.get("ZCODE_NATIVE_MEMORY").is_some_and(|s|s=="1"),"askUserQuestionAutoResolutionEnabled":false,"modelContextBudgetStrategy":"preflight-v1"}),
            ),
            "subagents/readRuntimeConfig" => Ok(json!({"kind":"built-in-fallback"})),
            "interaction/requestProviderRuntimeHeaders" => {
                Ok(s.account.lock().await.authorize(&p, &s.id, &s.cwd).await)
            }
            "interaction/requestOfficialMcpAuthHeaders" => {
                Ok(s.account.lock().await.official_mcp(&p, &s.cwd).await)
            }
            "interaction/requestPermission" => {
                let active = s.active.lock().await.clone();
                let Some(active) = active.filter(|_| !s.closing.load(Ordering::Acquire)) else {
                    return Ok(json!({"decision":"deny","reason":"No active client turn"}));
                };
                let Some(client) = &self.client else {
                    return Ok(json!({"decision":"deny","reason":"No interactive client"}));
                };
                let options: Vec<_> = list(&p["options"])
                    .iter()
                    .filter(|o| o["kind"] == "allow_once" || o["kind"] == "deny")
                    .cloned()
                    .collect();
                let params = json!({"sessionId":s.id,"toolCall":{"toolCallId":p["toolCallId"],"title":p["toolName"],"status":"pending","rawInput":p["input"]},"options":options.iter().map(|o|json!({"optionId":o["optionId"],"name":o["name"],"kind":if o["kind"]=="deny"{"reject_once"}else{"allow_once"}})).collect::<Vec<_>>()});
                let allowed = match client
                    .request(
                        "session/request_permission",
                        params,
                        Duration::from_secs(600),
                        Some(&active.cancel),
                    )
                    .await
                {
                    Ok(v) => {
                        v["outcome"]["outcome"] == "selected"
                            && options.iter().any(|o| {
                                o["optionId"] == v["outcome"]["optionId"]
                                    && o["kind"] == "allow_once"
                            })
                    }
                    Err(_) => false,
                };
                Ok(
                    json!({"decision":if allowed{"allow"}else{"deny"},"reason":"ACP client permission decision"}),
                )
            }
            _ => Err(Error {
                code: -32601,
                message: format!("Unsupported ZCode interaction: {method}"),
            }),
        }
    }
    async fn event(&self, s: &Session, e: &Value) {
        if e["sessionId"] != s.id {
            return;
        }
        let Some(active) = s.active.lock().await.clone() else {
            return;
        };
        if let Some(seq) = e["seq"].as_u64() {
            let prior = s.seq.load(Ordering::Acquire);
            if seq <= prior {
                return;
            }
            s.seq.store(seq, Ordering::Release);
        }
        let p = &e["payload"];
        {
            let mut turn = active.turn_id.lock().unwrap();
            if !e["turnId"].is_null() && turn.as_ref().is_some_and(|id| id != text(&e["turnId"])) {
                return;
            }
            if e["type"] == "turn.started" {
                *turn = e["turnId"].as_str().map(str::to_owned);
            }
        }
        if p.get("inputId").is_some_and(|v| v != &active.input) {
            return;
        }
        match text(&e["type"]) {
            "model.streaming" => match text(&p["kind"]) {
                "text_delta" => self.chunk(s, &active, "text", text(&p["delta"])).await,
                "reasoning_delta" => self.chunk(s, &active, "reasoning", text(&p["delta"])).await,
                _ => {}
            },
            "tool.updated" if !text(&p["toolCallId"]).is_empty() => {
                let first = active
                    .tools
                    .lock()
                    .unwrap()
                    .insert(text(&p["toolCallId"]).to_owned());
                let status = match text(&p["kind"]) {
                    "result" => "completed",
                    "error" => "failed",
                    "scheduled" => "pending",
                    _ => "in_progress",
                };
                let mut update = json!({"sessionUpdate":if first{"tool_call"}else{"tool_call_update"},"toolCallId":p["toolCallId"],"status":status});
                if first {
                    update["title"] = json!(p["toolName"].as_str().unwrap_or("ZCode tool"));
                    update["kind"] = json!("other");
                }
                if let Some(v) = p.get("input") {
                    update["rawInput"] = v.clone();
                }
                if let Some(v) = p
                    .get("result")
                    .filter(|v| !v.is_null())
                    .or_else(|| p.get("error"))
                {
                    update["rawOutput"] = v.clone();
                }
                self.update(&s.id, update).await;
            }
            "turn.completed" => {
                if !active.emitted.load(Ordering::Acquire) {
                    self.chunk(s, &active, "text", text(&p["response"])).await;
                }
                let result = match text(&p["resultType"]) {
                    "" | "success" | "cancelled" => Ok(
                        json!({"stopReason":if p["resultType"]=="cancelled"||active.cancelled.load(Ordering::Acquire){"cancelled"}else{"end_turn"}}),
                    ),
                    _ => Err(Error::new("ZCode turn ended without success")),
                };
                s.finish(result).await;
            }
            "turn.failed" => s.finish(Err(Error::native(-32603))).await,
            "session.closed" => {
                s.finish(Err(Error::new("ZCode session closed during a turn")))
                    .await
            }
            _ => {}
        }
    }
    async fn chunk(&self, s: &Session, t: &Turn, kind: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        if kind == "text" {
            t.emitted.store(true, Ordering::Release);
        }
        self.update(&s.id,json!({"sessionUpdate":if kind=="text"{"agent_message_chunk"}else{"agent_thought_chunk"},"content":{"type":"text","text":text}})).await;
    }
    async fn prompt(&self, s: Arc<Session>, p: Value) -> Result<Value> {
        let guard = s.operations.lock().await;
        if s.closing.load(Ordering::Acquire)
            || s.cancelling.load(Ordering::Acquire)
            || s.active.lock().await.is_some()
        {
            return Err(Error::invalid(
                "A turn or session shutdown is already in progress",
            ));
        }
        let blocks = p["prompt"]
            .as_array()
            .ok_or_else(|| Error::invalid("prompt must contain text blocks"))?;
        let mut contents = vec![];
        for b in blocks {
            let content = match text(&b["type"]) {
                "text" => b["text"].as_str(),
                "resource" => b["resource"]["text"].as_str(),
                _ => None,
            }
            .ok_or_else(|| Error::invalid("Unsupported prompt block"))?;
            contents.push(content);
        }
        let content = contents.join("\n");
        if content.trim().is_empty() {
            return Err(Error::invalid("Prompt is empty"));
        }
        {
            let snap = s.snapshot.lock().await;
            if snap["settings"]["model"]["current"].is_null()
                && list(&snap["settings"]["model"]["available"]).is_empty()
            {
                return Err(Error::new(
                    "No available model. Sign in to a supported plan in ZCode or configure a personal API provider",
                ));
            }
        }
        let (tx, rx) = oneshot::channel();
        let active = Arc::new(Turn {
            input: uuid::Uuid::new_v4().to_string(),
            turn_id: StdMutex::new(None),
            tools: StdMutex::new(HashSet::new()),
            emitted: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            done: StdMutex::new(Some(tx)),
            sending: AtomicBool::new(true),
            sent: Notify::new(),
        });
        // Durable checkpoint precedes the request. An unknown delivery outcome
        // must never become an automatic cold restore or a resent prompt.
        s.state.lock().await.save(json!({"inputSubmitted":true}))?;
        *s.active.lock().await = Some(active.clone());
        drop(guard);
        let send = s
            .native
            .request(
                "session/send",
                json!({"sessionId":s.id,"content":content,"inputId":active.input}),
            )
            .await;
        active.sending.store(false, Ordering::Release);
        active.sent.notify_waiters();
        if let Err(e) = send {
            s.finish(Err(e.clone())).await;
            return Err(e);
        }
        tokio::select! {r=rx=>r.map_err(|_|Error::new("Turn result unavailable"))?,_ = tokio::time::sleep(Duration::from_secs(4*60*60))=>{let _=self.cancel(&s).await;Err(Error::new("ZCode turn timed out"))}}
    }
    async fn cancel(&self, s: &Session) -> Result<()> {
        let _guard = s.cancel_operations.lock().await;
        s.cancelling.store(true, Ordering::Release);
        let result = async {
            let Some(active) = s.active.lock().await.clone() else {
                return Ok(());
            };
            active.cancelled.store(true, Ordering::Release);
            active.cancel.cancel();
            loop {
                let sent = active.sent.notified();
                if !active.sending.load(Ordering::Acquire) {
                    break;
                }
                sent.await;
            }
            if let Err(e) = s
                .native
                .request("session/stop", json!({"sessionId":s.id}))
                .await
            {
                s.finish(Err(e.clone())).await;
                s.native.close().await;
                return Err(e);
            }
            s.finish(Ok(json!({"stopReason":"cancelled"}))).await;
            Ok(())
        }
        .await;
        s.cancelling.store(false, Ordering::Release);
        result
    }
    async fn configure(&self, s: &Session, method: &str, p: &Value) -> Result<Value> {
        let _guard = s.operations.lock().await;
        if s.active.lock().await.is_some()
            || s.closing.load(Ordering::Acquire)
            || s.cancelling.load(Ordering::Acquire)
        {
            return Err(Error::invalid(
                "Cannot change configuration during a turn or shutdown",
            ));
        }
        let (method_native, params) = if method == "session/set_mode" {
            let mode = text(&p["modeId"]);
            if !["build", "plan", "edit", "yolo"].contains(&mode) {
                return Err(Error::invalid("Unknown mode"));
            }
            if self.restricted() && mode == "yolo" {
                return Err(Error::invalid("Full access requires BB full permissions"));
            }
            ("session/setMode", json!({"sessionId":s.id,"mode":mode}))
        } else if method == "session/set_model" || p["configId"] == "model" {
            let selected = p.get("modelId").unwrap_or(&p["value"]);
            let snapshot = s.snapshot.lock().await;
            let model = list(&snapshot["settings"]["model"]["available"])
                .iter()
                .find(|m| {
                    models::model_id(&m["ref"]) == text(selected)
                        && m.get("disabledReason").is_none_or(Value::is_null)
                })
                .ok_or_else(|| Error::invalid("Unknown model"))?;
            (
                "session/setModel",
                json!({"sessionId":s.id,"model":models::selection(model)}),
            )
        } else if p["configId"] == "effort" {
            let snap = s.snapshot.lock().await;
            let (levels, _, selection) = models::reasoning(&snap);
            if !levels.iter().any(|v| v["value"] == p["value"]) {
                return Err(Error::invalid("Unknown reasoning level"));
            }
            if selection {
                let mut model = snap["settings"]["model"]["current"].clone();
                model["options"] = json!({"reasoningLevel":p["value"]});
                ("session/setModel", json!({"sessionId":s.id,"model":model}))
            } else {
                (
                    "session/setThoughtLevel",
                    json!({"sessionId":s.id,"thoughtLevel":p["value"]}),
                )
            }
        } else {
            return Err(Error::invalid("Unknown configuration option"));
        };
        s.accept(s.native.request(method_native, params).await?)
            .await?;
        let snap = s.snapshot.lock().await;
        s.state.lock().await.preferences(&snap)?;
        Ok(if method == "session/set_config_option" {
            json!({"configOptions":models::state(&snap,self.restricted())["configOptions"]})
        } else {
            json!({})
        })
    }
    async fn end(&self, s: &Session) -> Result<()> {
        let _guard = s.operations.lock().await;
        if s.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let result = async {
            self.cancel(s).await?;
            s.native
                .request("session/close", json!({"sessionId":s.id}))
                .await?;
            Ok(())
        }
        .await;
        s.native.close().await;
        self.sessions.lock().await.remove(&s.id);
        result
    }
    pub async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let sessions: Vec<_> = self.sessions.lock().await.values().cloned().collect();
        for s in sessions {
            let _ = self.end(&s).await;
        }
    }
    pub async fn doctor(self: &Arc<Self>) -> Result<Value> {
        let r = self
            .handle(
                "session/new",
                json!({"cwd":path_string(std::env::current_dir()?),"mcpServers":[]}),
            )
            .await?;
        let s = self
            .sessions
            .lock()
            .await
            .get(text(&r["sessionId"]))
            .cloned()
            .unwrap();
        let account = s.account.lock().await;
        let models: Vec<_> = list(&r["models"]["availableModels"])
            .iter()
            .map(|m| json!({"id":m["modelId"],"name":m["name"]}))
            .collect();
        Ok(
            json!({"schemaVersion":1,"ok":!models.is_empty(),"platform":platform(),"adapterRuntime":"rust","desktopVersion":s.native.env.get("ZCODE_APP_VERSION"),"nativeRuntime":s.native.runtime.info(),"transport":"ACP -> ZCode app-server stdio","desktopCodingPlanVerified":account.coding_ready,"planLabel":account.plan_label,"startPlanVerified":account.start_ready,"accountDiagnostics":account.diagnostics,"models":models,"permissions":"plan","sessionStorage":"separate-from-desktop","modelRequestsSent":0,"generationVerified":false}),
        )
    }
}

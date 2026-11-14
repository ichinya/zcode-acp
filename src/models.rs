use crate::common::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
pub fn model_id(r: &Value) -> String {
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({"providerId":r["providerId"],"modelId":r["modelId"]})).unwrap(),
    )
}
pub fn selection(m: &Value) -> Value {
    let mut r = m["ref"].clone();
    let levels = list(&m["reasoning"]["levels"]);
    if levels.is_empty() {
        return r;
    }
    let selected = m["ref"]["options"]
        .get("reasoningLevel")
        .unwrap_or(&m["reasoning"]["defaultLevel"]);
    let level = levels
        .iter()
        .find(|l| l["value"] == *selected)
        .unwrap_or(&levels[0]);
    r["options"] = json!({"reasoningLevel":level["value"]});
    r
}
fn source(m: &Value) -> (u8, String) {
    let id = text(&m["ref"]["providerId"]);
    if let Some(v) = id.strip_prefix("account:") {
        for family in ["zai", "bigmodel"] {
            if let Some(plan) = v.strip_prefix(&format!("{family}-")) {
                let p = match plan {
                    "individual-coding-plan" => Some((0, "Coding Plan")),
                    "start-plan" => Some((1, "Start Plan")),
                    "team-coding-plan" => Some((2, "Team Coding Plan")),
                    "offpeak-idle-plan" => Some((3, "Off-peak Plan")),
                    "highspeed-card" => Some((4, "Highspeed Card")),
                    _ => None,
                };
                if let Some((rank, name)) = p {
                    return (
                        rank,
                        format!(
                            "{name} · {}",
                            if family == "zai" { "Z.ai" } else { "BigModel" }
                        ),
                    );
                }
            }
        }
    }
    (
        10,
        m.get("providerLabel")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(id)
            .into(),
    )
}
pub fn catalog(available: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let mut groups: Vec<(String, u8, String, Vec<&Value>)> = vec![];
    for m in available
        .iter()
        .filter(|m| m.get("disabledReason").is_none_or(Value::is_null))
    {
        let id = text(&m["ref"]["providerId"]);
        if let Some(g) = groups.iter_mut().find(|g| g.0 == id) {
            g.3.push(m);
        } else {
            let (rank, name) = source(m);
            groups.push((id.into(), rank, name, vec![m]));
        }
    }
    groups.sort_by_key(|g| g.1);
    let mut models = vec![];
    let mut options = vec![];
    for g in &groups {
        let name = if groups.iter().filter(|x| x.2 == g.2).count() > 1 {
            format!("{} ({})", g.2, g.0)
        } else {
            g.2.clone()
        };
        let mut choices = vec![];
        for m in &g.3 {
            let id = model_id(&m["ref"]);
            let label = format!(
                "{name} / {}",
                m.get("label")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(text(&m["ref"]["modelId"]))
            );
            models.push(json!({"modelId":id,"name":label,"description":format!("Source: {name}")}));
            choices.push(json!({"value":id,"name":label,"description":format!("Source: {name}")}));
        }
        options.push(json!({"group":g.0,"name":name,"options":choices}));
    }
    (models, options)
}
pub fn reasoning(snapshot: &Value) -> (Vec<Value>, Value, bool) {
    let settings = &snapshot["settings"];
    let current = &settings["model"]["current"];
    let model = list(&settings["model"]["available"])
        .iter()
        .find(|m| model_id(&m["ref"]) == model_id(current));
    let mut levels = model
        .map(|m| list(&m["reasoning"]["levels"]).to_vec())
        .unwrap_or_default();
    let selection = !levels.is_empty();
    if levels.is_empty() && settings["thoughtLevel"]["enabled"] == true {
        levels = list(&settings["thoughtLevel"]["available"]).to_vec();
    }
    let fallback = Value::Null;
    let value = [
        &current["options"]["reasoningLevel"],
        &settings["thoughtLevel"]["current"],
        model
            .map(|m| &m["reasoning"]["defaultLevel"])
            .unwrap_or(&fallback),
        &settings["thoughtLevel"]["defaultLevel"],
        levels.first().map(|l| &l["value"]).unwrap_or(&fallback),
    ]
    .into_iter()
    .find(|v| !v.is_null())
    .cloned()
    .unwrap_or(Value::Null);
    (levels, value, selection)
}
pub fn state(snapshot: &Value, restricted: bool) -> Value {
    let s = &snapshot["settings"];
    let available: Vec<_> = list(&s["model"]["available"])
        .iter()
        .filter(|m| m.get("disabledReason").is_none_or(Value::is_null))
        .cloned()
        .collect();
    let (models, options) = catalog(&available);
    let mut modes = vec![
        json!({"id":"build","name":"Ask before changes"}),
        json!({"id":"plan","name":"Plan"}),
        json!({"id":"edit","name":"Accept edits"}),
    ];
    if !restricted {
        modes.push(json!({"id":"yolo","name":"Full access"}));
    }
    let mut r = json!({"modes":{"currentModeId":s["mode"]["current"],"availableModes":modes}});
    let mut config = vec![];
    if !available.is_empty() {
        let current = if s["model"]["current"].is_null() {
            model_id(&available[0]["ref"])
        } else {
            model_id(&s["model"]["current"])
        };
        r["models"] = json!({"currentModelId":current,"availableModels":models});
        config.push(json!({"id":"model","name":"Model","category":"model","type":"select","currentValue":current,"options":options}));
    }
    let (levels, current, _) = reasoning(snapshot);
    if !levels.is_empty() {
        config.push(json!({"id":"effort","name":"Reasoning","category":"thought_level","type":"select","currentValue":current,"options":levels.iter().map(|e|json!({"value":e["value"],"name":e["label"]})).collect::<Vec<_>>()}));
    }
    r["configOptions"] = json!(config);
    r
}

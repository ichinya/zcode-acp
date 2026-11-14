use crate::{
    account::{Account, linked, read_login},
    common::*,
    config::{find_entry, native_env},
};
use serde_json::{Value, json};
pub fn base(status: &str, label: &str) -> Value {
    json!({"status":status,"plan":null,"accountEmail":null,"planLabel":label})
}
pub fn failed(label: &str) -> Value {
    let mut r = base("error", label);
    r["message"] = json!("Квоты недоступны. Проверьте вход в ZCode и повторите обновление.");
    r
}
pub fn coding_usage(data: &Value, label: &str) -> Value {
    if !data["limits"].is_array() {
        return failed(label);
    }
    let mut windows = vec![];
    for r in list(&data["limits"]) {
        let model = r["type"] == "TOKENS_LIMIT" || r["type"] == "CREDIT_LIMIT";
        let spec = if model && r["unit"] == 3 && r["number"] == 5 {
            Some(("five-hour", "model-5h", "Модели · 5 часов"))
        } else if model && r["unit"] == 6 {
            Some(("weekly", "model-week", "Модели · неделя"))
        } else if r["type"] == "TIME_LIMIT" && r["unit"] == 5 && r["number"] == 1 {
            Some(("custom", "tools-month", "MCP/tools · месяц"))
        } else {
            None
        };
        let Some((kind, id, name)) = spec else {
            continue;
        };
        if windows.iter().any(|w: &Value| w["id"] == id) {
            continue;
        }
        let Some(percent) = r["percentage"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
        else {
            return failed(label);
        };
        let reset = match r.get("nextResetTime") {
            None | Some(Value::Null) => Value::Null,
            Some(v) => match v.as_f64() {
                Some(n) if n > 0.0 => iso(n),
                _ => Value::Null,
            },
        };
        if number(&r["nextResetTime"]).is_some_and(|n| n > 0.0) && reset.is_null() {
            return failed(label);
        }
        windows.push(json!({"kind":kind,"id":id,"label":name,"usedPercent":percent,"resetsAt":reset,"model":null,"cost":null}));
    }
    windows.sort_by_key(|w| {
        ["model-5h", "model-week", "tools-month"]
            .iter()
            .position(|id| w["id"] == *id)
    });
    if windows.is_empty() {
        failed(label)
    } else {
        let mut r = base("ok", label);
        r["windows"] = json!(windows);
        r
    }
}
pub fn start_usage(data: &Value, clock: f64, date: Option<f64>) -> Value {
    if !data["plans"].is_array() || !data["balances"].is_array() {
        return failed("Start Plan");
    }
    let server = date
        .or_else(|| epoch(&data["server_time"]))
        .unwrap_or(clock);
    let mut windows = vec![];
    let mut expired = false;
    let mut pending = false;
    for (index, b) in list(&data["balances"]).iter().enumerate() {
        let plans: Vec<_> = list(&data["plans"])
            .iter()
            .filter(|p| linked(b, p))
            .collect();
        if plans.is_empty() {
            continue;
        }
        let active: Vec<_> = plans
            .iter()
            .copied()
            .filter(|p| text(&p["status"]).eq_ignore_ascii_case("active"))
            .collect();
        if active.is_empty() {
            expired |= plans
                .iter()
                .any(|p| text(&p["status"]).eq_ignore_ascii_case("expired"));
            continue;
        }
        let mut starts: Vec<_> = [&b["period_start"], &b["effective_at"]]
            .into_iter()
            .filter_map(epoch)
            .collect();
        let mut ends: Vec<_> = [&b["period_end"], &b["expires_at"]]
            .into_iter()
            .filter_map(epoch)
            .collect();
        for p in active {
            starts.extend(epoch(&p["starts_at"]));
            ends.extend(epoch(&p["ends_at"]));
            for e in list(&p["entitlements"])
                .iter()
                .filter(|e| e["entitlement_id"] == b["entitlement_id"])
            {
                starts.extend(epoch(&e["effective_at"]));
            }
        }
        if ends.iter().any(|t| *t <= server) {
            expired = true;
            continue;
        }
        if starts.iter().any(|t| *t > server) {
            pending = true;
            continue;
        }
        if b["unit_type"] != "token" || b["meter"] != "model_usage" {
            continue;
        }
        let Some(total) = number(&b["total_units"]).filter(|n| *n > 0.0) else {
            return failed("Start Plan");
        };
        let Some(used) = number(&b["used_units"]) else {
            return failed("Start Plan");
        };
        let Some(remaining) = number(&b["remaining_units"]) else {
            return failed("Start Plan");
        };
        let Some(available) = number(
            b.get("available_units")
                .filter(|v| !v.is_null())
                .unwrap_or(&b["remaining_units"]),
        ) else {
            return failed("Start Plan");
        };
        if used > total || remaining > total || available > total {
            return failed("Start Plan");
        }
        let models: Vec<_> = list(&b["capabilities"])
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| s.to_ascii_lowercase().starts_with("model:"))
            .map(|s| s[6..].to_owned())
            .collect();
        let model = models.join(", ");
        let end = ends.into_iter().reduce(f64::min);
        let mut label = format!("{} / {} токенов", amount(available), amount(total));
        if !model.is_empty() {
            label += &format!(" · {model}");
        }
        if let Some(t) = end {
            let s = iso(t);
            let s = text(&s);
            if s.len() >= 16 {
                label += &format!(" · до {} UTC", s[..16].replace('T', " "));
            }
        }
        windows.push(json!({"kind":"custom","id":format!("start-{index}"),"label":label,"usedPercent":(used/total*10000.0).round()/100.0,"resetsAt":end.map(iso).unwrap_or(Value::Null),"model":if model.is_empty(){Value::Null}else{json!(model)},"cost":null}));
    }
    let label = if !windows.is_empty() {
        "Start Plan"
    } else if pending {
        "Start Plan · ещё не активен"
    } else if expired {
        "Start Plan · срок истёк"
    } else {
        "Start Plan · нет активного баланса"
    };
    let mut r = base("ok", label);
    r["windows"] = json!(windows);
    r
}
fn amount(n: f64) -> String {
    let s = format!("{n:.0}");
    let mut r = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            r.push('\u{a0}');
        }
        r.push(c);
    }
    r
}
pub async fn collect(plan: &str, env: &Env) -> Value {
    let label = if plan == "start" {
        "Start Plan"
    } else {
        "Coding Plan"
    };
    let Ok(entry) = find_entry(env) else {
        return base("not_installed", label);
    };
    let env = native_env(&entry, env);
    let login = match read_login(&env) {
        Ok(Some(l)) => l,
        Ok(None) => return base("unauthenticated", label),
        Err(_) => return failed(label),
    };
    if plan == "start" && login.jwt.is_empty() || plan == "coding" && login.token.is_empty() {
        return base("unauthenticated", label);
    }
    let run = async {
        let file = env
            .get("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE")
            .ok_or_else(|| Error::new("Catalog unavailable"))?;
        let builtin = read_json(file)?;
        let mut account = Account::new(env.clone())?;
        if plan == "start" {
            account.init_start(&builtin).await?;
            Ok(start_usage(
                account
                    .balance_data
                    .as_ref()
                    .ok_or_else(|| Error::new("Start Plan unavailable"))?,
                now(),
                account.server_date,
            ))
        } else {
            account.init_coding(&builtin).await?;
            if !account.coding_ready {
                return Err(Error::new("Coding Plan unavailable"));
            }
            let data = account.coding_usage().await?;
            Ok(coding_usage(
                &data,
                account.plan_label.as_deref().unwrap_or(label),
            ))
        }
    };
    match run.await {
        Ok(v) => v,
        Err(error) => {
            let mut result = failed(label);
            result["message"] = json!(format!("Квоты недоступны: {}", error.message));
            result
        }
    }
}

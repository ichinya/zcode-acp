use crate::common::*;
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Config {
    pub env: Env,
    pub permission: Option<String>,
    pub command: String,
    pub plan: Option<String>,
}
impl Config {
    pub fn parse(args: &[String], mut env: Env) -> Result<Self> {
        let mut command = "acp".to_string();
        let mut plan = None;
        let mut permission = None;
        let mut bb = None;
        let mut seen = std::collections::HashSet::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--json" || a == "--stdio" {
                i += 1;
                continue;
            }
            if [
                "--help",
                "-h",
                "--version",
                "--doctor",
                "doctor",
                "inventory",
                "account-scope",
                "usage",
                "runtime-info",
            ]
            .contains(&a.as_str())
            {
                if command != "acp" {
                    return Err(Error::invalid("Only one command is allowed"));
                }
                command = a.trim_start_matches('-').to_owned();
                i += 1;
                continue;
            }
            let (flag, inline) = a
                .split_once('=')
                .map(|(a, b)| (a, Some(b)))
                .unwrap_or((a.as_str(), None));
            if ![
                "--native-entry",
                "--node",
                "--plan",
                "--permission",
                "--bb-permission",
            ]
            .contains(&flag)
            {
                return Err(Error::invalid(format!("Unknown argument: {flag}")));
            }
            if !seen.insert(flag.to_string()) {
                return Err(Error::invalid(format!("Duplicate option: {flag}")));
            }
            let value = if let Some(v) = inline {
                v.to_owned()
            } else {
                i += 1;
                args.get(i)
                    .filter(|v| !v.starts_with("--"))
                    .ok_or_else(|| Error::invalid(format!("Missing value for {flag}")))?
                    .clone()
            };
            if value.trim().is_empty() || value.contains('\0') {
                return Err(Error::invalid(format!("Invalid value for {flag}")));
            }
            match flag {
                "--native-entry" => {
                    env.insert("ZCODE_NATIVE_ENTRY".into(), value);
                }
                "--node" => {
                    env.insert("ZCODE_NODE".into(), value);
                }
                "--plan" => plan = Some(value),
                "--permission" => permission = Some(value),
                "--bb-permission" => bb = Some(value),
                _ => unreachable!(),
            };
            i += 1;
        }
        permission = bb.or(permission);
        if permission
            .as_ref()
            .is_some_and(|p| p != "full" && p != "accept-edits")
        {
            return Err(Error::invalid("Permission must be full or accept-edits"));
        }
        if command == "usage" && !matches!(plan.as_deref(), Some("coding" | "start")) {
            return Err(Error::invalid("usage requires --plan coding|start"));
        }
        Ok(Self {
            env,
            permission,
            command,
            plan,
        })
    }
}
pub fn find_entry(env: &Env) -> Result<PathBuf> {
    let mut candidates = vec![];
    if let Some(p) = env.get("ZCODE_NATIVE_ENTRY").filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(p));
    } else if cfg!(windows) {
        candidates.push(
            env.get("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join("AppData/Local"))
                .join("Programs/ZCode/resources/glm/zcode.cjs"),
        );
        for k in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = env.get(k) {
                candidates.push(PathBuf::from(p).join("ZCode/resources/glm/zcode.cjs"));
            }
        }
    } else {
        candidates.extend([
            PathBuf::from("/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs"),
            home().join("Applications/ZCode.app/Contents/Resources/glm/zcode.cjs"),
        ]);
    }
    candidates.into_iter().find(|p|p.is_file()).map(absolute).transpose()?.ok_or_else(||Error::new("ZCode bundled CLI not found. Set --native-entry or ZCODE_NATIVE_ENTRY to resources/glm/zcode.cjs"))
}
pub fn native_env(entry: &Path, env: &Env) -> Env {
    let mut result = env.clone();
    result.remove("ELECTRON_RUN_AS_NODE");
    let resources = entry.parent().unwrap_or(entry).parent().unwrap_or(entry);
    let builtin = resources.join("config/provider/zcode-builtin.json");
    if !result.contains_key("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE") && builtin.is_file() {
        result.insert(
            "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE".into(),
            path_string(builtin),
        );
    }
    result
        .entry("ZCODE_PERSONAL_PROVIDER_CONFIG_FILE".into())
        .or_insert_with(|| path_string(storage(env).join("provider_config.json")));
    if !result.contains_key("ZCODE_APP_VERSION") {
        result.insert("ZCODE_APP_VERSION".into(), desktop_version(resources));
    }
    result
}
fn desktop_version(resources: &Path) -> String {
    let packed = (|| -> Option<String> {
        let mut f = std::fs::File::open(resources.join("app.asar")).ok()?;
        let mut prefix = [0; 16];
        f.read_exact(&mut prefix).ok()?;
        let size = u32::from_le_bytes(prefix[12..16].try_into().ok()?) as usize;
        let offset = 8 + u32::from_le_bytes(prefix[4..8].try_into().ok()?) as u64;
        if !(2..=64 * 1024 * 1024).contains(&size) || offset < (size + 16) as u64 {
            return None;
        }
        let mut buf = vec![0; size];
        f.read_exact(&mut buf).ok()?;
        let header: Value = serde_json::from_slice(&buf).ok()?;
        let p = &header["files"]["package.json"];
        let n = p["size"].as_u64()?;
        if !(2..=1024 * 1024).contains(&n) || p["unpacked"] == true {
            return None;
        }
        let pos = text(&p["offset"]).parse::<u64>().ok()?;
        f.seek(SeekFrom::Start(offset.checked_add(pos)?)).ok()?;
        let mut content = vec![0; n as usize];
        f.read_exact(&mut content).ok()?;
        let v: Value = serde_json::from_slice(&content).ok()?;
        Some(text(&v["version"]).to_owned())
    })();
    if let Some(v) = packed {
        return v;
    }
    let plist = std::fs::read_to_string(resources.parent().unwrap_or(resources).join("Info.plist"))
        .unwrap_or_default();
    plist
        .split("<key>CFBundleShortVersionString</key>")
        .nth(1)
        .and_then(|s| s.split("<string>").nth(1))
        .and_then(|s| s.split("</string>").next())
        .unwrap_or("")
        .to_owned()
}
pub fn inventory(env: &Env) -> Value {
    match find_entry(env) {
        Ok(entry) => {
            json!({"schemaVersion":1,"installed":true,"nativeEntry":path_string(&entry),"desktopVersion":native_env(&entry,env).get("ZCODE_APP_VERSION"),"nativeRuntime":crate::runtime::Runtime::resolve(&entry,env).info()})
        }
        Err(_) => json!({"schemaVersion":1,"installed":false}),
    }
}

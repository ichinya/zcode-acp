use crate::common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The executable belongs to the selected CLI's bundle, never another detected
/// installation. Explicit overrides remain authoritative, including failures.
#[derive(Clone, Debug)]
pub struct Runtime {
    pub executable: PathBuf,
    pub electron: bool,
    pub source: &'static str,
}
impl Runtime {
    pub fn resolve(entry: &Path, env: &Env) -> Self {
        Self::for_platform(entry, env, platform())
    }

    pub(crate) fn for_platform(entry: &Path, env: &Env, os: &str) -> Self {
        let bundled = bundled_executable(entry, os);
        if let Some(explicit) = env.get("ZCODE_NODE").filter(|s| !s.trim().is_empty()) {
            let executable = PathBuf::from(explicit);
            let electron = bundled.as_ref().is_some_and(|candidate| {
                let supplied = absolute(&executable).map(path_string).unwrap_or_default();
                let candidate = path_string(candidate);
                if os == "win32" {
                    supplied.eq_ignore_ascii_case(&candidate)
                } else {
                    supplied == candidate
                }
            });
            return Self {
                executable,
                electron,
                source: "override",
            };
        }
        if let Some(executable) = bundled {
            return Self {
                executable,
                electron: true,
                source: "bundle",
            };
        }
        Self {
            executable: PathBuf::from("node"),
            electron: false,
            source: "path",
        }
    }

    pub fn environment(&self, env: &Env) -> Env {
        let mut child = env.clone();
        child.retain(|key, _| !key.eq_ignore_ascii_case("ELECTRON_RUN_AS_NODE"));
        if self.electron {
            child.insert("ELECTRON_RUN_AS_NODE".into(), "1".into());
        }
        child
    }

    pub fn info(&self) -> Value {
        json!({"kind":if self.electron {"zcode-electron"} else {"node"},
            "executable":path_string(&self.executable),"source":self.source})
    }
}

fn bundled_executable(entry: &Path, os: &str) -> Option<PathBuf> {
    if !["win32", "darwin"].contains(&os) {
        return None;
    }
    let entry = absolute(entry).ok()?;
    let glm = entry.parent()?;
    // ZCode declares its Electron runtime in the distributed CLI metadata.
    // A loose/custom JS file without that declaration continues to use Node.
    let marker = glm.join(".node-bundle-meta.json");
    if std::fs::metadata(&marker).ok()?.len() > 64 * 1024 {
        return None;
    }
    let meta = read_json(&marker).ok()?;
    if meta["runtime"] != "electron-node"
        || text(&meta["entry"]) != entry.file_name()?.to_str()?
        || !text(&meta["platform"]).starts_with(&format!("{os}-"))
    {
        return None;
    }
    let resources = glm.parent()?;
    let executable = if os == "win32" {
        resources.parent()?.join("ZCode.exe")
    } else {
        let contents = resources.parent()?;
        let plist = contents.join("Info.plist");
        let name = std::fs::metadata(&plist)
            .ok()
            .filter(|m| m.len() <= 1024 * 1024)
            .and_then(|_| std::fs::read_to_string(&plist).ok())
            .and_then(|s| plist_string(&s, "CFBundleExecutable").map(str::to_owned))
            .unwrap_or_else(|| "ZCode".into());
        // CFBundleExecutable is a basename. Do not allow a path in metadata.
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
            return None;
        }
        contents.join("MacOS").join(name)
    };
    executable.is_file().then_some(executable)
}

fn plist_string<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let rest = plist
        .split_once(&format!("<key>{key}</key>"))?
        .1
        .trim_start();
    rest.strip_prefix("<string>")?
        .split_once("</string>")
        .map(|(s, _)| s)
}

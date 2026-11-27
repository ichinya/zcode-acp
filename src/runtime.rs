use crate::common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The executable belongs to the selected CLI's bundle, never another detected
/// installation. Explicit overrides remain authoritative, including failures.
#[derive(Clone, Debug)]
pub struct Runtime {
    pub executable: PathBuf,
    /// The signed app's main binary. macOS Keychain items trust only this one,
    /// not the Helper that `executable` prefers to avoid Dock icons.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub main_executable: PathBuf,
    pub electron: bool,
    pub source: &'static str,
}
impl Runtime {
    pub fn resolve(entry: &Path, env: &Env) -> Self {
        Self::for_platform(entry, env, platform())
    }

    pub(crate) fn for_platform(entry: &Path, env: &Env, os: &str) -> Self {
        let main = bundled_executable(entry, os);
        let bundled = main.as_deref().map(|m| helper_executable(m, os));
        if let Some(explicit) = env.get("ZCODE_NODE").filter(|s| !s.trim().is_empty()) {
            let executable = PathBuf::from(explicit);
            let electron = main.as_ref().zip(bundled.as_ref()).is_some_and(|pair| {
                let supplied = absolute(&executable).map(path_string).unwrap_or_default();
                [pair.0, pair.1].into_iter().any(|candidate| {
                    let candidate = path_string(candidate);
                    if os == "win32" {
                        supplied.eq_ignore_ascii_case(&candidate)
                    } else {
                        supplied == candidate
                    }
                })
            });
            return Self {
                main_executable: if electron {
                    main.clone().unwrap_or_else(|| executable.clone())
                } else {
                    executable.clone()
                },
                executable,
                electron,
                source: "override",
            };
        }
        if let Some(executable) = bundled {
            return Self {
                main_executable: main.unwrap_or_else(|| executable.clone()),
                executable,
                electron: true,
                source: "bundle",
            };
        }
        Self {
            executable: PathBuf::from("node"),
            main_executable: PathBuf::from("node"),
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

// A Helper app is LSUIElement, so ELECTRON_RUN_AS_NODE children of it get no
// Dock icon; the main binary would show one per process.
fn helper_executable(main: &Path, os: &str) -> PathBuf {
    if os != "darwin" {
        return main.to_path_buf();
    }
    let helper = (|| {
        let name = main.file_name()?.to_str()?;
        let contents = main.parent()?.parent()?;
        let helper = contents
            .join("Frameworks")
            .join(format!("{name} Helper.app/Contents/MacOS/{name} Helper"));
        helper.is_file().then_some(helper)
    })();
    helper.unwrap_or_else(|| main.to_path_buf())
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

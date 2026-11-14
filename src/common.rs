use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub type Env = HashMap<String, String>;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone)]
pub struct Error {
    pub code: i64,
    pub message: String,
}
impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
    pub fn native(code: i64) -> Self {
        Self {
            code,
            message: format!(
                "ZCode runtime reported an error (code {code}); native diagnostic contents omitted"
            ),
        }
    }
    pub fn wire(&self) -> Value {
        json!({"code": self.code, "message": self.message})
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::new("File or process operation failed")
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::new("Invalid JSON data")
    }
}
pub fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
pub fn list(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn hash(s: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(s.as_ref()))
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}
pub fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}
pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        v => v,
    }
}
pub fn absolute(path: impl AsRef<Path>) -> Result<PathBuf> {
    let p = path.as_ref();
    let absolute = if p.is_absolute() {
        p.to_owned()
    } else {
        std::env::current_dir()?.join(p)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}
pub fn path_string(p: impl AsRef<Path>) -> String {
    let s = p.as_ref().to_string_lossy();
    if cfg!(windows) {
        if let Some(x) = s.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{x}");
        }
        if let Some(x) = s.strip_prefix(r"\\?\") {
            return x.to_string();
        }
    }
    if cfg!(windows) {
        s.replace('/', "\\")
    } else {
        s.into_owned()
    }
}
pub fn workspace(cwd: &Path) -> Value {
    let p = path_string(cwd);
    json!({"workspacePath":p,"workspaceKey":p})
}
pub fn storage(env: &Env) -> PathBuf {
    env.get("ZCODE_STORAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env.get("ZCODE_DATA_BASE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(home)
                .join(".zcode")
        })
        .join("v2")
}
pub fn read_json(p: impl AsRef<Path>) -> Result<Value> {
    let f = std::fs::File::open(p)?;
    if f.metadata()?.len() > 64 * 1024 * 1024 {
        return Err(Error::new("JSON file exceeds size limit"));
    }
    Ok(serde_json::from_reader(f)?)
}
pub fn number(v: &Value) -> Option<f64> {
    let n = v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())?;
    (n.is_finite() && n >= 0.0).then_some(n)
}
pub fn epoch(v: &Value) -> Option<f64> {
    number(v).filter(|n| *n > 0.0).map(|n| n * 1000.0)
}
pub fn iso(ms: f64) -> Value {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|x| json!(x.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
        .unwrap_or(Value::Null)
}
pub fn hide(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    #[cfg(not(windows))]
    let _ = command;
}

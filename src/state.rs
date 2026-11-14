use crate::common::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
pub struct State {
    pub directory: PathBuf,
    pub saved: Value,
}
impl State {
    pub async fn prepare(id: &str, cwd: &Path, resume: bool, env: &Env) -> Result<Self> {
        if !valid_id(id) {
            return Err(Error::invalid(
                "Only sessions created by this adapter can be resumed",
            ));
        }
        let base = env
            .get("ZCODE_ACP_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".zcode-bridge/native-acp"));
        let directory = base.join(id);
        let canonical = path_string(std::fs::canonicalize(cwd)?);
        let saved = if resume {
            let s = read_json(directory.join("binding.json")).map_err(|_| {
                Error::new(
                    "Native session binding is missing or unreadable; no new session was created",
                )
            })?;
            if s["version"] != 1 || s["id"] != id || s["cwd"] != canonical || s["ready"] != true {
                return Err(Error::new(
                    "Native session binding does not match this workspace or creation is incomplete",
                ));
            }
            s
        } else {
            private_directory(&base, true)?;
            private_directory(&directory, false)?;
            json!({"version":1,"id":id,"cwd":canonical,"ready":false,"inputSubmitted":false})
        };
        let meta = std::fs::symlink_metadata(&directory)?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            meta.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if meta.file_type().is_symlink() || reparse || !meta.is_dir() {
            return Err(Error::new(
                "Native ACP session state cannot be a symbolic link or reparse point",
            ));
        }
        protect(&directory).await?;
        let mut state = Self { directory, saved };
        if !resume {
            state.save(json!({}))?;
        }
        Ok(state)
    }
    pub fn environment(&self, env: &Env) -> Env {
        let mut e = env.clone();
        e.insert(
            "ZCODE_SESSION_DB_PATH".into(),
            path_string(self.directory.join("db.sqlite")),
        );
        e.insert(
            "ZCODE_LOG_DIR".into(),
            path_string(self.directory.join("logs")),
        );
        e
    }
    pub fn save(&mut self, patch: Value) -> Result<()> {
        let mut next = self.saved.clone();
        for (k, v) in patch
            .as_object()
            .ok_or_else(|| Error::new("Invalid state update"))?
        {
            next[k] = v.clone();
        }
        let temporary = self
            .directory
            .join(format!("binding-{}.tmp", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&temporary)?;
        f.write_all(serde_json::to_string(&next)?.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&temporary, self.directory.join("binding.json"))?;
        self.saved = next;
        Ok(())
    }
    pub fn preferences(&mut self, snapshot: &Value) -> Result<()> {
        let mut p = json!({"mode":snapshot["settings"]["mode"]["current"]});
        if !snapshot["settings"]["model"]["current"].is_null() {
            p["model"] = snapshot["settings"]["model"]["current"].clone();
        }
        self.save(json!({"preferences":p}))
    }
}
pub fn valid_id(s: &str) -> bool {
    s.strip_prefix("bb-native-")
        .is_some_and(|v| v.len() == 36 && uuid::Uuid::parse_str(v).is_ok())
}
fn private_directory(p: &Path, recursive: bool) -> Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(p)?;
    Ok(())
}
async fn protect(p: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        use base64::Engine;
        // Embedded helper survives relocation of the single binary. The path is
        // an environment value, never interpolated into PowerShell source.
        let script = include_str!("../native/protect-state.ps1");
        let body = script.lines().skip(1).collect::<Vec<_>>().join("\n");
        let body = format!("$Directory = $env:ZCODE_ACP_PROTECT_DIR\n{body}");
        let bytes: Vec<_> = body.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let mut c = tokio::process::Command::new("powershell.exe");
        hide(&mut c);
        c.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .env("ZCODE_ACP_PROTECT_DIR", p)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
        let result = tokio::time::timeout(std::time::Duration::from_secs(15), c.status()).await;
        if !matches!(result,Ok(Ok(s))if s.success()) {
            return Err(Error::new(
                "Cannot protect native session directory with a current-user Windows ACL",
            ));
        }
    }
    Ok(())
}

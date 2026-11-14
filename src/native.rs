use crate::{
    common::*,
    config::{find_entry, native_env},
    rpc::{Event, Peer},
    runtime::Runtime,
};
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    process::Command,
    sync::{Notify, mpsc},
};
use tokio_util::sync::CancellationToken;
pub struct Native {
    pub peer: Arc<Peer>,
    pub env: Env,
    pub runtime: Runtime,
    kill: CancellationToken,
    done: Arc<Notify>,
}
impl Native {
    pub fn spawn(cwd: &Path, env: &Env) -> Result<(Self, mpsc::Receiver<Event>)> {
        let entry = find_entry(env)?;
        let runtime = Runtime::resolve(&entry, env);
        let env = runtime.environment(&native_env(&entry, env));
        let mut command = Command::new(&runtime.executable);
        hide(&mut command);
        command
            .arg(&entry)
            .args(["app-server", "--stdio", "--cwd"])
            .arg(cwd)
            .current_dir(cwd)
            .env_clear()
            .envs(&env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| {
            Error::new(
                "Cannot launch ZCode CLI runtime. Check the ZCode installation or --node / ZCODE_NODE override",
            )
        })?;
        let pid = child.id();
        let output = child.stdout.take().unwrap();
        let input = child.stdin.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        // Drain diagnostics without retaining or forwarding native credentials.
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
        let (peer, rx) = Peer::start(output, input, false);
        let kill = CancellationToken::new();
        let done = Arc::new(Notify::new());
        let k = kill.clone();
        let d = done.clone();
        let p = peer.clone();
        tokio::spawn(async move {
            tokio::select! {_ = child.wait()=>{},_ = k.cancelled()=>{p.fail();if tokio::time::timeout(Duration::from_millis(1500),child.wait()).await.is_err(){
                #[cfg(windows)] if let Some(pid)=pid{let mut c=Command::new("taskkill.exe");hide(&mut c);let _=c.args(["/PID",&pid.to_string(),"/T","/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status().await;}
                #[cfg(not(windows))] let _=pid;
                let _=child.kill().await;let _=child.wait().await;
            }}}
            p.fail();
            d.notify_one();
        });
        Ok((
            Self {
                peer,
                env,
                runtime,
                kill,
                done,
            },
            rx,
        ))
    }
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.peer
            .request(method, params, Duration::from_secs(30), None)
            .await
    }
    pub async fn close(&self) {
        self.kill.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.done.notified()).await;
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.kill.cancel();
    }
}

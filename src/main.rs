mod account;
mod agent;
mod common;
mod config;
mod credentials;
mod models;
mod native;
mod quota;
mod rpc;
mod runtime;
mod state;
use common::*;
use serde_json::json;
use std::time::Duration;
#[cfg(test)]
mod tests;
use config::Config;
#[tokio::main]
async fn main() {
    let result = run().await;
    if let Err(e) = result {
        println!(
            "{}",
            json!({"ok":false,"error":e.message,"modelRequestsSent":0})
        );
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let config = Config::parse(
        &std::env::args().skip(1).collect::<Vec<_>>(),
        std::env::vars().collect(),
    )?;
    match config.command.as_str() {
        "help" | "h" => {
            println!(
                "zcode-acp {} (Rust)\n  zcode-acp                          ACP over stdin/stdout\n  zcode-acp --doctor                 account/model check, no generation\n  zcode-acp inventory --json         local installation check\n  zcode-acp account-scope --json     opaque account cache scope\n  zcode-acp usage --plan coding|start --json\n  zcode-acp runtime-info --json\n  --native-entry PATH               resources/glm/zcode.cjs (overrides ZCODE_NATIVE_ENTRY)\n  --node PATH                       explicit runtime override (or ZCODE_NODE)\n  default runtime                   bundled ZCode Electron, then node from PATH\n  --permission=accept-edits|full\n  --bb-permission=...                compatibility alias",
                env!("CARGO_PKG_VERSION")
            );
        }
        "version" => println!(
            "{}",
            json!({"schemaVersion":1,"name":"zcode-acp","version":env!("CARGO_PKG_VERSION"),"runtime":"rust"})
        ),
        "runtime-info" => println!(
            "{}",
            json!({"schemaVersion":1,"version":env!("CARGO_PKG_VERSION"),"runtime":"rust","cliPath":path_string(std::env::current_exe()?),"revision":option_env!("ZCODE_ACP_BUILD_REVISION").unwrap_or(env!("CARGO_PKG_VERSION"))})
        ),
        "inventory" => println!("{}", config::inventory(&config.env)),
        "account-scope" => println!(
            "{}",
            json!({"schemaVersion":1,"scope":account::account_scope(&config.env)})
        ),
        "usage" => println!(
            "{}",
            quota::collect(config.plan.as_deref().unwrap(), &config.env).await
        ),
        "doctor" => {
            let mut config = config;
            config.env.insert("ZCODE_NATIVE_MODE".into(), "plan".into());
            let agent = agent::Agent::new(config, None);
            let result = agent.doctor().await;
            agent.close().await;
            let r = result?;
            let ok = r["ok"] == true;
            println!("{r}");
            if !ok {
                std::process::exit(1);
            }
        }
        _ => serve(config).await?,
    }
    Ok(())
}
async fn serve(config: Config) -> Result<()> {
    let (peer, mut events) = rpc::Peer::start(tokio::io::stdin(), tokio::io::stdout(), true);
    let agent = agent::Agent::new(config, Some(peer.clone()));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {_ = tokio::signal::ctrl_c()=>break,event=events.recv()=>{let Some(rpc::Event::Message(m))=event else{break};let method=text(&m["method"]).to_owned();let id=m.get("id").cloned();if id.is_none()&&method!="session/cancel"{continue;}let a=agent.clone();let p=peer.clone();tasks.spawn(async move{let result=a.handle(&method,m.get("params").cloned().unwrap_or_else(||json!({}))).await;if let Some(id)=id{let _=match result{Ok(r)=>p.reply(id,r).await,Err(e)=>p.error(id,e).await};}});},Some(_)=tasks.join_next()=>{}}
    }
    // Close also resolves active prompts/permissions. In-flight session creation
    // observes the closed flag and tears down its own process before returning.
    let _ = tokio::time::timeout(Duration::from_secs(35), async {
        agent.close().await;
        while tasks.join_next().await.is_some() {}
        agent.close().await;
    })
    .await;
    tasks.abort_all();
    peer.fail();
    Ok(())
}

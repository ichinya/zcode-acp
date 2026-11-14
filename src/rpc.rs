use crate::common::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

pub const MAX_FRAME: usize = 16 * 1024 * 1024;
pub enum Event {
    Message(Value),
    Closed,
}
type Reply = oneshot::Sender<Result<Value>>;
pub struct Peer {
    out: mpsc::Sender<Value>,
    pending: Mutex<HashMap<String, Reply>>,
    counter: AtomicU64,
    closed: AtomicBool,
    pub stop: CancellationToken,
    acp: bool,
}
impl Peer {
    pub fn start<R, W>(read: R, mut write: W, acp: bool) -> (Arc<Self>, mpsc::Receiver<Event>)
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out, mut output) = mpsc::channel::<Value>(128);
        let (events, rx) = mpsc::channel(256);
        let peer = Arc::new(Self {
            out,
            pending: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            stop: CancellationToken::new(),
            acp,
        });
        let w = peer.clone();
        tokio::spawn(async move {
            loop {
                let m = tokio::select! {_ = w.stop.cancelled()=>break,m=output.recv()=>match m{Some(v)=>v,None=>break}};
                let Ok(mut bytes) = serde_json::to_vec(&m) else {
                    break;
                };
                bytes.push(b'\n');
                if write.write_all(&bytes).await.is_err() || write.flush().await.is_err() {
                    break;
                }
            }
            let _ = write.shutdown().await;
            w.fail();
        });
        let p = peer.clone();
        tokio::spawn(async move {
            let mut input = BufReader::new(read);
            loop {
                let frame = tokio::select! {_ = p.stop.cancelled()=>break,r=read_frame(&mut input,MAX_FRAME)=>r};
                let line = match frame {
                    Ok(Some(b)) => b,
                    Ok(None) => break,
                    Err(_) => break,
                };
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let m: Value = match serde_json::from_slice(&line) {
                    Ok(v) => v,
                    Err(_) => {
                        if acp {
                            let _ = p
                                .error(
                                    Value::Null,
                                    Error {
                                        code: -32700,
                                        message: "Invalid JSON".into(),
                                    },
                                )
                                .await;
                            continue;
                        }
                        break;
                    }
                };
                if !m.is_object() {
                    continue;
                }
                if m.get("method").is_none() {
                    if let Some(id) = m.get("id") {
                        let pending = p.pending.lock().unwrap().remove(&id.to_string());
                        if let Some(tx) = pending {
                            let r = if let Some(e) = m.get("error") {
                                Err(Error::native(e["code"].as_i64().unwrap_or(-32603)))
                            } else {
                                Ok(m["result"].clone())
                            };
                            let _ = tx.send(r);
                        }
                    }
                } else if events.send(Event::Message(m)).await.is_err() {
                    break;
                }
            }
            p.fail();
            let _ = events.send(Event::Closed).await;
        });
        (peer, rx)
    }
    pub async fn send(&self, mut m: Value) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::new("Protocol connection is closed"));
        }
        if self.acp {
            m["jsonrpc"] = json!("2.0");
        }
        self.out
            .send(m)
            .await
            .map_err(|_| Error::new("Protocol connection is closed"))
    }
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value> {
        let id = format!(
            "{}-{}",
            if self.acp { "native-client" } else { "native" },
            self.counter.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(json!(id).to_string(), tx);
        let result=async{self.send(json!({"id":id,"method":method,"params":params})).await?;let never=CancellationToken::new();let token=cancel.unwrap_or(&never);
            tokio::select!{r=rx=>r.map_err(|_|Error::new("Protocol connection closed"))?,_ = tokio::time::sleep(timeout)=>Err(Error::new("Protocol request timed out")),_ = token.cancelled()=>Err(Error::new("Client interaction cancelled")),_ = self.stop.cancelled()=>Err(Error::new("Protocol connection closed"))}
        }.await;
        self.pending.lock().unwrap().remove(&json!(id).to_string());
        result
    }
    pub async fn reply(&self, id: Value, result: Value) -> Result<()> {
        self.send(json!({"id":id,"result":result})).await
    }
    pub async fn error(&self, id: Value, error: Error) -> Result<()> {
        self.send(json!({"id":id,"error":error.wire()})).await
    }
    pub fn fail(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(Error::new("Protocol connection closed")));
        }
        self.stop.cancel();
    }
}
pub async fn read_frame<R: AsyncBufRead + Unpin>(
    input: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let chunk = input.fill_buf().await?;
        if chunk.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(Error::new("Incomplete JSON frame"))
            };
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let n = end.map(|i| i + 1).unwrap_or(chunk.len());
        if bytes.len() + n > max {
            return Err(Error::new("Protocol frame exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk[..n]);
        input.consume(n);
        if end.is_some() {
            return Ok(Some(bytes));
        }
    }
}

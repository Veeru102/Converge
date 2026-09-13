//! End-to-end through the real adapter: two WebSocket clients driven by the
//! reference `ClientSync`, a server restart with the same storage, and the
//! chaos injector.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use converge_client_sync::{ClientConfig, ClientSync, ConnState, Now, Output};
use converge_core::*;
use converge_proto::wire::{decode_server, encode_client};
use converge_proto::{DocId, UserInfo};
use converge_server::{app, Config};
use converge_storage::{MemoryStorage, Storage};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Server {
    addr: String,
    handle: tokio::task::JoinHandle<()>,
}

async fn start(storage: Arc<dyn Storage>) -> Server {
    let cfg = Config {
        bind: "127.0.0.1:0".into(),
        database_url: None,
        persist_window: Duration::from_millis(2),
        snapshot_every: 5,
        retain_ops: 1_000,
        ping_interval: Duration::from_secs(2),
        ..Config::from_env()
    };
    let (router, _state) = app(cfg, storage);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { addr, handle }
}

fn now() -> Now {
    thread_local! { static START: Instant = Instant::now(); }
    Now::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        START.with(|s| s.elapsed().as_millis() as u64),
    )
}

struct Client {
    sync: ClientSync,
    ws: Option<Ws>,
    gen: u64,
    server: String,
    /// Outputs produced by `edit`, flushed by the next `pump_until`.
    pending_out: Vec<Output>,
}

impl Client {
    fn new(doc: &str, replica: u64, server: &str) -> Self {
        let cfg = ClientConfig {
            retimestamp_margin_ms: 30_000,
            hello_timeout_ms: 700,
            ack_timeout_ms: 1_500,
        };
        Client {
            sync: ClientSync::new(
                DocId(doc.into()),
                ReplicaId(replica),
                UserInfo {
                    name: format!("r{replica}"),
                    color: 0,
                },
                cfg,
            ),
            ws: None,
            gen: 0,
            server: server.into(),
            pending_out: Vec::new(),
        }
    }

    async fn connect(&mut self) {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", self.server))
            .await
            .expect("connect");
        self.ws = Some(ws);
        let mut out = Vec::new();
        self.gen = self.sync.connected(now(), &mut out);
        self.outputs(out).await;
    }

    async fn outputs(&mut self, out: Vec<Output>) {
        for o in out {
            match o {
                Output::Send(m) => {
                    if let Some(ws) = self.ws.as_mut() {
                        let _ = ws.send(Message::Binary(encode_client(&m))).await;
                    }
                }
                Output::Persist(op) => {
                    // Memory store: durable at once.
                    let mut more = Vec::new();
                    self.sync.persisted(op.id.counter, now(), &mut more);
                    Box::pin(self.outputs(more)).await;
                }
                Output::Reconnect => {
                    self.ws = None;
                    self.sync.disconnected();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Box::pin(self.connect()).await;
                }
            }
        }
    }

    fn edit(&mut self, kind: OpKind) -> Op {
        let mut out = Vec::new();
        let op = self.sync.edit(kind, now(), &mut out);
        // Outputs are pumped by the caller.
        self.pending_out.extend(out);
        op
    }

    /// Process network and timers until `done` holds or the timeout elapses.
    async fn pump_until(&mut self, done: impl Fn(&ClientSync) -> bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let queued = std::mem::take(&mut self.pending_out);
            if !queued.is_empty() {
                self.outputs(queued).await;
            }
            if done(&self.sync) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            if self.ws.is_none() && self.sync.state() == ConnState::Disconnected {
                tokio::time::sleep(Duration::from_millis(30)).await;
                self.connect().await;
                continue;
            }
            let tick_at = self
                .sync
                .next_deadline()
                .map(|d| Duration::from_millis(d.saturating_sub(now().mono_ms)));
            let sleep = tick_at
                .unwrap_or(Duration::from_millis(200))
                .min(Duration::from_millis(200));
            let ws = self.ws.as_mut().unwrap();
            tokio::select! {
                frame = ws.next() => match frame {
                    Some(Ok(Message::Binary(b))) => {
                        let msg = decode_server(&b).unwrap();
                        let mut out = Vec::new();
                        self.sync.message(self.gen, msg, now(), &mut out);
                        self.outputs(out).await;
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        self.ws = None;
                        self.sync.disconnected();
                    }
                    _ => {}
                },
                _ = tokio::time::sleep(sleep) => {
                    let mut out = Vec::new();
                    self.sync.tick(now(), &mut out);
                    self.outputs(out).await;
                }
            }
        }
    }
}

async fn server_hash(addr: &str, doc: &str) -> (String, u64) {
    let body: serde_json::Value = reqwest_get(&format!("http://{addr}/docs/{doc}/hash")).await;
    (
        body["hash"].as_str().unwrap().to_string(),
        body["durable_seq"].as_u64().unwrap(),
    )
}

/// Tiny HTTP GET via tokio (no extra dependency).
async fn reqwest_get(url: &str) -> serde_json::Value {
    let url = url.strip_prefix("http://").unwrap();
    let (host, path) = url.split_once('/').unwrap();
    let mut stream = TcpStream::connect(host).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(
            format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("{}");
    serde_json::from_str(body).unwrap_or(serde_json::json!({}))
}

async fn http_put_json(url: &str, json: &str) {
    let url = url.strip_prefix("http://").unwrap();
    let (host, path) = url.split_once('/').unwrap();
    let mut stream = TcpStream::connect(host).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(format!("PUT /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}", json.len()).as_bytes())
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
}

fn rect(x: f64) -> OpKind {
    OpKind::Create {
        kind: ObjectKind::Rect,
        props: vec![
            ("x".into(), Value::F64(x)),
            ("z".into(), Value::FracIndex("a0".into())),
        ],
    }
}

#[tokio::test]
async fn two_clients_converge_and_match_server_hash() {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let server = start(storage).await;
    let doc = "doc-a";
    let mut a = Client::new(doc, 1, &server.addr);
    let mut b = Client::new(doc, 2, &server.addr);
    a.connect().await;
    assert!(
        a.pump_until(|s| s.state() == ConnState::Live, Duration::from_secs(3))
            .await
    );
    let created = a.edit(rect(1.0));
    a.edit(OpKind::SetProps {
        object: created.id,
        entries: vec![("x".into(), Value::F64(2.0))],
    });
    assert!(
        a.pump_until(
            |s| s.unacked().count() == 0 && s.last_seq() == 2,
            Duration::from_secs(3)
        )
        .await
    );
    b.connect().await;
    assert!(
        b.pump_until(|s| s.last_seq() == 2, Duration::from_secs(3))
            .await
    );
    b.edit(OpKind::Delete { object: created.id });
    assert!(
        b.pump_until(|s| s.unacked().count() == 0, Duration::from_secs(3))
            .await
    );
    assert!(
        a.pump_until(|s| s.last_seq() == 3, Duration::from_secs(3))
            .await
    );
    let (h, seq) = server_hash(&server.addr, doc).await;
    assert_eq!(seq, 3);
    assert_eq!(codec::hex(&a.sync.hash()), h);
    assert_eq!(codec::hex(&b.sync.hash()), h);
    server.handle.abort();
}

/// Postgres when `DATABASE_URL` is set (a scratch database), memory otherwise.
async fn durable_storage() -> Arc<dyn Storage> {
    match std::env::var("DATABASE_URL") {
        Ok(url) => Arc::new(
            converge_storage::PgStorage::connect(&url)
                .await
                .expect("postgres"),
        ),
        Err(_) => Arc::new(MemoryStorage::new()),
    }
}

#[tokio::test]
async fn server_restart_resumes_from_storage() {
    let storage = durable_storage().await;
    let doc = &format!(
        "doc-b-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let doc = doc.as_str();
    let server = start(storage.clone()).await;
    let mut a = Client::new(doc, 11, &server.addr);
    a.connect().await;
    assert!(
        a.pump_until(|s| s.state() == ConnState::Live, Duration::from_secs(3))
            .await
    );
    for i in 0..7 {
        a.edit(rect(i as f64));
    }
    assert!(
        a.pump_until(
            |s| s.unacked().count() == 0 && s.last_seq() == 7,
            Duration::from_secs(3)
        )
        .await
    );
    // Crash the server; ops 1..7 are durable (snapshot at 5 + tail).
    server.handle.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let server2 = start(storage).await;
    a.server = server2.addr.clone();
    a.ws = None;
    a.sync.disconnected();
    // Offline edits while the server was down, then reconnect and resume by seq.
    a.edit(rect(100.0));
    a.connect().await;
    assert!(
        a.pump_until(
            |s| s.unacked().count() == 0 && s.last_seq() == 8,
            Duration::from_secs(3)
        )
        .await
    );
    let mut b = Client::new(doc, 12, &server2.addr);
    b.connect().await;
    assert!(
        b.pump_until(|s| s.last_seq() == 8, Duration::from_secs(3))
            .await
    );
    let (h, seq) = server_hash(&server2.addr, doc).await;
    assert_eq!(seq, 8);
    assert_eq!(codec::hex(&a.sync.hash()), h);
    assert_eq!(codec::hex(&b.sync.hash()), h);
    assert_eq!(b.sync.document().render_order().len(), 8);
    server2.handle.abort();
}

#[tokio::test]
async fn converges_under_server_side_chaos() {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let server = start(storage).await;
    let doc = "doc-c";
    http_put_json(
        &format!("http://{}/admin/chaos", server.addr),
        r#"{"enabled":true,"latency_ms":[1,15],"drop_p":0.2,"dup_p":0.2,"disconnect_every":25}"#,
    )
    .await;
    let mut a = Client::new(doc, 21, &server.addr);
    let mut b = Client::new(doc, 22, &server.addr);
    a.connect().await;
    b.connect().await;
    let mut objects = Vec::new();
    for i in 0..30 {
        let c = if i % 2 == 0 { &mut a } else { &mut b };
        let op = c.edit(rect(i as f64));
        objects.push(op.id);
        if i % 3 == 0 {
            let target = objects[i / 3];
            c.edit(OpKind::SetProps {
                object: target,
                entries: vec![("x".into(), Value::F64(-1.0))],
            });
        }
        c.pump_until(|_| true, Duration::from_millis(1)).await;
        let other = if i % 2 == 0 { &mut b } else { &mut a };
        other.pump_until(|_| true, Duration::from_millis(1)).await;
    }
    // Drain under chaos, then switch chaos off and let everything settle.
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline
        && !(a.sync.unacked().count() == 0 && b.sync.unacked().count() == 0)
    {
        a.pump_until(|_| true, Duration::from_millis(50)).await;
        b.pump_until(|_| true, Duration::from_millis(50)).await;
    }
    http_put_json(
        &format!("http://{}/admin/chaos", server.addr),
        r#"{"enabled":false}"#,
    )
    .await;
    let (h, seq) = loop {
        let (h, seq) = server_hash(&server.addr, doc).await;
        a.pump_until(
            |s| s.last_seq() == seq && s.unacked().count() == 0,
            Duration::from_millis(300),
        )
        .await;
        b.pump_until(
            |s| s.last_seq() == seq && s.unacked().count() == 0,
            Duration::from_millis(300),
        )
        .await;
        let stable = a.sync.last_seq() == seq
            && b.sync.last_seq() == seq
            && a.sync.unacked().count() == 0
            && b.sync.unacked().count() == 0;
        if stable || Instant::now() > deadline + Duration::from_secs(10) {
            break (h, seq);
        }
    };
    assert_eq!(seq, 40, "every op accepted exactly once");
    assert_eq!(codec::hex(&a.sync.hash()), h);
    assert_eq!(codec::hex(&b.sync.hash()), h);
    server.handle.abort();
}

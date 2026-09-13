//! One task per open document. Owns the sans-I/O [`Hub`]; sessions talk to
//! it over a bounded channel; a persister task group-commits accepted ops
//! and reports durability back (`docs/ARCHITECTURE.md` §11).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use converge_core::{codec, Op};
use converge_hub::{Effect, Hub, HubConfig};
use converge_proto::{ByeReason, ClientMsg, DocId, Hello, ServerMsg, SessionId};
use converge_storage::Storage;
use tokio::sync::{mpsc, oneshot};

use crate::config::Config;

pub enum DocCommand {
    Open {
        session: SessionId,
        hello: Hello,
        tx: mpsc::Sender<ServerMsg>,
    },
    Msg {
        session: SessionId,
        msg: ClientMsg,
    },
    Closed {
        session: SessionId,
    },
    Durable {
        up_to: u64,
    },
    Inspect {
        reply: oneshot::Sender<Inspection>,
    },
    Snapshot {
        reply: oneshot::Sender<(Vec<u8>, u64)>,
    },
    Shutdown,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Inspection {
    pub doc: String,
    pub hash: String,
    pub head_seq: u64,
    pub durable_seq: u64,
    pub sessions: usize,
}

#[derive(Clone)]
pub struct DocHandle {
    pub tx: mpsc::Sender<DocCommand>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Load the document and run its actor until idle or shutdown.
pub async fn spawn(
    doc: DocId,
    cfg: Config,
    storage: Arc<dyn Storage>,
    on_exit: impl FnOnce() + Send + 'static,
) -> anyhow::Result<DocHandle> {
    storage.ensure_doc(&doc).await?;
    let loaded = storage.load(&doc).await?;
    let (snapshot_doc, snapshot_seq) = match loaded.snapshot {
        Some((bytes, seq)) => (
            codec::decode_snapshot(&bytes).map_err(|e| anyhow::anyhow!("corrupt snapshot: {e}"))?,
            seq,
        ),
        None => (converge_core::Document::new(), 0),
    };
    let hub = Hub::recover(
        doc.clone(),
        HubConfig {
            skew_tolerance_ms: cfg.skew_tolerance_ms,
            log_capacity: cfg.log_capacity,
        },
        snapshot_doc,
        snapshot_seq,
        loaded.tail,
    );
    tracing::info!(doc = %doc.0, seq = hub.durable_seq(), "document loaded");
    let (tx, rx) = mpsc::channel(1_024);
    let handle = DocHandle { tx: tx.clone() };
    let (persist_tx, persist_rx) = mpsc::channel::<Vec<(u64, Op)>>(1_024);
    tokio::spawn(persister(
        doc.clone(),
        cfg.clone(),
        storage.clone(),
        persist_rx,
        tx.clone(),
    ));
    tokio::spawn(async move {
        run(hub, cfg, storage, rx, persist_tx).await;
        on_exit();
    });
    Ok(handle)
}

async fn run(
    mut hub: Hub,
    cfg: Config,
    storage: Arc<dyn Storage>,
    mut rx: mpsc::Receiver<DocCommand>,
    persist_tx: mpsc::Sender<Vec<(u64, Op)>>,
) {
    let mut sessions: HashMap<SessionId, mpsc::Sender<ServerMsg>> = HashMap::new();
    let mut durable_since_snapshot = 0u64;
    let mut idle_since = Instant::now();
    let doc = hub.doc_id().clone();
    loop {
        let cmd = tokio::select! {
            c = rx.recv() => match c { Some(c) => c, None => break },
            _ = tokio::time::sleep(cfg.idle_shutdown) => {
                if sessions.is_empty() && idle_since.elapsed() >= cfg.idle_shutdown && hub.head_seq() == hub.durable_seq() {
                    tracing::info!(doc = %doc.0, "idle; unloading document");
                    break;
                }
                continue;
            }
        };
        let mut out = Vec::new();
        match cmd {
            DocCommand::Open { session, hello, tx } => {
                sessions.insert(session, tx);
                hub.open(session, hello, now_ms(), &mut out);
                metrics::gauge!("converge_sessions").set(sessions.len() as f64);
            }
            DocCommand::Msg { session, msg } => {
                if let ClientMsg::Submit { ops } = &msg {
                    metrics::counter!("converge_ops_submitted_total").increment(ops.len() as u64);
                }
                hub.handle(session, msg, now_ms(), &mut out);
            }
            DocCommand::Closed { session } => {
                sessions.remove(&session);
                hub.close(session, &mut out);
                metrics::gauge!("converge_sessions").set(sessions.len() as f64);
                if sessions.is_empty() {
                    idle_since = Instant::now();
                }
            }
            DocCommand::Durable { up_to } => {
                let before = hub.durable_seq();
                hub.durable(up_to, &mut out);
                let n = hub.durable_seq() - before;
                metrics::counter!("converge_ops_committed_total").increment(n);
                durable_since_snapshot += n;
                if durable_since_snapshot >= cfg.snapshot_every {
                    durable_since_snapshot = 0;
                    let (bytes, seq) = hub.snapshot();
                    let storage = storage.clone();
                    let doc = doc.clone();
                    let retain = cfg.retain_ops;
                    tokio::spawn(async move {
                        let t = Instant::now();
                        match storage.write_snapshot(&doc, seq, &bytes).await {
                            Ok(()) => {
                                metrics::histogram!("converge_snapshot_seconds")
                                    .record(t.elapsed().as_secs_f64());
                                if seq > retain {
                                    if let Err(e) = storage.prune(&doc, seq - retain).await {
                                        tracing::warn!(doc = %doc.0, "prune failed: {e}");
                                    }
                                }
                            }
                            Err(e) => tracing::warn!(doc = %doc.0, "snapshot failed: {e}"),
                        }
                    });
                }
            }
            DocCommand::Inspect { reply } => {
                let _ = reply.send(Inspection {
                    doc: doc.0.clone(),
                    hash: codec::hex(&hub.hash()),
                    head_seq: hub.head_seq(),
                    durable_seq: hub.durable_seq(),
                    sessions: sessions.len(),
                });
            }
            DocCommand::Snapshot { reply } => {
                let _ = reply.send(hub.snapshot());
            }
            DocCommand::Shutdown => {
                for sid in hub.sessions().collect::<Vec<_>>() {
                    out.push(Effect::Send {
                        to: sid,
                        msg: ServerMsg::Bye {
                            reason: ByeReason::Restart,
                        },
                    });
                }
                let (bytes, seq) = hub.snapshot();
                if let Err(e) = storage.write_snapshot(&doc, seq, &bytes).await {
                    tracing::warn!(doc = %doc.0, "final snapshot failed: {e}");
                }
                apply_effects(&mut hub, &mut sessions, &persist_tx, out, &cfg).await;
                break;
            }
        }
        apply_effects(&mut hub, &mut sessions, &persist_tx, out, &cfg).await;
    }
}

async fn apply_effects(
    hub: &mut Hub,
    sessions: &mut HashMap<SessionId, mpsc::Sender<ServerMsg>>,
    persist_tx: &mpsc::Sender<Vec<(u64, Op)>>,
    effects: Vec<Effect>,
    cfg: &Config,
) {
    let mut evict = Vec::new();
    for e in effects {
        match e {
            Effect::Send { to, msg } => {
                if let Some(tx) = sessions.get(&to) {
                    // A full queue means the client cannot keep up: evict it;
                    // it will resume by seq (strictly cheaper than buffering).
                    if tx.try_send(msg).is_err() {
                        evict.push(to);
                    }
                }
            }
            Effect::Close { session } => {
                sessions.remove(&session); // dropping the sender closes the socket
            }
            Effect::Persist { ops } => {
                metrics::counter!("converge_ops_accepted_total").increment(ops.len() as u64);
                if persist_tx.send(ops).await.is_err() {
                    tracing::error!("persister gone");
                }
            }
        }
    }
    for sid in evict {
        metrics::counter!("converge_slow_consumer_evictions_total").increment(1);
        if let Some(tx) = sessions.remove(&sid) {
            let _ = tx.try_send(ServerMsg::Bye {
                reason: ByeReason::SlowConsumer,
            });
        }
        let mut out = Vec::new();
        hub.close(sid, &mut out);
        // Presence-leave fan-out from the close; nothing else can recurse.
        for e in out {
            if let Effect::Send { to, msg } = e {
                if let Some(tx) = sessions.get(&to) {
                    let _ = tx.try_send(msg);
                }
            }
        }
    }
    let _ = cfg;
}

/// Single FIFO writer: batches accepted ops, writes them, reports `Durable`.
/// Never reorders, never skips: `durable_seq` stays a prefix of `head_seq`.
async fn persister(
    doc: DocId,
    cfg: Config,
    storage: Arc<dyn Storage>,
    mut rx: mpsc::Receiver<Vec<(u64, Op)>>,
    actor: mpsc::Sender<DocCommand>,
) {
    while let Some(first) = rx.recv().await {
        let mut batch = first;
        let deadline = tokio::time::sleep(cfg.persist_window);
        tokio::pin!(deadline);
        while batch.len() < cfg.persist_batch {
            tokio::select! {
                more = rx.recv() => match more { Some(m) => batch.extend(m), None => break },
                _ = &mut deadline => break,
            }
        }
        let up_to = batch.last().map(|(s, _)| *s).unwrap_or(0);
        let mut attempt = 0u32;
        loop {
            let t = Instant::now();
            match storage.append(&doc, &batch).await {
                Ok(()) => {
                    metrics::histogram!("converge_persist_seconds")
                        .record(t.elapsed().as_secs_f64());
                    metrics::histogram!("converge_persist_batch_size").record(batch.len() as f64);
                    break;
                }
                Err(e) => {
                    attempt += 1;
                    tracing::error!(doc = %doc.0, attempt, "persist failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50 * 2u64.pow(attempt.min(6)))).await;
                }
            }
        }
        if actor.send(DocCommand::Durable { up_to }).await.is_err() {
            return;
        }
    }
}

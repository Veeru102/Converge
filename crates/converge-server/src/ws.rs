//! WebSocket session adapter: decodes frames, forwards to the document
//! actor, drains the per-session outbound queue, applies chaos faults, and
//! keeps the connection alive with pings.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use converge_proto::wire::{decode_client, encode_server};
use converge_proto::{ByeReason, ClientMsg, ServerMsg, SessionId};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::actor::DocCommand;
use crate::chaos::Fate;
use crate::AppState;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

pub async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| session(socket, state))
}

async fn session(socket: WebSocket, state: AppState) {
    let session = SessionId(NEXT_SESSION.fetch_add(1, Ordering::Relaxed));
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(state.cfg.outbound_queue);
    let mut chaos_out = state.chaos.session(session.0);
    let mut chaos_in = state.chaos.session(session.0 ^ 0xFFFF);
    let ping_interval = state.cfg.ping_interval;

    // Outbound: single writer so FIFO order survives injected latency.
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(ping_interval);
        ping.tick().await;
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    match chaos_out.fate() {
                        Fate::Drop => continue,
                        Fate::Disconnect => break,
                        Fate::Deliver { delay, dup } => {
                            if !delay.is_zero() { tokio::time::sleep(delay).await; }
                            let bytes = encode_server(&msg);
                            metrics::counter!("converge_ws_messages_out_total").increment(1);
                            if sink.send(Message::Binary(bytes.clone())).await.is_err() { break; }
                            if dup && sink.send(Message::Binary(bytes)).await.is_err() { break; }
                        }
                    }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new())).await.is_err() { break; }
                }
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });

    let mut actor: Option<mpsc::Sender<DocCommand>> = None;
    let pong_timeout = ping_interval * 3;
    loop {
        let frame = match tokio::time::timeout(pong_timeout, stream.next()).await {
            Ok(Some(Ok(f))) => f,
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => {
                tracing::debug!(session = session.0, "ping timeout");
                break;
            }
        };
        let bytes = match frame {
            Message::Binary(b) => b,
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Text(_) => continue,
        };
        metrics::counter!("converge_ws_messages_in_total").increment(1);
        let msg = match decode_client(&bytes) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(session = session.0, "undecodable frame: {e}");
                let _ = tx.try_send(ServerMsg::Bye {
                    reason: ByeReason::ProtocolError,
                });
                break;
            }
        };
        match chaos_in.fate() {
            Fate::Drop => continue,
            Fate::Disconnect => break,
            Fate::Deliver { delay, dup } => {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if !forward(&state, &mut actor, session, &tx, msg.clone()).await {
                    break;
                }
                if dup && !forward(&state, &mut actor, session, &tx, msg).await {
                    break;
                }
            }
        }
    }
    if let Some(a) = actor {
        let _ = a.send(DocCommand::Closed { session }).await;
    }
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
}

/// Route one client message to the document actor (opening it on `Hello`).
async fn forward(
    state: &AppState,
    actor: &mut Option<mpsc::Sender<DocCommand>>,
    session: SessionId,
    tx: &mpsc::Sender<ServerMsg>,
    msg: ClientMsg,
) -> bool {
    match (&*actor, msg) {
        (None, ClientMsg::Hello(hello)) => match state.registry.get_or_spawn(&hello.doc).await {
            Ok(h) => {
                let cmd = DocCommand::Open {
                    session,
                    hello,
                    tx: tx.clone(),
                };
                if h.tx.send(cmd).await.is_err() {
                    return false;
                }
                *actor = Some(h.tx);
                true
            }
            Err(e) => {
                tracing::error!("cannot open document: {e}");
                let _ = tx.try_send(ServerMsg::Bye {
                    reason: ByeReason::UnknownDoc,
                });
                false
            }
        },
        (None, _) => {
            let _ = tx.try_send(ServerMsg::Bye {
                reason: ByeReason::ProtocolError,
            });
            false
        }
        (Some(a), msg) => a.send(DocCommand::Msg { session, msg }).await.is_ok(),
    }
}

//! JSON form of protocol messages (u64 as decimal strings), used by the
//! simulator traces that the TypeScript sync engine replays, and by debug
//! endpoints. Not the wire format.

use converge_core::json::{
    hlc_from_json, hlc_to_json, id_from_json, id_to_json, op_from_json, op_to_json, u64_from_json,
    JsonError,
};
use converge_core::{Hlc, ReplicaId};
use serde_json::{json, Value as J};

use crate::*;

fn err(m: &str) -> JsonError {
    JsonError(m.into())
}

pub fn user_to_json(u: &UserInfo) -> J {
    json!({"name": u.name, "color": u.color})
}

pub fn user_from_json(j: &J) -> Result<UserInfo, JsonError> {
    Ok(UserInfo {
        name: j["name"].as_str().unwrap_or("").into(),
        color: j["color"].as_u64().unwrap_or(0) as u32,
    })
}

pub fn presence_to_json(p: &PresenceState) -> J {
    json!({
        "cursor": p.cursor.map(|(x, y)| json!([x, y])),
        "selection": p.selection.iter().map(id_to_json).collect::<Vec<_>>(),
    })
}

pub fn presence_from_json(j: &J) -> Result<PresenceState, JsonError> {
    let cursor = match &j["cursor"] {
        J::Array(a) if a.len() == 2 => {
            Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
        }
        _ => None,
    };
    let selection = j["selection"]
        .as_array()
        .map(|a| a.iter().map(id_from_json).collect::<Result<Vec<_>, _>>())
        .transpose()?
        .unwrap_or_default();
    Ok(PresenceState { cursor, selection })
}

fn entry_to_json(e: &PresenceEntry) -> J {
    json!({"replica": e.replica.0.to_string(), "user": user_to_json(&e.user), "state": presence_to_json(&e.state)})
}

fn entry_from_json(j: &J) -> Result<PresenceEntry, JsonError> {
    Ok(PresenceEntry {
        replica: ReplicaId(u64_from_json(&j["replica"])?),
        user: user_from_json(&j["user"])?,
        state: presence_from_json(&j["state"])?,
    })
}

pub fn client_msg_to_json(m: &ClientMsg) -> J {
    match m {
        ClientMsg::Hello(h) => json!({
            "type": "hello", "version": h.version, "doc": h.doc.0, "replica": h.replica.0.to_string(),
            "last_seq": h.last_seq.to_string(), "want_snapshot": h.want_snapshot, "user": user_to_json(&h.user),
        }),
        ClientMsg::Submit { ops } => {
            json!({"type": "submit", "ops": ops.iter().map(op_to_json).collect::<Vec<_>>()})
        }
        ClientMsg::Presence(p) => json!({"type": "presence", "state": presence_to_json(p)}),
        ClientMsg::Pong { nonce } => json!({"type": "pong", "nonce": nonce.to_string()}),
    }
}

pub fn client_msg_from_json(j: &J) -> Result<ClientMsg, JsonError> {
    Ok(match j["type"].as_str().ok_or_else(|| err("type"))? {
        "hello" => ClientMsg::Hello(Hello {
            version: j["version"].as_u64().ok_or_else(|| err("version"))? as u32,
            doc: DocId(j["doc"].as_str().ok_or_else(|| err("doc"))?.into()),
            replica: ReplicaId(u64_from_json(&j["replica"])?),
            last_seq: u64_from_json(&j["last_seq"])?,
            want_snapshot: j["want_snapshot"].as_bool().unwrap_or(false),
            user: user_from_json(&j["user"])?,
        }),
        "submit" => ClientMsg::Submit {
            ops: j["ops"]
                .as_array()
                .ok_or_else(|| err("ops"))?
                .iter()
                .map(op_from_json)
                .collect::<Result<_, _>>()?,
        },
        "presence" => ClientMsg::Presence(presence_from_json(&j["state"])?),
        "pong" => ClientMsg::Pong {
            nonce: u64_from_json(&j["nonce"])?,
        },
        t => return Err(JsonError(format!("bad client message type {t}"))),
    })
}

fn bye_name(r: ByeReason) -> &'static str {
    match r {
        ByeReason::VersionMismatch => "version_mismatch",
        ByeReason::UnknownDoc => "unknown_doc",
        ByeReason::Superseded => "superseded",
        ByeReason::ClockSkew => "clock_skew",
        ByeReason::SlowConsumer => "slow_consumer",
        ByeReason::Restart => "restart",
        ByeReason::ProtocolError => "protocol_error",
    }
}

fn bye_from_name(s: &str) -> Option<ByeReason> {
    Some(match s {
        "version_mismatch" => ByeReason::VersionMismatch,
        "unknown_doc" => ByeReason::UnknownDoc,
        "superseded" => ByeReason::Superseded,
        "clock_skew" => ByeReason::ClockSkew,
        "slow_consumer" => ByeReason::SlowConsumer,
        "restart" => ByeReason::Restart,
        "protocol_error" => ByeReason::ProtocolError,
        _ => return None,
    })
}

fn nack_name(r: NackReason) -> &'static str {
    match r {
        NackReason::ClockSkew => "clock_skew",
        NackReason::Malformed => "malformed",
        NackReason::Rejected => "rejected",
    }
}

fn nack_from_name(s: &str) -> Option<NackReason> {
    Some(match s {
        "clock_skew" => NackReason::ClockSkew,
        "malformed" => NackReason::Malformed,
        "rejected" => NackReason::Rejected,
        _ => return None,
    })
}

pub fn server_msg_to_json(m: &ServerMsg) -> J {
    match m {
        ServerMsg::Welcome {
            session,
            server_time_ms,
            durable_head_seq,
            catch_up,
            presence,
        } => {
            let catch = match catch_up {
                CatchUp::Ops(ops) => {
                    json!({"ops": ops.iter().map(|(s, op)| json!({"seq": s.to_string(), "op": op_to_json(op)})).collect::<Vec<_>>()})
                }
                CatchUp::Snapshot { bytes, seq } => {
                    json!({"snapshot": converge_core::codec::hex(bytes), "seq": seq.to_string()})
                }
            };
            json!({
                "type": "welcome", "session": session.0.to_string(), "server_time_ms": server_time_ms.to_string(),
                "durable_head_seq": durable_head_seq.to_string(), "catch_up": catch,
                "presence": presence.iter().map(entry_to_json).collect::<Vec<_>>(),
            })
        }
        ServerMsg::Bye { reason } => json!({"type": "bye", "reason": bye_name(*reason)}),
        ServerMsg::Commit { seq, op } => {
            json!({"type": "commit", "seq": seq.to_string(), "op": op_to_json(op)})
        }
        ServerMsg::Ack { op_id, seq } => {
            json!({"type": "ack", "op_id": id_to_json(op_id), "seq": seq.map(|s| s.to_string())})
        }
        ServerMsg::Nack { op_id, reason } => {
            json!({"type": "nack", "op_id": id_to_json(op_id), "reason": nack_name(*reason)})
        }
        ServerMsg::PresenceUpdate(e) => {
            json!({"type": "presence_update", "entry": entry_to_json(e)})
        }
        ServerMsg::PresenceLeave { replica } => {
            json!({"type": "presence_leave", "replica": replica.0.to_string()})
        }
        ServerMsg::Ping { nonce } => json!({"type": "ping", "nonce": nonce.to_string()}),
    }
}

fn unhex(s: &str) -> Result<Vec<u8>, JsonError> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| err("hex")))
        .collect()
}

pub fn server_msg_from_json(j: &J) -> Result<ServerMsg, JsonError> {
    Ok(match j["type"].as_str().ok_or_else(|| err("type"))? {
        "welcome" => {
            let c = &j["catch_up"];
            let catch_up = if let Some(ops) = c["ops"].as_array() {
                CatchUp::Ops(
                    ops.iter()
                        .map(|e| Ok((u64_from_json(&e["seq"])?, op_from_json(&e["op"])?)))
                        .collect::<Result<_, JsonError>>()?,
                )
            } else {
                CatchUp::Snapshot {
                    bytes: unhex(c["snapshot"].as_str().ok_or_else(|| err("snapshot"))?)?,
                    seq: u64_from_json(&c["seq"])?,
                }
            };
            ServerMsg::Welcome {
                session: SessionId(u64_from_json(&j["session"])?),
                server_time_ms: u64_from_json(&j["server_time_ms"])?,
                durable_head_seq: u64_from_json(&j["durable_head_seq"])?,
                catch_up,
                presence: j["presence"]
                    .as_array()
                    .map(|a| a.iter().map(entry_from_json).collect::<Result<Vec<_>, _>>())
                    .transpose()?
                    .unwrap_or_default(),
            }
        }
        "bye" => ServerMsg::Bye {
            reason: bye_from_name(j["reason"].as_str().unwrap_or(""))
                .ok_or_else(|| err("bye reason"))?,
        },
        "commit" => ServerMsg::Commit {
            seq: u64_from_json(&j["seq"])?,
            op: op_from_json(&j["op"])?,
        },
        "ack" => ServerMsg::Ack {
            op_id: id_from_json(&j["op_id"])?,
            seq: j["seq"]
                .as_str()
                .map(|s| s.parse().map_err(|_| err("seq")))
                .transpose()?,
        },
        "nack" => ServerMsg::Nack {
            op_id: id_from_json(&j["op_id"])?,
            reason: nack_from_name(j["reason"].as_str().unwrap_or(""))
                .ok_or_else(|| err("nack reason"))?,
        },
        "presence_update" => ServerMsg::PresenceUpdate(entry_from_json(&j["entry"])?),
        "presence_leave" => ServerMsg::PresenceLeave {
            replica: ReplicaId(u64_from_json(&j["replica"])?),
        },
        "ping" => ServerMsg::Ping {
            nonce: u64_from_json(&j["nonce"])?,
        },
        t => return Err(JsonError(format!("bad server message type {t}"))),
    })
}

pub fn hlc_json(h: &Hlc) -> J {
    hlc_to_json(h)
}

pub fn hlc_from(j: &J) -> Result<Hlc, JsonError> {
    hlc_from_json(j)
}

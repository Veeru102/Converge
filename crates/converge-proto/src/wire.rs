//! Protobuf wire encoding of protocol messages (`proto/converge/v1`).

use converge_core::{Hlc, ObjectKind, Op, OpId, OpKind, ReplicaId, Value};
use prost::Message;

use crate::*;

#[allow(clippy::all, missing_docs)]
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/converge.v1.rs"));
}

#[derive(thiserror::Error, Debug)]
pub enum WireError {
    #[error("protobuf decode: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("missing field {0}")]
    Missing(&'static str),
    #[error("invalid enum value for {0}")]
    BadEnum(&'static str),
    #[error("logical clock out of range")]
    Logical,
}

type R<T> = Result<T, WireError>;

// ----- core types -----

fn id_to_pb(id: &OpId) -> pb::OpId {
    pb::OpId {
        replica: id.replica.0,
        counter: id.counter,
    }
}
fn id_from_pb(p: Option<pb::OpId>, what: &'static str) -> R<OpId> {
    let p = p.ok_or(WireError::Missing(what))?;
    Ok(OpId {
        replica: ReplicaId(p.replica),
        counter: p.counter,
    })
}
fn hlc_to_pb(h: &Hlc) -> pb::Hlc {
    pb::Hlc {
        wall_ms: h.wall_ms,
        logical: h.logical as u32,
    }
}
fn hlc_from_pb(p: Option<pb::Hlc>) -> R<Hlc> {
    let p = p.ok_or(WireError::Missing("hlc"))?;
    Ok(Hlc::new(
        p.wall_ms,
        u16::try_from(p.logical).map_err(|_| WireError::Logical)?,
    ))
}
fn value_to_pb(v: &Value) -> pb::Value {
    use pb::value::Kind;
    let kind = match v {
        Value::Null => Kind::Null(pb::Null {}),
        Value::Bool(b) => Kind::Bool(*b),
        Value::I64(i) => Kind::I64(*i),
        Value::F64(f) => Kind::F64(*f),
        Value::Str(s) => Kind::Str(s.clone()),
        Value::Color(c) => Kind::Color(*c),
        Value::FracIndex(s) => Kind::FracIndex(s.clone()),
        Value::ObjRef(id) => Kind::ObjRef(id_to_pb(id)),
    };
    pb::Value { kind: Some(kind) }
}
fn value_from_pb(p: Option<pb::Value>) -> R<Value> {
    use pb::value::Kind;
    Ok(
        match p.and_then(|v| v.kind).ok_or(WireError::Missing("value"))? {
            Kind::Null(_) => Value::Null,
            Kind::Bool(b) => Value::Bool(b),
            Kind::I64(i) => Value::I64(i),
            Kind::F64(f) => Value::F64(f),
            Kind::Str(s) => Value::Str(s),
            Kind::Color(c) => Value::Color(c),
            Kind::FracIndex(s) => Value::FracIndex(s),
            Kind::ObjRef(id) => Value::ObjRef(id_from_pb(Some(id), "obj_ref")?),
        },
    )
}
fn entries_to_pb(e: &[(String, Value)]) -> Vec<pb::Entry> {
    e.iter()
        .map(|(k, v)| pb::Entry {
            key: k.clone(),
            value: Some(value_to_pb(v)),
        })
        .collect()
}
fn entries_from_pb(e: Vec<pb::Entry>) -> R<Vec<(String, Value)>> {
    e.into_iter()
        .map(|en| Ok((en.key, value_from_pb(en.value)?)))
        .collect()
}
fn kind_to_pb(k: ObjectKind) -> i32 {
    match k {
        ObjectKind::Rect => pb::ObjectKind::Rect as i32,
        ObjectKind::Text => pb::ObjectKind::Text as i32,
        ObjectKind::Group => pb::ObjectKind::Group as i32,
        ObjectKind::Connector => pb::ObjectKind::Connector as i32,
        ObjectKind::Ellipse => pb::ObjectKind::Ellipse as i32,
        ObjectKind::Line => pb::ObjectKind::Line as i32,
    }
}
fn kind_from_pb(v: i32) -> R<ObjectKind> {
    Ok(
        match pb::ObjectKind::try_from(v).map_err(|_| WireError::BadEnum("object_kind"))? {
            pb::ObjectKind::Rect => ObjectKind::Rect,
            pb::ObjectKind::Text => ObjectKind::Text,
            pb::ObjectKind::Group => ObjectKind::Group,
            pb::ObjectKind::Connector => ObjectKind::Connector,
            pb::ObjectKind::Ellipse => ObjectKind::Ellipse,
            pb::ObjectKind::Line => ObjectKind::Line,
            pb::ObjectKind::Unspecified => return Err(WireError::BadEnum("object_kind")),
        },
    )
}

pub fn op_to_pb(op: &Op) -> pb::Op {
    use pb::op::Kind;
    let kind = match &op.kind {
        OpKind::Create { kind, props } => Kind::Create(pb::Create {
            kind: kind_to_pb(*kind),
            props: entries_to_pb(props),
        }),
        OpKind::SetProps { object, entries } => Kind::SetProps(pb::SetProps {
            object: Some(id_to_pb(object)),
            entries: entries_to_pb(entries),
        }),
        OpKind::Delete { object } => Kind::Delete(pb::Delete {
            object: Some(id_to_pb(object)),
        }),
        OpKind::Restore { object } => Kind::Restore(pb::Restore {
            object: Some(id_to_pb(object)),
        }),
    };
    pb::Op {
        id: Some(id_to_pb(&op.id)),
        hlc: Some(hlc_to_pb(&op.hlc)),
        kind: Some(kind),
    }
}

pub fn op_from_pb(p: pb::Op) -> R<Op> {
    use pb::op::Kind;
    let kind = match p.kind.ok_or(WireError::Missing("op.kind"))? {
        Kind::Create(c) => OpKind::Create {
            kind: kind_from_pb(c.kind)?,
            props: entries_from_pb(c.props)?,
        },
        Kind::SetProps(s) => OpKind::SetProps {
            object: id_from_pb(s.object, "object")?,
            entries: entries_from_pb(s.entries)?,
        },
        Kind::Delete(d) => OpKind::Delete {
            object: id_from_pb(d.object, "object")?,
        },
        Kind::Restore(r) => OpKind::Restore {
            object: id_from_pb(r.object, "object")?,
        },
    };
    Ok(Op {
        id: id_from_pb(p.id, "op.id")?,
        hlc: hlc_from_pb(p.hlc)?,
        kind,
    })
}

pub fn encode_op(op: &Op) -> Vec<u8> {
    op_to_pb(op).encode_to_vec()
}
pub fn decode_op(bytes: &[u8]) -> R<Op> {
    op_from_pb(pb::Op::decode(bytes)?)
}

// ----- protocol types -----

fn user_to_pb(u: &UserInfo) -> pb::UserInfo {
    pb::UserInfo {
        name: u.name.clone(),
        color: u.color,
    }
}
fn user_from_pb(p: Option<pb::UserInfo>) -> UserInfo {
    p.map(|u| UserInfo {
        name: u.name,
        color: u.color,
    })
    .unwrap_or_default()
}
fn presence_to_pb(p: &PresenceState) -> pb::PresenceState {
    pb::PresenceState {
        has_cursor: p.cursor.is_some(),
        cursor_x: p.cursor.map(|c| c.0).unwrap_or(0.0),
        cursor_y: p.cursor.map(|c| c.1).unwrap_or(0.0),
        selection: p.selection.iter().map(id_to_pb).collect(),
    }
}
fn presence_from_pb(p: pb::PresenceState) -> R<PresenceState> {
    Ok(PresenceState {
        cursor: if p.has_cursor {
            Some((p.cursor_x, p.cursor_y))
        } else {
            None
        },
        selection: p
            .selection
            .into_iter()
            .map(|id| id_from_pb(Some(id), "selection"))
            .collect::<R<_>>()?,
    })
}
fn entry_to_pb(e: &PresenceEntry) -> pb::PresenceEntry {
    pb::PresenceEntry {
        replica: e.replica.0,
        user: Some(user_to_pb(&e.user)),
        state: Some(presence_to_pb(&e.state)),
    }
}
fn entry_from_pb(p: pb::PresenceEntry) -> R<PresenceEntry> {
    Ok(PresenceEntry {
        replica: ReplicaId(p.replica),
        user: user_from_pb(p.user),
        state: presence_from_pb(p.state.unwrap_or_default())?,
    })
}

pub fn client_msg_to_pb(m: &ClientMsg) -> pb::ClientMsg {
    use pb::client_msg::Msg;
    let msg = match m {
        ClientMsg::Hello(h) => Msg::Hello(pb::Hello {
            version: h.version,
            doc: h.doc.0.clone(),
            replica: h.replica.0,
            last_seq: h.last_seq,
            want_snapshot: h.want_snapshot,
            user: Some(user_to_pb(&h.user)),
        }),
        ClientMsg::Submit { ops } => Msg::Submit(pb::Submit {
            ops: ops.iter().map(op_to_pb).collect(),
        }),
        ClientMsg::Presence(p) => Msg::Presence(presence_to_pb(p)),
        ClientMsg::Pong { nonce } => Msg::Pong(pb::Pong { nonce: *nonce }),
    };
    pb::ClientMsg { msg: Some(msg) }
}

pub fn client_msg_from_pb(p: pb::ClientMsg) -> R<ClientMsg> {
    use pb::client_msg::Msg;
    Ok(match p.msg.ok_or(WireError::Missing("client_msg"))? {
        Msg::Hello(h) => ClientMsg::Hello(Hello {
            version: h.version,
            doc: DocId(h.doc),
            replica: ReplicaId(h.replica),
            last_seq: h.last_seq,
            want_snapshot: h.want_snapshot,
            user: user_from_pb(h.user),
        }),
        Msg::Submit(s) => ClientMsg::Submit {
            ops: s.ops.into_iter().map(op_from_pb).collect::<R<_>>()?,
        },
        Msg::Presence(p) => ClientMsg::Presence(presence_from_pb(p)?),
        Msg::Pong(p) => ClientMsg::Pong { nonce: p.nonce },
    })
}

fn bye_to_pb(r: ByeReason) -> i32 {
    (match r {
        ByeReason::VersionMismatch => pb::ByeReason::VersionMismatch,
        ByeReason::UnknownDoc => pb::ByeReason::UnknownDoc,
        ByeReason::Superseded => pb::ByeReason::Superseded,
        ByeReason::ClockSkew => pb::ByeReason::ClockSkew,
        ByeReason::SlowConsumer => pb::ByeReason::SlowConsumer,
        ByeReason::Restart => pb::ByeReason::Restart,
        ByeReason::ProtocolError => pb::ByeReason::ProtocolError,
    }) as i32
}
fn bye_from_pb(v: i32) -> R<ByeReason> {
    Ok(
        match pb::ByeReason::try_from(v).map_err(|_| WireError::BadEnum("bye_reason"))? {
            pb::ByeReason::VersionMismatch => ByeReason::VersionMismatch,
            pb::ByeReason::UnknownDoc => ByeReason::UnknownDoc,
            pb::ByeReason::Superseded => ByeReason::Superseded,
            pb::ByeReason::ClockSkew => ByeReason::ClockSkew,
            pb::ByeReason::SlowConsumer => ByeReason::SlowConsumer,
            pb::ByeReason::Restart => ByeReason::Restart,
            pb::ByeReason::ProtocolError => ByeReason::ProtocolError,
            pb::ByeReason::Unspecified => return Err(WireError::BadEnum("bye_reason")),
        },
    )
}
fn nack_to_pb(r: NackReason) -> i32 {
    (match r {
        NackReason::ClockSkew => pb::NackReason::ClockSkew,
        NackReason::Malformed => pb::NackReason::Malformed,
        NackReason::Rejected => pb::NackReason::Rejected,
    }) as i32
}
fn nack_from_pb(v: i32) -> R<NackReason> {
    Ok(
        match pb::NackReason::try_from(v).map_err(|_| WireError::BadEnum("nack_reason"))? {
            pb::NackReason::ClockSkew => NackReason::ClockSkew,
            pb::NackReason::Malformed => NackReason::Malformed,
            pb::NackReason::Rejected => NackReason::Rejected,
            pb::NackReason::Unspecified => return Err(WireError::BadEnum("nack_reason")),
        },
    )
}

pub fn server_msg_to_pb(m: &ServerMsg) -> pb::ServerMsg {
    use pb::server_msg::Msg;
    let msg = match m {
        ServerMsg::Welcome {
            session,
            server_time_ms,
            durable_head_seq,
            catch_up,
            presence,
        } => Msg::Welcome(pb::Welcome {
            session: session.0,
            server_time_ms: *server_time_ms,
            durable_head_seq: *durable_head_seq,
            catch_up: Some(match catch_up {
                CatchUp::Ops(ops) => pb::welcome::CatchUp::Ops(pb::CatchUpOps {
                    ops: ops
                        .iter()
                        .map(|(seq, op)| pb::CommittedOp {
                            seq: *seq,
                            op: Some(op_to_pb(op)),
                        })
                        .collect(),
                }),
                CatchUp::Snapshot { bytes, seq } => pb::welcome::CatchUp::Snapshot(pb::Snapshot {
                    bytes: bytes.clone(),
                    seq: *seq,
                }),
            }),
            presence: presence.iter().map(entry_to_pb).collect(),
        }),
        ServerMsg::Bye { reason } => Msg::Bye(pb::Bye {
            reason: bye_to_pb(*reason),
        }),
        ServerMsg::Commit { seq, op } => Msg::Commit(pb::Commit {
            seq: *seq,
            op: Some(op_to_pb(op)),
        }),
        ServerMsg::Ack { op_id, seq } => Msg::Ack(pb::Ack {
            op_id: Some(id_to_pb(op_id)),
            seq: *seq,
        }),
        ServerMsg::Nack { op_id, reason } => Msg::Nack(pb::Nack {
            op_id: Some(id_to_pb(op_id)),
            reason: nack_to_pb(*reason),
        }),
        ServerMsg::PresenceUpdate(e) => Msg::PresenceUpdate(entry_to_pb(e)),
        ServerMsg::PresenceLeave { replica } => {
            Msg::PresenceLeave(pb::PresenceLeave { replica: replica.0 })
        }
        ServerMsg::Ping { nonce } => Msg::Ping(pb::Ping { nonce: *nonce }),
    };
    pb::ServerMsg { msg: Some(msg) }
}

pub fn server_msg_from_pb(p: pb::ServerMsg) -> R<ServerMsg> {
    use pb::server_msg::Msg;
    Ok(match p.msg.ok_or(WireError::Missing("server_msg"))? {
        Msg::Welcome(w) => ServerMsg::Welcome {
            session: SessionId(w.session),
            server_time_ms: w.server_time_ms,
            durable_head_seq: w.durable_head_seq,
            catch_up: match w.catch_up.ok_or(WireError::Missing("catch_up"))? {
                pb::welcome::CatchUp::Ops(o) => CatchUp::Ops(
                    o.ops
                        .into_iter()
                        .map(|c| Ok((c.seq, op_from_pb(c.op.ok_or(WireError::Missing("op"))?)?)))
                        .collect::<R<_>>()?,
                ),
                pb::welcome::CatchUp::Snapshot(s) => CatchUp::Snapshot {
                    bytes: s.bytes,
                    seq: s.seq,
                },
            },
            presence: w
                .presence
                .into_iter()
                .map(entry_from_pb)
                .collect::<R<_>>()?,
        },
        Msg::Bye(b) => ServerMsg::Bye {
            reason: bye_from_pb(b.reason)?,
        },
        Msg::Commit(c) => ServerMsg::Commit {
            seq: c.seq,
            op: op_from_pb(c.op.ok_or(WireError::Missing("op"))?)?,
        },
        Msg::Ack(a) => ServerMsg::Ack {
            op_id: id_from_pb(a.op_id, "op_id")?,
            seq: a.seq,
        },
        Msg::Nack(n) => ServerMsg::Nack {
            op_id: id_from_pb(n.op_id, "op_id")?,
            reason: nack_from_pb(n.reason)?,
        },
        Msg::PresenceUpdate(e) => ServerMsg::PresenceUpdate(entry_from_pb(e)?),
        Msg::PresenceLeave(l) => ServerMsg::PresenceLeave {
            replica: ReplicaId(l.replica),
        },
        Msg::Ping(p) => ServerMsg::Ping { nonce: p.nonce },
    })
}

pub fn encode_client(m: &ClientMsg) -> Vec<u8> {
    client_msg_to_pb(m).encode_to_vec()
}
pub fn decode_client(bytes: &[u8]) -> R<ClientMsg> {
    client_msg_from_pb(pb::ClientMsg::decode(bytes)?)
}
pub fn encode_server(m: &ServerMsg) -> Vec<u8> {
    server_msg_to_pb(m).encode_to_vec()
}
pub fn decode_server(bytes: &[u8]) -> R<ServerMsg> {
    server_msg_from_pb(pb::ServerMsg::decode(bytes)?)
}

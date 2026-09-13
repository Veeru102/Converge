//! Wire round-trips, and generation of `fixtures/wire.json` so the TypeScript
//! codec can be checked byte-for-byte against prost.

use converge_core::*;
use converge_proto::json as pj;
use converge_proto::wire::*;
use converge_proto::*;
use serde_json::json;

fn sample_ops() -> Vec<Op> {
    let a = OpId::new(u64::MAX - 5, 1);
    vec![
        Op {
            id: a,
            hlc: Hlc::new(1_700_000_000_123, 7),
            kind: OpKind::Create {
                kind: ObjectKind::Rect,
                props: vec![
                    ("x".into(), Value::F64(-12.5)),
                    ("fill".into(), Value::Color(0xff00ff00)),
                    ("z".into(), Value::FracIndex("a0V".into())),
                    ("n".into(), Value::Null),
                    ("b".into(), Value::Bool(true)),
                    ("i".into(), Value::I64(-9_007_199_254_740_993)),
                    ("s".into(), Value::Str("héllo 𝄞".into())),
                    ("r".into(), Value::ObjRef(OpId::new(1, 2))),
                ],
            },
        },
        Op {
            id: OpId::new(3, 9),
            hlc: Hlc::new(5, 65535),
            kind: OpKind::SetProps {
                object: a,
                entries: vec![("text".into(), Value::Str(String::new()))],
            },
        },
        Op {
            id: OpId::new(3, 10),
            hlc: Hlc::new(6, 0),
            kind: OpKind::Delete { object: a },
        },
        Op {
            id: OpId::new(3, 11),
            hlc: Hlc::new(7, 0),
            kind: OpKind::Restore { object: a },
        },
        Op {
            id: OpId::new(3, 12),
            hlc: Hlc::new(8, 0),
            kind: OpKind::Create {
                kind: ObjectKind::Ellipse,
                props: vec![("w".into(), Value::F64(10.0))],
            },
        },
        Op {
            id: OpId::new(3, 13),
            hlc: Hlc::new(9, 0),
            kind: OpKind::Create {
                kind: ObjectKind::Line,
                props: vec![("arrow".into(), Value::Bool(true))],
            },
        },
    ]
}

fn sample_client_msgs() -> Vec<ClientMsg> {
    vec![
        ClientMsg::Hello(Hello {
            version: PROTOCOL_VERSION,
            doc: DocId("doc-1".into()),
            replica: ReplicaId(u64::MAX),
            last_seq: 42,
            want_snapshot: true,
            user: UserInfo {
                name: "ann".into(),
                color: 0xabcdef,
            },
        }),
        ClientMsg::Submit { ops: sample_ops() },
        ClientMsg::Presence(PresenceState {
            cursor: Some((1.5, -2.0)),
            selection: vec![OpId::new(1, 1), OpId::new(2, 2)],
        }),
        ClientMsg::Presence(PresenceState {
            cursor: None,
            selection: vec![],
        }),
        ClientMsg::Pong { nonce: 77 },
    ]
}

fn sample_server_msgs() -> Vec<ServerMsg> {
    let ops = sample_ops();
    let mut doc = Document::new();
    for op in &ops {
        doc.apply(op);
    }
    vec![
        ServerMsg::Welcome {
            session: SessionId(9),
            server_time_ms: 1_700_000_000_000,
            durable_head_seq: 4,
            catch_up: CatchUp::Ops(
                ops.iter()
                    .cloned()
                    .enumerate()
                    .map(|(i, op)| (i as u64 + 1, op))
                    .collect(),
            ),
            presence: vec![PresenceEntry {
                replica: ReplicaId(5),
                user: UserInfo {
                    name: "bob".into(),
                    color: 1,
                },
                state: PresenceState {
                    cursor: Some((0.0, 0.0)),
                    selection: vec![],
                },
            }],
        },
        ServerMsg::Welcome {
            session: SessionId(10),
            server_time_ms: 1,
            durable_head_seq: 4,
            catch_up: CatchUp::Snapshot {
                bytes: codec::encode_snapshot(&doc),
                seq: 4,
            },
            presence: vec![],
        },
        ServerMsg::Bye {
            reason: ByeReason::Superseded,
        },
        ServerMsg::Commit {
            seq: 5,
            op: ops[1].clone(),
        },
        ServerMsg::Ack {
            op_id: OpId::new(3, 9),
            seq: Some(2),
        },
        ServerMsg::Ack {
            op_id: OpId::new(3, 9),
            seq: None,
        },
        ServerMsg::Nack {
            op_id: OpId::new(3, 9),
            reason: NackReason::ClockSkew,
        },
        ServerMsg::PresenceUpdate(PresenceEntry {
            replica: ReplicaId(5),
            user: UserInfo::default(),
            state: PresenceState::default(),
        }),
        ServerMsg::PresenceLeave {
            replica: ReplicaId(5),
        },
        ServerMsg::Ping { nonce: 1 },
    ]
}

#[test]
fn round_trips() {
    for m in sample_client_msgs() {
        assert_eq!(decode_client(&encode_client(&m)).unwrap(), m);
    }
    for m in sample_server_msgs() {
        assert_eq!(decode_server(&encode_server(&m)).unwrap(), m);
    }
    for op in sample_ops() {
        assert_eq!(decode_op(&encode_op(&op)).unwrap(), op);
    }
}

#[test]
fn rejects_garbage() {
    assert!(decode_client(&[0xff, 0xff, 0xff]).is_err());
    assert!(decode_server(&[]).is_err());
}

#[test]
#[ignore]
fn generate() {
    let client: Vec<_> = sample_client_msgs()
        .iter()
        .map(|m| json!({"json": pj::client_msg_to_json(m), "bytes": codec::hex(&encode_client(m))}))
        .collect();
    let server: Vec<_> = sample_server_msgs()
        .iter()
        .map(|m| json!({"json": pj::server_msg_to_json(m), "bytes": codec::hex(&encode_server(m))}))
        .collect();
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/wire.json");
    std::fs::write(
        path,
        serde_json::to_string_pretty(&json!({"client": client, "server": server})).unwrap(),
    )
    .unwrap();
}

#[test]
fn checked_in_wire_fixture_matches() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/wire.json");
    let j: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for c in j["client"].as_array().unwrap() {
        let m = pj::client_msg_from_json(&c["json"]).unwrap();
        assert_eq!(codec::hex(&encode_client(&m)), c["bytes"]);
    }
    for s in j["server"].as_array().unwrap() {
        let m = pj::server_msg_from_json(&s["json"]).unwrap();
        assert_eq!(codec::hex(&encode_server(&m)), s["bytes"]);
    }
}

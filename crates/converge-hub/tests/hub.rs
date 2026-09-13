use converge_core::*;
use converge_hub::{Effect, Hub, HubConfig};
use converge_proto::*;

fn hello(replica: u64, last_seq: u64) -> ClientMsg {
    ClientMsg::Hello(Hello {
        version: PROTOCOL_VERSION,
        doc: DocId("d".into()),
        replica: ReplicaId(replica),
        last_seq,
        want_snapshot: false,
        user: UserInfo::default(),
    })
}

fn create(replica: u64, counter: u64, wall: u64) -> Op {
    Op {
        id: OpId::new(replica, counter),
        hlc: Hlc::new(wall, 0),
        kind: OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("x".into(), Value::F64(counter as f64))],
        },
    }
}

fn hub() -> Hub {
    hub_with_capacity(100)
}

fn hub_with_capacity(log_capacity: usize) -> Hub {
    Hub::new(
        DocId("d".into()),
        HubConfig {
            skew_tolerance_ms: 1_000,
            log_capacity,
        },
    )
}

fn sends(out: &[Effect], to: SessionId) -> Vec<&ServerMsg> {
    out.iter()
        .filter_map(|e| match e {
            Effect::Send { to: t, msg } if *t == to => Some(msg),
            _ => None,
        })
        .collect()
}

fn persisted_seq(out: &[Effect]) -> Option<u64> {
    out.iter()
        .filter_map(|e| match e {
            Effect::Persist { ops } => ops.last().map(|(s, _)| *s),
            _ => None,
        })
        .next_back()
}

#[test]
fn newest_hello_evicts_previous_session_for_replica() {
    let mut h = hub();
    let mut out = vec![];
    h.handle(SessionId(1), hello(7, 0), 0, &mut out);
    out.clear();
    h.handle(SessionId(2), hello(7, 0), 0, &mut out);
    assert!(matches!(
        sends(&out, SessionId(1))[..],
        [ServerMsg::Bye {
            reason: ByeReason::Superseded
        }]
    ));
    assert!(out.contains(&Effect::Close {
        session: SessionId(1)
    }));
    assert!(matches!(
        sends(&out, SessionId(2))[..],
        [ServerMsg::Welcome { .. }]
    ));
    assert_eq!(h.session_count(), 1);
    // Late messages from the evicted session are ignored.
    out.clear();
    h.handle(
        SessionId(1),
        ClientMsg::Submit {
            ops: vec![create(7, 1, 0)],
        },
        0,
        &mut out,
    );
    assert!(out.is_empty());
}

#[test]
fn welcome_and_commits_reveal_only_durable_state() {
    let mut h = hub();
    let mut out = vec![];
    h.handle(SessionId(1), hello(7, 0), 0, &mut out);
    out.clear();
    h.handle(
        SessionId(1),
        ClientMsg::Submit {
            ops: (1..=5).map(|c| create(7, c, 10)).collect(),
        },
        10,
        &mut out,
    );
    assert_eq!(persisted_seq(&out), Some(5));
    assert!(
        sends(&out, SessionId(1)).is_empty(),
        "no Commit before durability"
    );
    assert_eq!(h.head_seq(), 5);
    assert_eq!(h.durable_seq(), 0);

    out.clear();
    h.durable(2, &mut out);
    let commits: Vec<u64> = sends(&out, SessionId(1))
        .iter()
        .filter_map(|m| match m {
            ServerMsg::Commit { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(commits, vec![1, 2]);
    assert_eq!(h.document().len(), 2);

    // A joiner sees durable_head = 2 and ops (0, 2].
    out.clear();
    h.handle(SessionId(2), hello(8, 0), 10, &mut out);
    match sends(&out, SessionId(2))[..] {
        [ServerMsg::Welcome {
            durable_head_seq,
            catch_up: CatchUp::Ops(ops),
            ..
        }] => {
            assert_eq!(*durable_head_seq, 2);
            assert_eq!(ops.iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![1, 2]);
        }
        ref other => panic!("unexpected {other:?}"),
    }
    // A duplicate submit of a non-durable op is silent; of a durable op is Acked.
    out.clear();
    h.handle(
        SessionId(1),
        ClientMsg::Submit {
            ops: vec![create(7, 4, 10), create(7, 2, 10)],
        },
        10,
        &mut out,
    );
    assert!(matches!(
        sends(&out, SessionId(1))[..],
        [ServerMsg::Ack { seq: Some(2), .. }]
    ));
    assert!(persisted_seq(&out).is_none());
}

#[test]
fn skew_rejection_closes_session_and_stops_the_batch() {
    let mut h = hub();
    let mut out = vec![];
    h.handle(SessionId(1), hello(7, 0), 100, &mut out);
    out.clear();
    let ops = vec![create(7, 1, 500), create(7, 2, 5_000), create(7, 3, 600)];
    h.handle(SessionId(1), ClientMsg::Submit { ops }, 100, &mut out);
    let msgs = sends(&out, SessionId(1));
    assert!(
        matches!(msgs[0], ServerMsg::Nack { reason: NackReason::ClockSkew, op_id } if op_id.counter == 2)
    );
    assert!(matches!(
        msgs[1],
        ServerMsg::Bye {
            reason: ByeReason::ClockSkew
        }
    ));
    assert!(out.contains(&Effect::Close {
        session: SessionId(1)
    }));
    assert_eq!(
        h.head_seq(),
        1,
        "op 3 must not be accepted after op 2 was rejected (H3)"
    );
    assert_eq!(h.session_count(), 0);
    // The replica can come straight back.
    out.clear();
    h.handle(SessionId(2), hello(7, 0), 100, &mut out);
    assert!(matches!(
        sends(&out, SessionId(2))[..],
        [ServerMsg::Welcome { .. }]
    ));
}

#[test]
fn catch_up_falls_back_to_snapshot_behind_the_log_floor() {
    let mut h = hub_with_capacity(4);
    let mut out = vec![];
    h.handle(SessionId(1), hello(7, 0), 0, &mut out);
    h.handle(
        SessionId(1),
        ClientMsg::Submit {
            ops: (1..=10).map(|c| create(7, c, 0)).collect(),
        },
        0,
        &mut out,
    );
    h.durable(10, &mut out);
    out.clear();
    h.handle(SessionId(2), hello(8, 3), 0, &mut out);
    assert!(matches!(
        sends(&out, SessionId(2))[..],
        [ServerMsg::Welcome {
            catch_up: CatchUp::Snapshot { seq: 10, .. },
            ..
        }]
    ));
    out.clear();
    h.handle(SessionId(3), hello(9, 7), 0, &mut out);
    match sends(&out, SessionId(3))[..] {
        [ServerMsg::Welcome {
            catch_up: CatchUp::Ops(ops),
            ..
        }] => assert_eq!(
            ops.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![8, 9, 10]
        ),
        ref o => panic!("{o:?}"),
    }
    // A client claiming to be ahead of the server gets a snapshot.
    out.clear();
    h.handle(SessionId(4), hello(10, 99), 0, &mut out);
    assert!(matches!(
        sends(&out, SessionId(4))[..],
        [ServerMsg::Welcome {
            catch_up: CatchUp::Snapshot { .. },
            ..
        }]
    ));
    // A duplicate of an op evicted from the log is Acked without a seq.
    out.clear();
    h.handle(
        SessionId(1),
        ClientMsg::Submit {
            ops: vec![create(7, 1, 0)],
        },
        0,
        &mut out,
    );
    assert!(matches!(
        sends(&out, SessionId(1))[..],
        [ServerMsg::Ack { seq: None, .. }]
    ));
}

#[test]
fn malformed_and_foreign_ops_are_nacked_permanently() {
    let mut h = hub();
    let mut out = vec![];
    h.handle(SessionId(1), hello(7, 0), 0, &mut out);
    out.clear();
    let bad = Op {
        id: OpId::new(7, 1),
        hlc: Hlc::MIN,
        kind: OpKind::SetProps {
            object: OpId::new(7, 1),
            entries: vec![("x".into(), Value::F64(f64::INFINITY))],
        },
    };
    h.handle(
        SessionId(1),
        ClientMsg::Submit {
            ops: vec![bad, create(8, 1, 0)],
        },
        0,
        &mut out,
    );
    let msgs = sends(&out, SessionId(1));
    assert!(matches!(
        msgs[0],
        ServerMsg::Nack {
            reason: NackReason::Malformed,
            ..
        }
    ));
    assert!(matches!(
        msgs[1],
        ServerMsg::Nack {
            reason: NackReason::Rejected,
            ..
        }
    ));
    assert_eq!(h.head_seq(), 0);
}

#[test]
fn recover_rebuilds_from_snapshot_and_tail() {
    let mut h = hub();
    let mut out = vec![];
    h.handle(SessionId(1), hello(7, 0), 0, &mut out);
    h.handle(
        SessionId(1),
        ClientMsg::Submit {
            ops: (1..=6).map(|c| create(7, c, 0)).collect(),
        },
        0,
        &mut out,
    );
    h.durable(4, &mut out);
    let (snap, seq) = h.snapshot();
    assert_eq!(seq, 4);
    let tail: Vec<(u64, Op)> = h
        .log()
        .filter(|(s, _)| *s > 2 && *s <= 4)
        .cloned()
        .collect();
    let snap2 = {
        let mut d = Document::new();
        for (_, op) in h.log().filter(|(s, _)| *s <= 2) {
            d.apply(op);
        }
        d
    };
    let r = Hub::recover(DocId("d".into()), HubConfig::default(), snap2, 2, tail);
    assert_eq!(r.head_seq(), 4);
    assert_eq!(r.durable_seq(), 4);
    assert_eq!(codec::encode_snapshot(r.document()), snap);
    // Ops 5 and 6 were never durable: resubmitting them is accepted again.
    let mut out = vec![];
    let mut r = r;
    r.handle(SessionId(9), hello(7, 4), 0, &mut out);
    out.clear();
    r.handle(
        SessionId(9),
        ClientMsg::Submit {
            ops: vec![create(7, 5, 0), create(7, 6, 0), create(7, 3, 0)],
        },
        0,
        &mut out,
    );
    assert_eq!(persisted_seq(&out), Some(6));
    assert!(matches!(
        sends(&out, SessionId(9))[..],
        [ServerMsg::Ack { seq: Some(3), .. }]
    ));
}

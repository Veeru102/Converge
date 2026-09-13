use converge_client_sync::{ClientConfig, ClientSync, ConnState, Output};
use converge_core::*;
use converge_proto::*;

fn client(replica: u64) -> ClientSync {
    ClientSync::new(
        DocId("d".into()),
        ReplicaId(replica),
        UserInfo::default(),
        ClientConfig::default(),
    )
}

fn remote_create(replica: u64, counter: u64, wall: u64) -> Op {
    Op {
        id: OpId::new(replica, counter),
        hlc: Hlc::new(wall, 0),
        kind: OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("x".into(), Value::F64(counter as f64))],
        },
    }
}

fn welcome(head: u64, ops: Vec<(u64, Op)>) -> ServerMsg {
    ServerMsg::Welcome {
        session: SessionId(1),
        server_time_ms: 1_000,
        durable_head_seq: head,
        catch_up: CatchUp::Ops(ops),
        presence: vec![],
    }
}

fn submits(out: &[Output]) -> Vec<&Op> {
    out.iter()
        .flat_map(|o| match o {
            Output::Send(ClientMsg::Submit { ops }) => ops.iter().collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect()
}

#[test]
fn ops_are_sent_only_after_local_persistence_and_in_order() {
    let mut c = client(1);
    let mut out = vec![];
    let g = c.connected(&mut out);
    c.message(g, welcome(0, vec![]), 1_000, &mut out);
    out.clear();
    let a = c.edit(
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![],
        },
        1_000,
        &mut out,
    );
    let b = c.edit(
        OpKind::SetProps {
            object: a.id,
            entries: vec![("x".into(), Value::F64(1.0))],
        },
        1_001,
        &mut out,
    );
    assert_eq!(
        out,
        vec![Output::Persist(a.clone()), Output::Persist(b.clone())]
    );
    out.clear();
    // Second write completes first: nothing can be sent yet (counter order).
    c.persisted(b.id.counter, &mut out);
    assert!(submits(&out).is_empty());
    c.persisted(a.id.counter, &mut out);
    assert_eq!(
        submits(&out)
            .iter()
            .map(|o| o.id.counter)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(a.hlc < b.hlc);
}

#[test]
fn gap_in_commits_keeps_last_seq_and_reconnects() {
    let mut c = client(1);
    let mut out = vec![];
    let g = c.connected(&mut out);
    c.message(g, welcome(0, vec![]), 1_000, &mut out);
    c.message(
        g,
        ServerMsg::Commit {
            seq: 1,
            op: remote_create(2, 1, 1_000),
        },
        1_000,
        &mut out,
    );
    c.message(
        g,
        ServerMsg::Commit {
            seq: 2,
            op: remote_create(2, 2, 1_000),
        },
        1_000,
        &mut out,
    );
    out.clear();
    c.message(
        g,
        ServerMsg::Commit {
            seq: 4,
            op: remote_create(2, 4, 1_000),
        },
        1_000,
        &mut out,
    );
    assert_eq!(c.last_seq(), 2);
    assert_eq!(c.document().len(), 3, "the op itself is still applied");
    assert_eq!(out, vec![Output::Reconnect]);
    // Messages on the dead generation are ignored.
    c.message(
        g,
        ServerMsg::Commit {
            seq: 3,
            op: remote_create(2, 3, 1_000),
        },
        1_000,
        &mut out,
    );
    assert_eq!(c.last_seq(), 2);
    out.clear();
    let g2 = c.connected(&mut out);
    assert!(
        matches!(&out[0], Output::Send(ClientMsg::Hello(h)) if h.last_seq == 2 && !h.want_snapshot)
    );
    c.message(
        g2,
        welcome(
            4,
            vec![
                (3, remote_create(2, 3, 1_000)),
                (4, remote_create(2, 4, 1_000)),
            ],
        ),
        1_000,
        &mut out,
    );
    assert_eq!(c.last_seq(), 4);
    assert_eq!(c.state(), ConnState::Live);
}

#[test]
fn permanent_nack_drops_op_and_forces_snapshot_resync() {
    let mut c = client(1);
    let mut out = vec![];
    let g = c.connected(&mut out);
    c.message(g, welcome(0, vec![]), 1_000, &mut out);
    let op = c.edit(
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![],
        },
        1_000,
        &mut out,
    );
    c.persisted(1, &mut out);
    out.clear();
    c.message(
        g,
        ServerMsg::Nack {
            op_id: op.id,
            reason: NackReason::Malformed,
        },
        1_000,
        &mut out,
    );
    assert_eq!(out, vec![Output::Reconnect]);
    assert_eq!(c.pending_len(), 0);
    out.clear();
    let g2 = c.connected(&mut out);
    assert!(matches!(&out[0], Output::Send(ClientMsg::Hello(h)) if h.want_snapshot));
    let empty = codec::encode_snapshot(&Document::new());
    c.message(
        g2,
        ServerMsg::Welcome {
            session: SessionId(2),
            server_time_ms: 1_000,
            durable_head_seq: 0,
            catch_up: CatchUp::Snapshot {
                bytes: empty,
                seq: 0,
            },
            presence: vec![],
        },
        1_000,
        &mut out,
    );
    assert_eq!(c.document().len(), 0, "phantom effect removed");
}

#[test]
fn own_ops_in_catch_up_are_acked_and_dropped_at_snapshot() {
    let mut c = client(1);
    let mut out = vec![];
    let g = c.connected(&mut out);
    c.message(g, welcome(0, vec![]), 1_000, &mut out);
    let a = c.edit(
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![],
        },
        1_000,
        &mut out,
    );
    c.persisted(1, &mut out);
    c.disconnected();
    out.clear();
    let g2 = c.connected(&mut out);
    c.message(g2, welcome(1, vec![(1, a.clone())]), 1_000, &mut out);
    assert!(submits(&out).is_empty(), "committed op must not be resent");
    assert_eq!(c.unacked().count(), 0);
    let snap = c.snapshot();
    assert_eq!(snap.acked, vec![1]);
    assert_eq!(snap.seq, 1);
    c.snapshot_persisted(&snap.acked);
    assert_eq!(c.pending_len(), 0);
}

#[test]
fn far_future_pending_ops_are_retimestamped_in_counter_order() {
    // Client clock is ten minutes ahead while offline.
    let mut c = client(1);
    let mut out = vec![];
    let skew = 600_000u64;
    let a = c.edit(
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("x".into(), Value::F64(1.0))],
        },
        1_000 + skew,
        &mut out,
    );
    let b = c.edit(
        OpKind::SetProps {
            object: a.id,
            entries: vec![("x".into(), Value::F64(2.0))],
        },
        1_000 + skew,
        &mut out,
    );
    let d = c.edit(
        OpKind::SetProps {
            object: a.id,
            entries: vec![("x".into(), Value::F64(3.0))],
        },
        1_001 + skew,
        &mut out,
    );
    for i in 1..=3 {
        c.persisted(i, &mut out);
    }
    out.clear();
    let g = c.connected(&mut out);
    out.clear();
    // Server says the time is 1_000 while the client thinks it is 601_000.
    c.message(
        g,
        ServerMsg::Welcome {
            session: SessionId(1),
            server_time_ms: 1_000,
            durable_head_seq: 0,
            catch_up: CatchUp::Ops(vec![]),
            presence: vec![],
        },
        1_000 + skew,
        &mut out,
    );
    assert_eq!(
        out,
        vec![Output::Reconnect],
        "must rebuild from a snapshot before re-stamping"
    );
    out.clear();
    c.disconnected();
    let g = c.connected(&mut out);
    assert!(matches!(&out[0], Output::Send(ClientMsg::Hello(h)) if h.want_snapshot));
    out.clear();
    let empty = codec::encode_snapshot(&Document::new());
    c.message(
        g,
        ServerMsg::Welcome {
            session: SessionId(2),
            server_time_ms: 1_000,
            durable_head_seq: 0,
            catch_up: CatchUp::Snapshot {
                bytes: empty,
                seq: 0,
            },
            presence: vec![],
        },
        1_000 + skew,
        &mut out,
    );
    let sent = submits(&out);
    assert_eq!(
        sent.iter().map(|o| o.id).collect::<Vec<_>>(),
        vec![a.id, b.id, d.id],
        "same ids, counter order"
    );
    assert!(
        sent[0].hlc < sent[1].hlc && sent[1].hlc < sent[2].hlc,
        "strictly increasing"
    );
    assert!(
        sent[2].hlc.wall_ms <= 1_000 + 1,
        "re-timestamped to server time"
    );
    assert!(sent[0].hlc.wall_ms >= 1_000);
    // Local state equals a fresh replica applying the re-timestamped ops.
    let mut fresh = Document::new();
    for op in &sent {
        fresh.apply(op);
    }
    assert_eq!(codec::hash(&fresh), c.hash());
    assert_eq!(
        c.document().get(&a.id).unwrap().get("x"),
        Some(&Value::F64(3.0))
    );
}

#[test]
fn skew_nack_retimestamps_from_the_rejected_op_and_keeps_earlier_ones() {
    let mut c = client(1);
    let mut out = vec![];
    let g = c.connected(&mut out);
    c.message(g, welcome(0, vec![]), 1_000, &mut out);
    let a = c.edit(
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![],
        },
        1_000,
        &mut out,
    );
    c.persisted(1, &mut out);
    // Clock jumps forward mid-session.
    let b = c.edit(
        OpKind::SetProps {
            object: a.id,
            entries: vec![("x".into(), Value::F64(1.0))],
        },
        900_000,
        &mut out,
    );
    let d = c.edit(
        OpKind::SetProps {
            object: a.id,
            entries: vec![("y".into(), Value::F64(1.0))],
        },
        900_001,
        &mut out,
    );
    c.persisted(2, &mut out);
    c.persisted(3, &mut out);
    out.clear();
    // Server accepted a (Commit follows later) and rejected b for skew; d never processed.
    c.message(
        g,
        ServerMsg::Nack {
            op_id: b.id,
            reason: NackReason::ClockSkew,
        },
        900_001,
        &mut out,
    );
    c.message(
        g,
        ServerMsg::Bye {
            reason: ByeReason::ClockSkew,
        },
        900_001,
        &mut out,
    );
    assert_eq!(c.state(), ConnState::Disconnected);
    c.disconnected();
    let g2 = c.connected(&mut out);
    out.clear();
    c.message(
        g2,
        ServerMsg::Welcome {
            session: SessionId(2),
            server_time_ms: 2_000,
            durable_head_seq: 1,
            catch_up: CatchUp::Ops(vec![(1, a.clone())]),
            presence: vec![],
        },
        900_002,
        &mut out,
    );
    assert_eq!(out, vec![Output::Reconnect]);
    assert_eq!(c.last_seq(), 1);
    c.disconnected();
    out.clear();
    let g3 = c.connected(&mut out);
    out.clear();
    let mut base = Document::new();
    base.apply(&a);
    c.message(
        g3,
        ServerMsg::Welcome {
            session: SessionId(3),
            server_time_ms: 2_001,
            durable_head_seq: 1,
            catch_up: CatchUp::Snapshot {
                bytes: codec::encode_snapshot(&base),
                seq: 1,
            },
            presence: vec![],
        },
        900_003,
        &mut out,
    );
    let sent = submits(&out);
    assert_eq!(
        sent.iter().map(|o| o.id).collect::<Vec<_>>(),
        vec![b.id, d.id]
    );
    assert!(
        sent[0].hlc > a.hlc,
        "re-timestamped ops stay after the accepted op (H1)"
    );
    assert!(sent[0].hlc < sent[1].hlc);
    assert!(sent[0].hlc.wall_ms <= 2_100);
    assert_eq!(c.unacked().count(), 2);
}

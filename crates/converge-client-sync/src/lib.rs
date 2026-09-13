//! Sans-I/O client sync engine — the reference implementation of the client
//! side of the protocol, used by the simulator and mirrored by
//! `@converge/sync` in TypeScript.
//!
//! The engine owns the local [`Document`], the HLC, the pending queue and the
//! resume bookkeeping. It never touches a socket, a store or a clock: the
//! driver feeds it events with `now_ms` (the raw local clock) and executes
//! the [`Output`]s. Store durability is modelled explicitly so that the
//! crash windows in `docs/IMPLEMENTATION_RISKS.md` §2 are reachable.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use converge_core::{codec, Document, Hash, Hlc, HlcClock, Op, OpKind, ReplicaId};
use converge_proto::{
    ByeReason, CatchUp, ClientMsg, DocId, Hello, NackReason, PresenceState, ServerMsg, UserInfo,
    MAX_SUBMIT_OPS, PROTOCOL_VERSION,
};

/// What the driver must do.
#[derive(Clone, PartialEq, Debug)]
pub enum Output {
    /// Send on the current connection.
    Send(ClientMsg),
    /// Write to the local store, then call [`ClientSync::persisted`].
    Persist(Op),
    /// Drop the connection and reconnect (gap detected, or a resync is
    /// required); the next `Hello` is built by [`ClientSync::connected`].
    Reconnect,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnState {
    Disconnected,
    HelloSent,
    Live,
}

#[derive(Clone, PartialEq, Debug)]
struct Pending {
    op: Op,
    /// The local store confirmed the write (D1: never sent before this).
    durable: bool,
    /// Ever transmitted on any connection. Once true the server may hold the
    /// op, so its HLC may no longer change unless a NACK proves otherwise.
    sent: bool,
    /// Committed by the server (Commit or Ack seen). Dropped from the queue
    /// at the next snapshot write (D2).
    acked: bool,
}

/// Everything needed to write the local snapshot record (D2/D3/H6).
#[derive(Clone, PartialEq, Debug)]
pub struct SnapshotWrite {
    pub bytes: Vec<u8>,
    pub seq: u64,
    pub high_water: Hlc,
    /// Pending counters to delete in the same transaction.
    pub acked: Vec<u64>,
}

/// Persisted replica row.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ReplicaRow {
    pub next_counter: u64,
    pub high_water: Hlc,
    pub clock_offset_ms: i64,
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Ops whose timestamp is further ahead of server time than this are
    /// re-timestamped before being (re)sent; half the server tolerance.
    pub retimestamp_margin_ms: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            retimestamp_margin_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClientSync {
    cfg: ClientConfig,
    doc_id: DocId,
    user: UserInfo,
    replica: ReplicaId,
    doc: Document,
    clock: HlcClock,
    next_counter: u64,
    pending: BTreeMap<u64, Pending>,
    last_seq: u64,
    state: ConnState,
    /// Connection generation; messages tagged with another generation are
    /// dropped (H5).
    gen: u64,
    offset_ms: i64,
    /// Counter of the first op the server NACKed for skew on the last
    /// session; everything from here on is known-unaccepted (H3).
    first_skew_nack: Option<u64>,
    want_snapshot: bool,
    /// The last snapshot write handed out and not yet confirmed.
    resync_pending: bool,
}

impl ClientSync {
    pub fn new(doc_id: DocId, replica: ReplicaId, user: UserInfo, cfg: ClientConfig) -> Self {
        ClientSync {
            cfg,
            doc_id,
            user,
            replica,
            doc: Document::new(),
            clock: HlcClock::new(),
            next_counter: 1,
            pending: BTreeMap::new(),
            last_seq: 0,
            state: ConnState::Disconnected,
            gen: 0,
            offset_ms: 0,
            first_skew_nack: None,
            want_snapshot: false,
            resync_pending: false,
        }
    }

    /// Rebuild from the local store after a reload or crash. Pending ops
    /// are treated as possibly-sent (conservative: they may be on the server).
    pub fn restore(
        doc_id: DocId,
        replica: ReplicaId,
        user: UserInfo,
        cfg: ClientConfig,
        row: ReplicaRow,
        snapshot: Option<(Vec<u8>, u64)>,
        pending: Vec<Op>,
    ) -> Self {
        let mut c = ClientSync::new(doc_id, replica, user, cfg);
        c.next_counter = row.next_counter.max(1);
        c.clock = HlcClock::from_high_water(row.high_water);
        c.offset_ms = row.clock_offset_ms;
        if let Some((bytes, seq)) = snapshot {
            c.doc = codec::decode_snapshot(&bytes).expect("local snapshot decodes");
            c.last_seq = seq;
        }
        for op in pending {
            assert_eq!(op.id.replica, replica);
            c.doc.apply(&op);
            c.clock.observe(op.hlc, 0);
            c.pending.insert(
                op.id.counter,
                Pending {
                    op,
                    durable: true,
                    sent: true,
                    acked: false,
                },
            );
        }
        c
    }

    // ----- accessors -----

    pub fn replica(&self) -> ReplicaId {
        self.replica
    }
    pub fn document(&self) -> &Document {
        &self.doc
    }
    pub fn hash(&self) -> Hash {
        codec::hash(&self.doc)
    }
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }
    pub fn state(&self) -> ConnState {
        self.state
    }
    pub fn generation(&self) -> u64 {
        self.gen
    }
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
    /// Pending ops not yet confirmed by the server.
    pub fn unacked(&self) -> impl Iterator<Item = &Op> {
        self.pending.values().filter(|p| !p.acked).map(|p| &p.op)
    }
    pub fn pending_ops(&self) -> impl Iterator<Item = &Op> {
        self.pending.values().map(|p| &p.op)
    }
    pub fn replica_row(&self) -> ReplicaRow {
        ReplicaRow {
            next_counter: self.next_counter,
            high_water: self.clock.high_water(),
            clock_offset_ms: self.offset_ms,
        }
    }
    fn server_now(&self, now_ms: u64) -> u64 {
        (now_ms as i64 + self.offset_ms).max(0) as u64
    }

    // ----- connection lifecycle -----

    /// The transport opened. Returns the generation the driver must tag
    /// inbound messages with.
    pub fn connected(&mut self, out: &mut Vec<Output>) -> u64 {
        self.gen += 1;
        self.state = ConnState::HelloSent;
        out.push(Output::Send(ClientMsg::Hello(Hello {
            version: PROTOCOL_VERSION,
            doc: self.doc_id.clone(),
            replica: self.replica,
            last_seq: self.last_seq,
            want_snapshot: self.want_snapshot,
            user: self.user.clone(),
        })));
        self.gen
    }

    pub fn disconnected(&mut self) {
        self.state = ConnState::Disconnected;
    }

    // ----- local edits and store -----

    /// Author an op: applied locally at once, persisted before it is sent.
    pub fn edit(&mut self, kind: OpKind, now_ms: u64, out: &mut Vec<Output>) -> Op {
        let hlc = self.clock.tick(self.server_now(now_ms));
        let id = converge_core::OpId {
            replica: self.replica,
            counter: self.next_counter,
        };
        self.next_counter += 1;
        let op = Op { id, hlc, kind };
        debug_assert!(op.validate().is_ok());
        self.doc.apply(&op);
        self.pending.insert(
            id.counter,
            Pending {
                op: op.clone(),
                durable: false,
                sent: false,
                acked: false,
            },
        );
        out.push(Output::Persist(op.clone()));
        op
    }

    /// The store confirmed the pending write for `counter`.
    pub fn persisted(&mut self, counter: u64, out: &mut Vec<Output>) {
        if let Some(p) = self.pending.get_mut(&counter) {
            p.durable = true;
        }
        if self.state == ConnState::Live {
            self.flush(out);
        }
    }

    /// Build the next snapshot record. The driver writes it atomically and
    /// then calls [`ClientSync::snapshot_persisted`].
    pub fn snapshot(&self) -> SnapshotWrite {
        SnapshotWrite {
            bytes: codec::encode_snapshot(&self.doc),
            seq: self.last_seq,
            high_water: self.clock.high_water(),
            acked: self
                .pending
                .iter()
                .filter(|(_, p)| p.acked)
                .map(|(c, _)| *c)
                .collect(),
        }
    }

    pub fn snapshot_persisted(&mut self, acked: &[u64]) {
        for c in acked {
            if self.pending.get(c).map(|p| p.acked).unwrap_or(false) {
                self.pending.remove(c);
            }
        }
    }

    /// Send every durable, unacked op in counter order, stopping at the
    /// first non-durable one (ops are always transmitted in counter order).
    fn flush(&mut self, out: &mut Vec<Output>) {
        let mut batch: Vec<Op> = Vec::new();
        for p in self.pending.values_mut() {
            if p.acked {
                continue;
            }
            if !p.durable {
                break;
            }
            if !p.sent || self.resync_pending {
                p.sent = true;
                batch.push(p.op.clone());
            }
            if batch.len() == MAX_SUBMIT_OPS {
                out.push(Output::Send(ClientMsg::Submit {
                    ops: std::mem::take(&mut batch),
                }));
            }
        }
        if !batch.is_empty() {
            out.push(Output::Send(ClientMsg::Submit { ops: batch }));
        }
        self.resync_pending = false;
    }

    pub fn presence(&self, state: PresenceState, out: &mut Vec<Output>) {
        if self.state == ConnState::Live {
            out.push(Output::Send(ClientMsg::Presence(state)));
        }
    }

    // ----- inbound -----

    pub fn message(&mut self, gen: u64, msg: ServerMsg, now_ms: u64, out: &mut Vec<Output>) {
        if gen != self.gen || self.state == ConnState::Disconnected {
            return;
        }
        match msg {
            ServerMsg::Welcome {
                server_time_ms,
                durable_head_seq,
                catch_up,
                ..
            } => self.welcome(server_time_ms, durable_head_seq, catch_up, now_ms, out),
            ServerMsg::Commit { seq, op } => self.commit(seq, op, now_ms, out),
            ServerMsg::Ack { op_id, .. } => {
                if op_id.replica == self.replica {
                    if let Some(p) = self.pending.get_mut(&op_id.counter) {
                        p.acked = true;
                    }
                }
            }
            ServerMsg::Nack { op_id, reason } => self.nack(op_id.counter, reason, out),
            ServerMsg::Bye { reason } => {
                self.state = ConnState::Disconnected;
                if reason == ByeReason::VersionMismatch {
                    // Nothing the client can do; the driver decides.
                }
            }
            ServerMsg::PresenceUpdate(_) | ServerMsg::PresenceLeave { .. } => {}
            ServerMsg::Ping { nonce } => out.push(Output::Send(ClientMsg::Pong { nonce })),
        }
    }

    fn welcome(
        &mut self,
        server_time_ms: u64,
        durable_head: u64,
        catch_up: CatchUp,
        now_ms: u64,
        out: &mut Vec<Output>,
    ) {
        self.offset_ms = server_time_ms as i64 - now_ms as i64;
        let server_now = self.server_now(now_ms);
        match catch_up {
            CatchUp::Ops(ops) => {
                for (seq, op) in ops {
                    debug_assert_eq!(seq, self.last_seq + 1, "catch-up must be contiguous (P5)");
                    self.absorb(&op, server_now);
                    self.last_seq = seq;
                }
                debug_assert_eq!(self.last_seq, durable_head);
                if self.retimestamp_needed(server_now) {
                    // Re-stamped ops carry *lower* stamps than the skewed
                    // originals, so they cannot be re-applied over the local
                    // state; rebuild it from a snapshot instead (P7 path).
                    self.want_snapshot = true;
                    self.state = ConnState::Disconnected;
                    out.push(Output::Reconnect);
                    return;
                }
                self.first_skew_nack = None;
            }
            CatchUp::Snapshot { bytes, seq } => {
                let mut doc = codec::decode_snapshot(&bytes).expect("server snapshot decodes");
                let max_stamp = max_hlc(&doc);
                self.clock.observe(max_stamp, server_now);
                if self.retimestamp_needed(server_now) {
                    self.retimestamp(&doc, server_now);
                }
                self.first_skew_nack = None;
                // Durable state plus our own pending ops (commutative, so exact).
                for p in self.pending.values() {
                    doc.apply(&p.op);
                }
                self.doc = doc;
                self.last_seq = seq;
            }
        }
        self.want_snapshot = false;
        self.state = ConnState::Live;
        self.resync_pending = true; // resend everything unacked
        self.flush(out);
    }

    /// First counter of the known-unaccepted suffix of the pending queue:
    /// ops from the first skew NACK on (H3) and ops never transmitted.
    fn retimestamp_start(&self) -> Option<u64> {
        let first_unsent = self.pending.iter().find(|(_, p)| !p.sent).map(|(c, _)| *c);
        match (self.first_skew_nack, first_unsent) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn retimestamp_needed(&self, server_now: u64) -> bool {
        let Some(start) = self.retimestamp_start() else {
            return false;
        };
        let limit = server_now.saturating_add(self.cfg.retimestamp_margin_ms);
        self.pending
            .range(start..)
            .any(|(_, p)| p.op.hlc.wall_ms > limit)
    }

    /// H4: re-stamp the known-unaccepted suffix in counter order with the
    /// corrected clock. `base` is the server's durable state; the clock is
    /// reset to the highest stamp that can still be accepted anywhere (what
    /// the server holds plus our own ops before the suffix) — the one place
    /// where `wall` may go down.
    fn retimestamp(&mut self, base: &Document, server_now: u64) {
        let Some(start) = self.retimestamp_start() else {
            return;
        };
        let mut floor = max_hlc(base).max(Hlc::new(server_now, 0));
        for (_, p) in self.pending.range(..start) {
            floor = floor.max(p.op.hlc);
        }
        self.clock = HlcClock::from_high_water(floor);
        let counters: Vec<u64> = self.pending.range(start..).map(|(c, _)| *c).collect();
        for c in counters {
            let hlc = self.clock.tick(server_now);
            let p = self.pending.get_mut(&c).unwrap();
            debug_assert!(
                !p.acked,
                "an acked op can never be in the re-timestamp suffix (H3)"
            );
            p.op.hlc = hlc;
            p.sent = false;
        }
    }

    fn absorb(&mut self, op: &Op, server_now: u64) {
        self.clock.observe(op.hlc, server_now);
        if op.id.replica == self.replica {
            if let Some(p) = self.pending.get_mut(&op.id.counter) {
                debug_assert_eq!(p.op.hlc, op.hlc, "one HLC per OpId (H2)");
                p.acked = true;
            }
        }
        self.doc.apply(op);
    }

    fn commit(&mut self, seq: u64, op: Op, now_ms: u64, out: &mut Vec<Output>) {
        if seq <= self.last_seq {
            return;
        }
        let server_now = self.server_now(now_ms);
        self.absorb(&op, server_now);
        if seq == self.last_seq + 1 {
            self.last_seq = seq;
        } else {
            // P1: a gap — keep last_seq and resume from it.
            self.state = ConnState::Disconnected;
            out.push(Output::Reconnect);
        }
    }

    fn nack(&mut self, counter: u64, reason: NackReason, out: &mut Vec<Output>) {
        match reason {
            NackReason::ClockSkew => {
                self.first_skew_nack =
                    Some(self.first_skew_nack.map_or(counter, |c| c.min(counter)));
                // Bye follows; the driver reconnects.
            }
            NackReason::Malformed | NackReason::Rejected => {
                // P7: the local effect cannot be undone; drop it and resync.
                self.pending.remove(&counter);
                self.want_snapshot = true;
                self.state = ConnState::Disconnected;
                out.push(Output::Reconnect);
            }
        }
    }
}

fn max_hlc(doc: &Document) -> Hlc {
    let mut m = Hlc::MIN;
    for (_, obj) in doc.objects() {
        m = m.max(obj.deleted.at.hlc);
        for r in obj.props.values() {
            m = m.max(r.at.hlc);
        }
    }
    m
}

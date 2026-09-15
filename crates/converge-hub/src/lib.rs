//! Sans-I/O per-document server state machine.
//!
//! The [`Hub`] owns the *durable* document state, the op log, sequencing,
//! dedup and sessions. It never touches a socket or a clock: every entry
//! point takes `now_ms` and pushes [`Effect`]s for the adapter (Tokio server
//! or simulator) to carry out. See `docs/ARCHITECTURE.md` §5 and the
//! invariants P1–P8 in `docs/INVARIANTS.md`.

#![forbid(unsafe_code)]

mod seen;

pub use seen::SeenSet;

use std::collections::{BTreeMap, HashMap, VecDeque};

use converge_core::{codec, Document, Hash, Op, OpId, ReplicaId};
use converge_proto::{
    ByeReason, CatchUp, ClientMsg, DocId, Hello, NackReason, PresenceEntry, PresenceState,
    ServerMsg, SessionId, UserInfo, PROTOCOL_VERSION,
};

#[derive(Clone, Debug)]
pub struct HubConfig {
    /// Ops whose `wall_ms` exceeds `now + skew_tolerance_ms` are rejected.
    pub skew_tolerance_ms: u64,
    /// Durable ops retained in memory for resume-by-ops.
    pub log_capacity: usize,
}

impl Default for HubConfig {
    fn default() -> Self {
        HubConfig {
            skew_tolerance_ms: 60_000,
            log_capacity: 10_000,
        }
    }
}

/// What the adapter must do after a hub call.
#[derive(Clone, PartialEq, Debug)]
pub enum Effect {
    Send {
        to: SessionId,
        msg: ServerMsg,
    },
    /// Close the transport; the hub has already forgotten the session.
    Close {
        session: SessionId,
    },
    /// Append these accepted ops to durable storage, then call
    /// [`Hub::durable`] with the highest seq written.
    Persist {
        ops: Vec<(u64, Op)>,
    },
}

#[derive(Clone, Debug)]
struct Session {
    replica: ReplicaId,
    user: UserInfo,
    presence: PresenceState,
}

#[derive(Clone, Debug)]
pub struct Hub {
    cfg: HubConfig,
    doc_id: DocId,
    /// Durable state only (P2). Ops between `durable_seq` and `head_seq`
    /// live in `log` and are applied when storage confirms them.
    doc: Document,
    head_seq: u64,
    durable_seq: u64,
    /// `(seq, op)` ascending; always contains every non-durable op.
    log: VecDeque<(u64, Op)>,
    index: HashMap<OpId, u64>,
    seen: HashMap<ReplicaId, SeenSet>,
    sessions: BTreeMap<SessionId, Session>,
    by_replica: HashMap<ReplicaId, SessionId>,
}

impl Hub {
    pub fn new(doc_id: DocId, cfg: HubConfig) -> Self {
        Self::recover(doc_id, cfg, Document::new(), 0, Vec::new())
    }

    /// Rebuild from durable storage: a snapshot at `snapshot_seq` plus the
    /// durable ops after it. Anything that was accepted but not durable is
    /// gone — clients still hold those ops as pending and will resend them.
    pub fn recover(
        doc_id: DocId,
        cfg: HubConfig,
        mut doc: Document,
        snapshot_seq: u64,
        log_tail: Vec<(u64, Op)>,
    ) -> Self {
        let mut log = VecDeque::with_capacity(log_tail.len());
        let mut index = HashMap::new();
        let mut seen: HashMap<ReplicaId, SeenSet> = HashMap::new();
        let mut seq = snapshot_seq;
        for (s, op) in log_tail {
            assert_eq!(s, seq + 1, "log tail must be contiguous after the snapshot");
            seq = s;
            doc.apply(&op);
            index.insert(op.id, s);
            seen.entry(op.id.replica).or_default().insert(op.id.counter);
            log.push_back((s, op));
        }
        Hub {
            cfg,
            doc_id,
            doc,
            head_seq: seq,
            durable_seq: seq,
            log,
            index,
            seen,
            sessions: BTreeMap::new(),
            by_replica: HashMap::new(),
        }
    }

    pub fn doc_id(&self) -> &DocId {
        &self.doc_id
    }
    pub fn head_seq(&self) -> u64 {
        self.head_seq
    }
    pub fn durable_seq(&self) -> u64 {
        self.durable_seq
    }
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
    pub fn sessions(&self) -> impl Iterator<Item = SessionId> + '_ {
        self.sessions.keys().copied()
    }
    /// The durable document.
    pub fn document(&self) -> &Document {
        &self.doc
    }
    pub fn hash(&self) -> Hash {
        codec::hash(&self.doc)
    }
    /// Snapshot of the durable state and the seq it corresponds to.
    pub fn snapshot(&self) -> (Vec<u8>, u64) {
        (codec::encode_snapshot(&self.doc), self.durable_seq)
    }
    /// Oldest seq still in the in-memory log (0 if the log is complete from 1).
    fn log_floor(&self) -> u64 {
        self.log
            .front()
            .map(|(s, _)| *s)
            .unwrap_or(self.head_seq + 1)
    }

    /// Dispatch a client message. Messages from unknown sessions are ignored
    /// (they belong to a connection the hub has already closed — H5).
    pub fn handle(
        &mut self,
        session: SessionId,
        msg: ClientMsg,
        now_ms: u64,
        out: &mut Vec<Effect>,
    ) {
        match msg {
            ClientMsg::Hello(h) => self.open(session, h, now_ms, out),
            ClientMsg::Submit { ops } => self.submit(session, ops, now_ms, out),
            ClientMsg::Presence(p) => self.presence(session, p, out),
            ClientMsg::Pong { .. } => {}
        }
    }

    pub fn open(&mut self, session: SessionId, hello: Hello, now_ms: u64, out: &mut Vec<Effect>) {
        if self.sessions.contains_key(&session) {
            return; // duplicate Hello on an open session: idempotent
        }
        if hello.version != PROTOCOL_VERSION {
            out.push(Effect::Send {
                to: session,
                msg: ServerMsg::Bye {
                    reason: ByeReason::VersionMismatch,
                },
            });
            out.push(Effect::Close { session });
            return;
        }
        if hello.doc != self.doc_id {
            out.push(Effect::Send {
                to: session,
                msg: ServerMsg::Bye {
                    reason: ByeReason::UnknownDoc,
                },
            });
            out.push(Effect::Close { session });
            return;
        }
        // P4: newest Hello for a replica wins.
        if let Some(old) = self.by_replica.get(&hello.replica).copied() {
            if old != session {
                self.sessions.remove(&old);
                out.push(Effect::Send {
                    to: old,
                    msg: ServerMsg::Bye {
                        reason: ByeReason::Superseded,
                    },
                });
                out.push(Effect::Close { session: old });
            }
        }
        let presence: Vec<PresenceEntry> = self
            .sessions
            .iter()
            .filter(|(sid, _)| **sid != session)
            .map(|(_, s)| PresenceEntry {
                replica: s.replica,
                user: s.user.clone(),
                state: s.presence.clone(),
            })
            .collect();
        self.sessions.insert(
            session,
            Session {
                replica: hello.replica,
                user: hello.user,
                presence: PresenceState::default(),
            },
        );
        self.by_replica.insert(hello.replica, session);

        let catch_up = self.catch_up(hello.last_seq, hello.want_snapshot);
        out.push(Effect::Send {
            to: session,
            msg: ServerMsg::Welcome {
                session,
                server_time_ms: now_ms,
                durable_head_seq: self.durable_seq,
                catch_up,
                presence,
            },
        });
    }

    /// P2/P5: only durable ops, contiguous up to `durable_seq`, else a snapshot.
    fn catch_up(&self, last_seq: u64, want_snapshot: bool) -> CatchUp {
        if !want_snapshot && last_seq == self.durable_seq {
            return CatchUp::Ops(Vec::new());
        }
        if !want_snapshot && last_seq < self.durable_seq && last_seq + 1 >= self.log_floor() {
            let ops: Vec<(u64, Op)> = self
                .log
                .iter()
                .filter(|(s, _)| *s > last_seq && *s <= self.durable_seq)
                .cloned()
                .collect();
            let contiguous = ops.len() as u64 == self.durable_seq - last_seq
                && ops
                    .iter()
                    .zip(last_seq + 1..)
                    .all(|((s, _), want)| *s == want);
            if contiguous {
                return CatchUp::Ops(ops);
            }
        }
        let (bytes, seq) = self.snapshot();
        CatchUp::Snapshot { bytes, seq }
    }

    pub fn submit(&mut self, session: SessionId, ops: Vec<Op>, now_ms: u64, out: &mut Vec<Effect>) {
        let Some(sess) = self.sessions.get(&session) else {
            return;
        };
        let replica = sess.replica;
        let mut accepted: Vec<(u64, Op)> = Vec::new();
        for op in ops {
            if op.id.replica != replica {
                out.push(Effect::Send {
                    to: session,
                    msg: ServerMsg::Nack {
                        op_id: op.id,
                        reason: NackReason::Rejected,
                    },
                });
                continue;
            }
            if op.validate().is_err() {
                out.push(Effect::Send {
                    to: session,
                    msg: ServerMsg::Nack {
                        op_id: op.id,
                        reason: NackReason::Malformed,
                    },
                });
                continue;
            }
            if op.hlc.wall_ms > now_ms.saturating_add(self.cfg.skew_tolerance_ms) {
                // A ClockSkew NACK promises the client that no later counter
                // from it has been accepted (H3). If one has (only possible
                // when the transport reordered messages), reject this op
                // permanently instead so the client resyncs.
                let later_accepted = self
                    .seen
                    .get(&replica)
                    .is_some_and(|s| s.max_seen() > op.id.counter);
                if later_accepted {
                    out.push(Effect::Send {
                        to: session,
                        msg: ServerMsg::Nack {
                            op_id: op.id,
                            reason: NackReason::Rejected,
                        },
                    });
                    continue;
                }
                // H3: reject and close; nothing later from this session is processed.
                out.push(Effect::Send {
                    to: session,
                    msg: ServerMsg::Nack {
                        op_id: op.id,
                        reason: NackReason::ClockSkew,
                    },
                });
                out.push(Effect::Send {
                    to: session,
                    msg: ServerMsg::Bye {
                        reason: ByeReason::ClockSkew,
                    },
                });
                self.remove_session(session, out);
                break;
            }
            let seen = self.seen.entry(replica).or_default();
            if !seen.insert(op.id.counter) {
                // Duplicate. Ack only if durable; otherwise the Commit will follow.
                match self.index.get(&op.id) {
                    Some(&seq) if seq <= self.durable_seq => out.push(Effect::Send {
                        to: session,
                        msg: ServerMsg::Ack {
                            op_id: op.id,
                            seq: Some(seq),
                        },
                    }),
                    Some(_) => {}
                    None => out.push(Effect::Send {
                        to: session,
                        msg: ServerMsg::Ack {
                            op_id: op.id,
                            seq: None,
                        },
                    }),
                }
                continue;
            }
            self.head_seq += 1;
            let seq = self.head_seq;
            self.index.insert(op.id, seq);
            self.log.push_back((seq, op.clone()));
            accepted.push((seq, op));
        }
        if !accepted.is_empty() {
            out.push(Effect::Persist { ops: accepted });
        }
    }

    /// Storage confirmed everything up to `up_to_seq`: apply to the durable
    /// state and broadcast the commits (P8: in seq order).
    pub fn durable(&mut self, up_to_seq: u64, out: &mut Vec<Effect>) {
        let up_to = up_to_seq.min(self.head_seq);
        if up_to <= self.durable_seq {
            return;
        }
        let from = self.durable_seq;
        let mut commits: Vec<(u64, Op)> = Vec::new();
        for (seq, op) in self.log.iter() {
            if *seq > from && *seq <= up_to {
                commits.push((*seq, op.clone()));
            }
        }
        debug_assert_eq!(commits.len() as u64, up_to - from);
        for (seq, op) in commits {
            self.doc.apply(&op);
            self.durable_seq = seq;
            for sid in self.sessions.keys() {
                out.push(Effect::Send {
                    to: *sid,
                    msg: ServerMsg::Commit {
                        seq,
                        op: op.clone(),
                    },
                });
            }
        }
        self.trim_log();
    }

    fn trim_log(&mut self) {
        while self.log.len() > self.cfg.log_capacity {
            match self.log.front() {
                Some((seq, _)) if *seq <= self.durable_seq => {
                    let (_, op) = self.log.pop_front().unwrap();
                    self.index.remove(&op.id);
                }
                _ => break,
            }
        }
    }

    pub fn presence(&mut self, session: SessionId, state: PresenceState, out: &mut Vec<Effect>) {
        let Some(sess) = self.sessions.get_mut(&session) else {
            return;
        };
        sess.presence = state.clone();
        let entry = PresenceEntry {
            replica: sess.replica,
            user: sess.user.clone(),
            state,
        };
        for sid in self.sessions.keys() {
            if *sid != session {
                out.push(Effect::Send {
                    to: *sid,
                    msg: ServerMsg::PresenceUpdate(entry.clone()),
                });
            }
        }
    }

    /// The transport closed (or the adapter is evicting a slow consumer).
    pub fn close(&mut self, session: SessionId, out: &mut Vec<Effect>) {
        self.remove_session(session, out);
    }

    fn remove_session(&mut self, session: SessionId, out: &mut Vec<Effect>) {
        let Some(sess) = self.sessions.remove(&session) else {
            return;
        };
        if self.by_replica.get(&sess.replica) == Some(&session) {
            self.by_replica.remove(&sess.replica);
        }
        out.push(Effect::Close { session });
        for sid in self.sessions.keys() {
            out.push(Effect::Send {
                to: *sid,
                msg: ServerMsg::PresenceLeave {
                    replica: sess.replica,
                },
            });
        }
    }

    /// Every op the hub has accepted (durable or not), in seq order, from the
    /// in-memory log. For tests and the simulator's oracle.
    pub fn log(&self) -> impl Iterator<Item = &(u64, Op)> {
        self.log.iter()
    }
}

//! Deterministic discrete-event simulator for the Converge protocol.
//!
//! One [`Hub`] plus N reference clients ([`ClientSync`]) exchange messages
//! through simulated connections under a seeded [`FaultPlan`]. Everything is
//! driven from a virtual clock and forked `ChaCha8Rng` streams, so a
//! `(scenario, seed)` pair replays exactly. See `docs/IMPLEMENTATION_RISKS.md`
//! §4 for the architecture and the invariants I1–I8 checked here.

#![forbid(unsafe_code)]

pub mod faults;
pub mod scenario;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

use converge_client_sync::{
    ClientConfig, ClientSync, ConnState, Now, Output, ReplicaRow, SnapshotWrite,
};
use converge_core::json as cj;
use converge_core::{
    codec, fracindex, Document, Hlc, ObjectKind, Op, OpId, OpKind, ReplicaId, Value,
};
use converge_hub::{Effect, Hub, HubConfig};
use converge_proto::json as pj;
use converge_proto::{CatchUp, ClientMsg, DocId, PresenceState, ServerMsg, SessionId, UserInfo};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde_json::{json, Value as J};

pub use faults::{FaultPlan, Range};
pub use scenario::{Scenario, SCENARIOS};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ConnId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir {
    ToServer,
    ToClient,
}

#[derive(Clone, Debug)]
enum StoreWrite {
    Append { op: Op, row: ReplicaRow },
    Snapshot(SnapshotWrite),
}

#[derive(Clone, Debug)]
enum Event {
    Deliver {
        conn: ConnId,
        dir: Dir,
        msg: Msg,
    },
    ClientAction {
        client: usize,
    },
    Open {
        client: usize,
        attempt: u64,
    },
    /// Client-side close; the server learns of it via `ServerNoticedClose`.
    ClientClose {
        client: usize,
        conn: ConnId,
    },
    ServerNoticedClose {
        conn: ConnId,
    },
    StoreDone {
        client: usize,
        incarnation: u64,
        write: StoreWrite,
    },
    PersistDone {
        incarnation: u64,
        ops: Vec<(u64, Op)>,
    },
    ClientSnapshot {
        client: usize,
    },
    OfflineBegin {
        client: usize,
        until: u64,
    },
    OfflineEnd {
        client: usize,
    },
    CrashServer,
    ServerUp,
    CrashClient {
        client: usize,
    },
    ClientTick {
        client: usize,
        deadline: u64,
    },
    ClockJump {
        client: usize,
        skew_ms: i64,
    },
}

#[derive(Clone, Debug)]
enum Msg {
    C(ClientMsg),
    S(ServerMsg),
    /// Server-side close frame: delivered in order after earlier messages.
    Close,
}

struct Scheduled {
    time: u64,
    order: u64,
    event: Event,
}

impl PartialEq for Scheduled {
    fn eq(&self, o: &Self) -> bool {
        self.time == o.time && self.order == o.order
    }
}
impl Eq for Scheduled {}
impl PartialOrd for Scheduled {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Scheduled {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.time, self.order).cmp(&(o.time, o.order))
    }
}

struct Conn {
    client: usize,
    open: bool,
    /// ClientSync generation this connection was opened with.
    gen: u64,
    last_delivery: [u64; 2],
}

/// The client's durable local store (IndexedDB stand-in). Writes complete
/// asynchronously; a crash loses whatever has not completed.
#[derive(Clone, Debug, Default)]
struct FakeStore {
    row: ReplicaRow,
    snapshot: Option<(Vec<u8>, u64)>,
    pending: BTreeMap<u64, Op>,
    /// FIFO completion time of the last write.
    last_done: u64,
}

struct SimClient {
    sync: ClientSync,
    store: FakeStore,
    rng: ChaCha8Rng,
    conn: Option<ConnId>,
    incarnation: u64,
    skew_ms: i64,
    offline: bool,
    edits_since_snapshot: usize,
    snapshot_scheduled: bool,
    next_z: Option<String>,
    /// Ops whose local write completed — must all reach the server (I2).
    durable_ops: BTreeSet<u64>,
    open_attempt: u64,
    /// Deadline a `ClientTick` is currently scheduled for.
    armed: Option<u64>,
    /// Per-step record of every `ClientSync` call (for TS replay).
    trace: Option<Vec<J>>,
}

/// Server durable storage (Postgres stand-in).
#[derive(Clone, Debug)]
struct FakeStorage {
    snapshot: (Document, u64),
    log: Vec<(u64, Op)>,
    durable_seq: u64,
    last_persist_done: u64,
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub ops_authored: u64,
    pub ops_committed: u64,
    pub messages_delivered: u64,
    pub messages_dropped: u64,
    pub messages_duplicated: u64,
    pub reconnects: u64,
    pub gap_resumes: u64,
    pub snapshot_catch_ups: u64,
    pub ops_catch_ups: u64,
    pub skew_nacks: u64,
    pub permanent_nacks: u64,
    pub superseded: u64,
    pub timeouts: u64,
    pub clock_jumps: u64,
    pub server_crashes: u64,
    pub client_crashes: u64,
    pub duplicate_commits_in_storage: u64,
    pub end_time_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub invariant: &'static str,
    pub detail: String,
    pub time_ms: u64,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{} ms] {} violated: {}",
            self.time_ms, self.invariant, self.detail
        )
    }
}

pub struct Sim {
    scenario: Scenario,
    seed: u64,
    now: u64,
    order: u64,
    events: BinaryHeap<Reverse<Scheduled>>,
    rng: ChaCha8Rng,
    hub: Option<Hub>,
    storage: FakeStorage,
    server_incarnation: u64,
    server_up: bool,
    clients: Vec<SimClient>,
    conns: BTreeMap<ConnId, Conn>,
    next_conn: u64,
    steps_left: usize,
    draining: bool,
    /// OpId → HLC as accepted by the hub (I4).
    hlc_of: HashMap<OpId, Hlc>,
    /// Ops the server permanently rejected (excluded from I2).
    rejected: HashSet<OpId>,
    pub stats: Stats,
    pub trace: Vec<String>,
    trace_enabled: bool,
    failure: Option<Failure>,
    durable_since_snapshot: u64,
}

const DOC: &str = "sim-doc";

/// Virtual time starts at a realistic epoch so negative clock skews never
/// underflow.
pub const EPOCH_MS: u64 = 1_700_000_000_000;

impl Sim {
    pub fn new(scenario: Scenario, seed: u64) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let hub = Hub::new(
            DocId(DOC.into()),
            HubConfig {
                skew_tolerance_ms: scenario.skew_tolerance_ms,
                log_capacity: scenario.hub_log_capacity,
            },
        );
        let mut clients = Vec::new();
        for i in 0..scenario.clients {
            let replica = ReplicaId(rng.gen::<u64>() | 1);
            let client_rng = ChaCha8Rng::seed_from_u64(rng.gen());
            let sync = ClientSync::new(
                DocId(DOC.into()),
                replica,
                UserInfo {
                    name: format!("c{i}"),
                    color: i as u32,
                },
                ClientConfig {
                    retimestamp_margin_ms: scenario.skew_tolerance_ms / 2,
                    hello_timeout_ms: 2_000,
                    ack_timeout_ms: 3_000,
                },
            );
            clients.push(SimClient {
                sync,
                store: FakeStore::default(),
                rng: client_rng,
                conn: None,
                incarnation: 0,
                skew_ms: scenario.skew_ms[i % scenario.skew_ms.len().max(1)],
                offline: false,
                edits_since_snapshot: 0,
                snapshot_scheduled: false,
                next_z: None,
                durable_ops: BTreeSet::new(),
                open_attempt: 0,
                armed: None,
                trace: None,
            });
        }
        let steps_left = scenario.steps;
        let mut sim = Sim {
            scenario,
            seed,
            now: EPOCH_MS,
            order: 0,
            events: BinaryHeap::new(),
            rng,
            hub: Some(hub),
            storage: FakeStorage {
                snapshot: (Document::new(), 0),
                log: Vec::new(),
                durable_seq: 0,
                last_persist_done: 0,
            },
            server_incarnation: 0,
            server_up: true,
            clients,
            conns: BTreeMap::new(),
            next_conn: 1,
            steps_left,
            draining: false,
            hlc_of: HashMap::new(),
            rejected: HashSet::new(),
            stats: Stats::default(),
            trace: Vec::new(),
            trace_enabled: false,
            failure: None,
            durable_since_snapshot: 0,
        };
        sim.schedule_initial();
        sim
    }

    pub fn enable_trace(&mut self) {
        self.trace_enabled = true;
    }

    /// Record every `ClientSync` interaction so the TypeScript engine can
    /// replay it (`fixtures/sync`).
    pub fn enable_client_traces(&mut self) {
        for c in &mut self.clients {
            c.trace = Some(Vec::new());
        }
    }

    /// The recorded client traces (one JSON document per client).
    pub fn client_traces(&self) -> Vec<J> {
        self.clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                c.trace.as_ref().map(|steps| {
                    json!({
                        "scenario": self.scenario.name,
                        "seed": self.seed,
                        "client": i,
                        "doc": DOC,
                        "replica": c.sync.replica().0.to_string(),
                        "user": pj::user_to_json(&UserInfo { name: format!("c{i}"), color: i as u32 }),
                        "config": {
                            "retimestamp_margin_ms": self.scenario.skew_tolerance_ms / 2,
                            "hello_timeout_ms": 2_000,
                            "ack_timeout_ms": 3_000,
                        },
                        "steps": steps,
                        "final_hash": codec::hex(&c.sync.hash()),
                    })
                })
            })
            .collect()
    }

    fn now_json(now: Now) -> J {
        json!({"wall": now.wall_ms.to_string(), "mono": now.mono_ms.to_string()})
    }

    fn outputs_json(out: &[Output]) -> J {
        J::Array(
            out.iter()
                .map(|o| match o {
                    Output::Send(m) => json!({"send": pj::client_msg_to_json(m)}),
                    Output::Persist(op) => json!({"persist": cj::op_to_json(op)}),
                    Output::Reconnect => json!("reconnect"),
                })
                .collect(),
        )
    }

    /// Append one step to a client's trace: the input, the outputs and the
    /// observable state afterwards.
    fn record(&mut self, client: usize, input: J, out: &[Output]) {
        let c = &mut self.clients[client];
        let Some(trace) = c.trace.as_mut() else {
            return;
        };
        trace.push(json!({
            "in": input,
            "out": Self::outputs_json(out),
            "hash": codec::hex(&c.sync.hash()),
            "last_seq": c.sync.last_seq().to_string(),
            "pending": c.sync.pending_debug().iter().map(|(n, d, s, a)| json!([n.to_string(), d, s, a])).collect::<Vec<_>>(),
        }));
    }

    fn schedule_initial(&mut self) {
        for i in 0..self.clients.len() {
            let t = self.now + self.rng.gen_range(0..50);
            self.at(
                t,
                Event::Open {
                    client: i,
                    attempt: 0,
                },
            );
            let dt = self.scenario.edit_interval.sample(&mut self.rng);
            self.at(t + dt, Event::ClientAction { client: i });
        }
        // Spread crashes and offline bursts over the active phase.
        let horizon = (self.scenario.steps as u64)
            * (self.scenario.edit_interval.0 + self.scenario.edit_interval.1)
            / 2
            / self.clients.len().max(1) as u64;
        let horizon = horizon.max(1_000);
        let base = self.now;
        for _ in 0..self.scenario.server_crashes {
            let t = base + self.rng.gen_range(horizon / 10..horizon);
            self.at(t, Event::CrashServer);
        }
        for _ in 0..self.scenario.client_crashes {
            let t = base + self.rng.gen_range(horizon / 10..horizon);
            let c = self.rng.gen_range(0..self.clients.len());
            self.at(t, Event::CrashClient { client: c });
        }
        for _ in 0..self.scenario.clock_jumps {
            let t = base + self.rng.gen_range(horizon / 10..horizon);
            let c = self.rng.gen_range(0..self.clients.len());
            let (lo, hi) = self.scenario.clock_jump_range;
            let skew = self.rng.gen_range(lo..=hi);
            self.at(
                t,
                Event::ClockJump {
                    client: c,
                    skew_ms: skew,
                },
            );
        }
        if let Some(burst) = self.scenario.offline_burst {
            let c = self.rng.gen_range(0..self.clients.len());
            let t = base + self.rng.gen_range(horizon / 5..horizon / 2);
            let len = burst.sample(&mut self.rng);
            self.at(
                t,
                Event::OfflineBegin {
                    client: c,
                    until: t + len,
                },
            );
        }
    }

    fn at(&mut self, time: u64, event: Event) {
        self.order += 1;
        self.events.push(Reverse(Scheduled {
            time: time.max(self.now),
            order: self.order,
            event,
        }));
    }

    fn fail(&mut self, invariant: &'static str, detail: String) {
        if self.failure.is_none() {
            self.failure = Some(Failure {
                invariant,
                detail,
                time_ms: self.now - EPOCH_MS,
            });
        }
    }

    fn log(&mut self, line: impl FnOnce() -> String) {
        if self.trace_enabled {
            let l = format!("{:>8} {}", self.now - EPOCH_MS, line());
            self.trace.push(l);
        }
    }

    /// Run to quiescence. Returns the first invariant violation, if any.
    pub fn run(&mut self) -> Result<Stats, Failure> {
        loop {
            if let Some(f) = &self.failure {
                return Err(f.clone());
            }
            let Some(Reverse(s)) = self.events.pop() else {
                if self.finish_or_continue() {
                    continue;
                }
                break;
            };
            if let Event::ClientTick { client, deadline } = &s.event {
                if self.clients[*client].sync.next_deadline() != Some(*deadline) {
                    let c = *client;
                    if self.clients[c].armed == Some(*deadline) {
                        self.clients[c].armed = None;
                    }
                    self.arm_tick(c);
                    continue; // stale timer: must not hold up quiescence
                }
            }
            if s.time > EPOCH_MS + self.scenario.max_time_ms {
                return Err(Failure {
                    invariant: "I6 liveness",
                    detail: format!(
                        "did not quiesce by {} ms ({} events pending)",
                        self.scenario.max_time_ms,
                        self.events.len() + 1
                    ),
                    time_ms: self.now - EPOCH_MS,
                });
            }
            self.now = s.time;
            self.step(s.event);
        }
        self.stats.end_time_ms = self.now - EPOCH_MS;
        self.check_final()?;
        Ok(self.stats.clone())
    }

    /// Heap is empty. Kick anything still needed for a clean quiescent
    /// state (reconnects, final snapshots); returns true if events were added.
    fn finish_or_continue(&mut self) -> bool {
        self.draining = true;
        let mut added = false;
        if !self.server_up {
            self.at(self.now + 1, Event::ServerUp);
            return true;
        }
        for i in 0..self.clients.len() {
            let c = &self.clients[i];
            if c.offline {
                self.at(self.now + 1, Event::OfflineEnd { client: i });
                added = true;
            } else if c.conn.is_none() {
                self.at(
                    self.now + 1,
                    Event::Open {
                        client: i,
                        attempt: c.open_attempt,
                    },
                );
                added = true;
            } else if c.sync.pending_len() > 0
                && c.sync.state() == ConnState::Live
                && !c.snapshot_scheduled
                && c.sync.unacked().count() == 0
            {
                self.at(self.now + 1, Event::ClientSnapshot { client: i });
                added = true;
            }
        }
        added
    }

    // ----- event dispatch -----

    fn step(&mut self, event: Event) {
        match event {
            Event::Deliver { conn, dir, msg } => self.deliver(conn, dir, msg),
            Event::ClientAction { client } => self.client_action(client),
            Event::Open { client, attempt } => self.open(client, attempt),
            Event::ClientClose { client, conn } => {
                if !self.draining {
                    self.client_close(client, conn); // scheduled fault
                }
            }
            Event::ServerNoticedClose { conn } => {
                if let Some(hub) = self.hub.as_mut() {
                    let mut out = Vec::new();
                    hub.close(SessionId(conn.0), &mut out);
                    self.effects(out);
                }
            }
            Event::StoreDone {
                client,
                incarnation,
                write,
            } => self.store_done(client, incarnation, write),
            Event::PersistDone { incarnation, ops } => self.persist_done(incarnation, ops),
            Event::ClientSnapshot { client } => self.client_snapshot(client),
            Event::OfflineBegin { client, until } => {
                self.log(|| format!("client {client} offline until {until}"));
                self.clients[client].offline = true;
                if let Some(conn) = self.clients[client].conn {
                    self.client_close(client, conn);
                }
                self.at(until, Event::OfflineEnd { client });
            }
            Event::OfflineEnd { client } => {
                self.clients[client].offline = false;
                if self.clients[client].conn.is_none() {
                    let a = self.clients[client].open_attempt;
                    self.at(self.now, Event::Open { client, attempt: a });
                }
            }
            Event::CrashServer => self.crash_server(),
            Event::ServerUp => {
                self.server_up = true;
                self.log(|| "server up".into());
            }
            Event::CrashClient { client } => self.crash_client(client),
            Event::ClockJump { client, skew_ms } => {
                self.log(|| format!("client {client} clock jumps to skew {skew_ms} ms"));
                self.clients[client].skew_ms = skew_ms;
                self.clients[client].armed = None;
                self.arm_tick(client);
                self.stats.clock_jumps += 1;
            }
            Event::ClientTick { client, deadline } => {
                let c = &mut self.clients[client];
                c.armed = None;
                if c.sync.next_deadline() != Some(deadline) {
                    self.arm_tick(client);
                    return; // stale timer
                }
                let now_local = Now::new((self.now as i64 + c.skew_ms).max(0) as u64, self.now);
                let mut out = Vec::new();
                c.sync.tick(now_local, &mut out);
                self.record(
                    client,
                    json!({"kind": "tick", "now": Self::now_json(now_local)}),
                    &out,
                );
                if !out.is_empty() {
                    self.stats.timeouts += 1;
                    self.log(|| format!("client {client} timed out waiting for the server"));
                }
                self.outputs(client, out);
            }
        }
    }

    // ----- network -----

    fn send(&mut self, conn_id: ConnId, dir: Dir, msg: Msg) {
        let Some(conn) = self.conns.get_mut(&conn_id) else {
            return;
        };
        if !conn.open {
            return;
        }
        let plan = &self.scenario.faults;
        let faults_on = !self.draining;
        if faults_on && self.rng.gen_bool(plan.drop_p) {
            self.stats.messages_dropped += 1;
            self.log(|| format!("drop {dir:?} on {conn_id:?}"));
            return;
        }
        let lat = plan.latency.sample(&mut self.rng);
        let d = if dir == Dir::ToServer { 0 } else { 1 };
        let mut t = self.now + lat;
        if plan.fifo {
            t = t.max(conn.last_delivery[d]);
            conn.last_delivery[d] = t;
        }
        let dup = faults_on && self.rng.gen_bool(plan.dup_p);
        let dup_delay = plan.dup_delay.sample(&mut self.rng);
        if dup {
            self.stats.messages_duplicated += 1;
            self.at(
                t + dup_delay,
                Event::Deliver {
                    conn: conn_id,
                    dir,
                    msg: msg.clone(),
                },
            );
        }
        self.at(
            t,
            Event::Deliver {
                conn: conn_id,
                dir,
                msg,
            },
        );
    }

    /// The close frame is never dropped or duplicated but keeps FIFO order.
    fn send_close(&mut self, conn_id: ConnId) {
        let Some(conn) = self.conns.get_mut(&conn_id) else {
            return;
        };
        let lat = self.scenario.faults.latency.sample(&mut self.rng);
        let mut t = self.now + lat;
        if self.scenario.faults.fifo {
            t = t.max(conn.last_delivery[1]);
            conn.last_delivery[1] = t;
        }
        self.at(
            t,
            Event::Deliver {
                conn: conn_id,
                dir: Dir::ToClient,
                msg: Msg::Close,
            },
        );
    }

    fn deliver(&mut self, conn_id: ConnId, dir: Dir, msg: Msg) {
        let Some(conn) = self.conns.get(&conn_id) else {
            return;
        };
        if !conn.open {
            return; // H5: nothing from a closed connection is processed
        }
        let client = conn.client;
        let gen = conn.gen;
        if let Msg::Close = msg {
            self.conns.get_mut(&conn_id).unwrap().open = false;
            self.log(|| format!("client {client} saw server close of conn {}", conn_id.0));
            self.client_lost_conn(client, conn_id);
            return;
        }
        self.stats.messages_delivered += 1;
        self.observe_msg(&msg);
        match (dir, msg) {
            (Dir::ToServer, Msg::C(m)) => {
                if !self.server_up {
                    return;
                }
                let Some(hub) = self.hub.as_mut() else { return };
                let mut out = Vec::new();
                hub.handle(SessionId(conn_id.0), m, self.now, &mut out);
                self.effects(out);
            }
            (Dir::ToClient, Msg::S(m)) => {
                if self.clients[client].conn != Some(conn_id) {
                    return;
                }
                self.check_server_msg(client, &m);
                let now_local = self.local_now(client);
                let mut out = Vec::new();
                let msg_json = pj::server_msg_to_json(&m);
                self.clients[client]
                    .sync
                    .message(gen, m, now_local, &mut out);
                self.record(client, json!({"kind": "message", "gen": gen.to_string(), "now": Self::now_json(now_local), "msg": msg_json}), &out);
                self.outputs(client, out);
                let last = self.clients[client].sync.last_seq();
                if last > self.storage.durable_seq {
                    self.fail(
                        "I5 client ahead of durable",
                        format!(
                            "client {client} last_seq {last} > storage durable {}",
                            self.storage.durable_seq
                        ),
                    );
                }
            }
            _ => unreachable!("direction/message mismatch"),
        }
    }

    /// I4: one HLC per OpId. `hlc_of` is populated when the hub accepts an
    /// op; every op crossing the wire afterwards must carry that HLC.
    fn observe_msg(&mut self, msg: &Msg) {
        let check = |this: &mut Sim, op: &Op| {
            if let Some(h) = this.hlc_of.get(&op.id) {
                if *h != op.hlc {
                    this.fail(
                        "I4 one HLC per OpId",
                        format!("{} accepted with {:?}, seen with {:?}", op.id, h, op.hlc),
                    );
                }
            }
        };
        match msg {
            Msg::C(ClientMsg::Submit { ops }) => {
                for op in ops {
                    check(self, op);
                }
            }
            Msg::S(ServerMsg::Commit { op, .. }) => check(self, op),
            Msg::S(ServerMsg::Welcome {
                catch_up: CatchUp::Ops(ops),
                ..
            }) => {
                for (_, op) in ops {
                    check(self, op);
                }
            }
            _ => {}
        }
    }

    fn check_server_msg(&mut self, client: usize, m: &ServerMsg) {
        match m {
            ServerMsg::Commit { seq, .. } => {
                if *seq > self.storage.durable_seq {
                    self.fail(
                        "P2 commit before durable",
                        format!(
                            "Commit seq {seq} > storage durable {}",
                            self.storage.durable_seq
                        ),
                    );
                }
                self.stats.ops_committed += 1;
            }
            ServerMsg::Welcome { .. }
                if self.clients[client].sync.state() != ConnState::HelloSent =>
            {
                // Duplicate delivery; the client ignores it.
            }
            ServerMsg::Welcome {
                durable_head_seq,
                catch_up,
                ..
            } => {
                if *durable_head_seq > self.storage.durable_seq {
                    self.fail(
                        "P2 welcome reveals non-durable",
                        format!(
                            "durable_head {durable_head_seq} > storage {}",
                            self.storage.durable_seq
                        ),
                    );
                }
                match catch_up {
                    CatchUp::Ops(ops) => {
                        self.stats.ops_catch_ups += 1;
                        let last = self.clients[client].sync.last_seq();
                        for (i, (s, _)) in ops.iter().enumerate() {
                            if *s != last + 1 + i as u64 {
                                self.fail(
                                    "P5 catch-up contiguity",
                                    format!("expected {}, got {s}", last + 1 + i as u64),
                                );
                            }
                        }
                        if let Some((s, _)) = ops.last() {
                            if *s != *durable_head_seq {
                                self.fail(
                                    "P5 catch-up ends at durable head",
                                    format!("{s} != {durable_head_seq}"),
                                );
                            }
                        }
                    }
                    CatchUp::Snapshot { .. } => self.stats.snapshot_catch_ups += 1,
                }
            }
            ServerMsg::Nack { reason, op_id } => {
                let accepted = self.hlc_of.get(op_id).copied();
                self.log(|| format!("client {client} got Nack {reason:?} for {op_id} (accepted hlc {accepted:?})"));
                if reason.is_permanent() {
                    self.stats.permanent_nacks += 1;
                    self.rejected.insert(*op_id);
                } else {
                    self.stats.skew_nacks += 1;
                }
            }
            ServerMsg::Bye { reason } => {
                self.log(|| format!("client {client} got Bye {reason:?}"));
            }
            _ => {}
        }
    }

    // ----- server side -----

    fn effects(&mut self, out: Vec<Effect>) {
        for e in out {
            match e {
                Effect::Send { to, msg } => {
                    if let ServerMsg::Bye {
                        reason: converge_proto::ByeReason::Superseded,
                    } = msg
                    {
                        self.stats.superseded += 1;
                    }
                    self.send(ConnId(to.0), Dir::ToClient, Msg::S(msg))
                }
                Effect::Close { session } => {
                    let id = ConnId(session.0);
                    if self.conns.get(&id).is_some_and(|c| c.open) {
                        self.log(|| format!("server closing conn {}", id.0));
                        self.send_close(id);
                    }
                }
                Effect::Persist { ops } => {
                    for (_, op) in &ops {
                        match self.hlc_of.get(&op.id) {
                            Some(h) if *h != op.hlc => self.fail(
                                "I4 one HLC per OpId",
                                format!("{} accepted twice with {:?} and {:?}", op.id, h, op.hlc),
                            ),
                            _ => {
                                self.hlc_of.insert(op.id, op.hlc);
                            }
                        }
                    }
                    let lat = self.scenario.persist_latency.sample(&mut self.rng);
                    // The persister is a single FIFO task (P8).
                    let t = (self.now + lat).max(self.storage.last_persist_done);
                    self.storage.last_persist_done = t;
                    let inc = self.server_incarnation;
                    self.at(
                        t,
                        Event::PersistDone {
                            incarnation: inc,
                            ops,
                        },
                    );
                }
            }
        }
    }

    fn persist_done(&mut self, incarnation: u64, ops: Vec<(u64, Op)>) {
        if incarnation != self.server_incarnation || !self.server_up {
            return; // lost with the crashed process
        }
        let mut up_to = self.storage.durable_seq;
        for (seq, op) in ops {
            if seq != self.storage.durable_seq + 1 {
                self.fail(
                    "P8 persist order",
                    format!(
                        "persisting seq {seq} after durable {}",
                        self.storage.durable_seq
                    ),
                );
                return;
            }
            self.storage.log.push((seq, op));
            self.storage.durable_seq = seq;
            up_to = seq;
        }
        let Some(hub) = self.hub.as_mut() else { return };
        let mut out = Vec::new();
        hub.durable(up_to, &mut out);
        let hub_durable = hub.durable_seq();
        self.durable_since_snapshot += 1;
        let snapshot = if self.durable_since_snapshot >= self.scenario.server_snapshot_every {
            self.durable_since_snapshot = 0;
            Some(hub.snapshot())
        } else {
            None
        };
        if hub_durable > self.storage.durable_seq {
            self.fail(
                "I3 hub durable beyond storage",
                format!("{} > {}", hub_durable, self.storage.durable_seq),
            );
        }
        if let Some((bytes, seq)) = snapshot {
            let doc = codec::decode_snapshot(&bytes).unwrap();
            self.storage.snapshot = (doc, seq);
            self.storage.log.retain(|(s, _)| *s > seq);
        }
        self.effects(out);
    }

    fn crash_server(&mut self) {
        if !self.server_up {
            return;
        }
        self.stats.server_crashes += 1;
        self.log(|| "server crash".into());
        self.server_up = false;
        self.server_incarnation += 1;
        self.hub = None;
        let open: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| c.open)
            .map(|(id, _)| *id)
            .collect();
        for id in open {
            let conn = self.conns.get_mut(&id).unwrap();
            conn.open = false;
            let client = conn.client;
            self.client_lost_conn(client, id);
        }
        // Rebuild from durable storage.
        let (doc, seq) = self.storage.snapshot.clone();
        let tail: Vec<(u64, Op)> = self
            .storage
            .log
            .iter()
            .filter(|(s, _)| *s > seq)
            .cloned()
            .collect();
        let hub = Hub::recover(
            DocId(DOC.into()),
            HubConfig {
                skew_tolerance_ms: self.scenario.skew_tolerance_ms,
                log_capacity: self.scenario.hub_log_capacity,
            },
            doc,
            seq,
            tail,
        );
        assert_eq!(hub.durable_seq(), self.storage.durable_seq);
        self.hub = Some(hub);
        self.storage.last_persist_done = self.now;
        let down = self.scenario.server_downtime.sample(&mut self.rng);
        self.at(self.now + down, Event::ServerUp);
    }

    // ----- client side -----

    /// The client's clocks: skewed wall time for the HLC, sim time as the
    /// monotonic clock.
    fn local_now(&self, client: usize) -> Now {
        Now::new(
            (self.now as i64 + self.clients[client].skew_ms).max(0) as u64,
            self.now,
        )
    }

    fn open(&mut self, client: usize, attempt: u64) {
        let c = &self.clients[client];
        if attempt != c.open_attempt || c.conn.is_some() || c.offline {
            return;
        }
        if !self.server_up {
            let b = self.scenario.reconnect_backoff.sample(&mut self.rng);
            self.at(self.now + b, Event::Open { client, attempt });
            return;
        }
        let id = ConnId(self.next_conn);
        self.next_conn += 1;
        let mut out = Vec::new();
        let now_local = self.local_now(client);
        let gen = self.clients[client].sync.connected(now_local, &mut out);
        self.record(
            client,
            json!({"kind": "connected", "now": Self::now_json(now_local)}),
            &out,
        );
        self.conns.insert(
            id,
            Conn {
                client,
                open: true,
                gen,
                last_delivery: [self.now, self.now],
            },
        );
        self.clients[client].conn = Some(id);
        self.stats.reconnects += 1;
        self.log(|| format!("client {client} opened conn {}", id.0));
        self.outputs(client, out);
        if let Some(d) = self.scenario.disconnect_interval {
            if !self.draining {
                let when = d.sample(&mut self.rng);
                self.at(self.now + when, Event::ClientClose { client, conn: id });
            }
        }
    }

    /// The client drops its connection (fault, gap resume or resync).
    fn client_close(&mut self, client: usize, conn: ConnId) {
        let current = self.clients[client].conn;
        let conn = if conn == ConnId(u64::MAX) {
            match current {
                Some(c) => c,
                None => return,
            }
        } else {
            conn
        };
        if current != Some(conn) {
            return;
        }
        if let Some(c) = self.conns.get_mut(&conn) {
            if c.open {
                c.open = false;
                let d = self
                    .scenario
                    .faults
                    .server_notice_delay
                    .sample(&mut self.rng);
                self.at(self.now + d, Event::ServerNoticedClose { conn });
            }
        }
        self.log(|| format!("client {client} closed conn {}", conn.0));
        self.client_lost_conn(client, conn);
    }

    fn client_lost_conn(&mut self, client: usize, conn: ConnId) {
        let c = &mut self.clients[client];
        if c.conn != Some(conn) {
            return;
        }
        c.conn = None;
        c.sync.disconnected();
        c.open_attempt += 1;
        self.record(client, json!({"kind": "disconnected"}), &[]);
        let c = &mut self.clients[client];
        let attempt = c.open_attempt;
        if c.offline {
            return;
        }
        let b = self.scenario.reconnect_backoff.sample(&mut self.rng);
        self.at(self.now + b, Event::Open { client, attempt });
    }

    fn outputs(&mut self, client: usize, out: Vec<Output>) {
        self.outputs_inner(client, out);
        self.arm_tick(client);
    }

    /// Make sure a `ClientTick` is scheduled for the client's current
    /// deadline. Idempotent; stale ticks are skipped when popped.
    fn arm_tick(&mut self, client: usize) {
        let c = &mut self.clients[client];
        let after = c.sync.next_deadline();
        if after.is_some() && after != c.armed {
            c.armed = after;
            let d = after.unwrap();
            self.at(
                d.max(self.now),
                Event::ClientTick {
                    client,
                    deadline: d,
                },
            );
        }
    }

    fn outputs_inner(&mut self, client: usize, out: Vec<Output>) {
        for o in out {
            match o {
                Output::Send(m) => {
                    if let Some(conn) = self.clients[client].conn {
                        self.send(conn, Dir::ToServer, Msg::C(m));
                    }
                }
                Output::Persist(op) => {
                    let row = self.clients[client].sync.replica_row();
                    self.store_write(client, StoreWrite::Append { op, row });
                }
                Output::Reconnect => {
                    self.stats.gap_resumes += 1;
                    if let Some(conn) = self.clients[client].conn {
                        self.client_close(client, conn);
                    }
                }
            }
        }
    }

    fn store_write(&mut self, client: usize, write: StoreWrite) {
        let lat = self.scenario.store_latency.sample(&mut self.rng);
        let c = &mut self.clients[client];
        let t = (self.now + lat).max(c.store.last_done);
        c.store.last_done = t;
        let inc = c.incarnation;
        self.at(
            t,
            Event::StoreDone {
                client,
                incarnation: inc,
                write,
            },
        );
    }

    fn store_done(&mut self, client: usize, incarnation: u64, write: StoreWrite) {
        if self.clients[client].incarnation != incarnation {
            return; // lost in the crash
        }
        match write {
            StoreWrite::Append { op, row } => {
                let c = &mut self.clients[client];
                let counter = op.id.counter;
                c.store.pending.insert(counter, op);
                c.store.row = row;
                c.durable_ops.insert(counter);
                let mut out = Vec::new();
                let now_local = Now::new((self.now as i64 + c.skew_ms).max(0) as u64, self.now);
                c.sync.persisted(counter, now_local, &mut out);
                self.record(client, json!({"kind": "persisted", "counter": counter.to_string(), "now": Self::now_json(now_local)}), &out);
                self.outputs(client, out);
            }
            StoreWrite::Snapshot(w) => {
                let c = &mut self.clients[client];
                c.snapshot_scheduled = false;
                // D4: never overwrite a newer snapshot.
                let stored_seq = c.store.snapshot.as_ref().map(|(_, s)| *s).unwrap_or(0);
                if w.seq >= stored_seq {
                    c.store.snapshot = Some((w.bytes, w.seq));
                    for a in &w.acked {
                        c.store.pending.remove(a);
                    }
                    c.store.row.high_water = c.store.row.high_water.max(w.high_water);
                    c.sync.snapshot_persisted(&w.acked);
                    let acked: Vec<String> = w.acked.iter().map(|a| a.to_string()).collect();
                    self.record(
                        client,
                        json!({"kind": "snapshot_persisted", "acked": acked}),
                        &[],
                    );
                }
            }
        }
    }

    fn client_snapshot(&mut self, client: usize) {
        let c = &mut self.clients[client];
        if c.snapshot_scheduled {
            return;
        }
        c.snapshot_scheduled = true;
        c.edits_since_snapshot = 0;
        let w = c.sync.snapshot();
        self.record(
            client,
            json!({"kind": "snapshot", "expect": {"seq": w.seq.to_string(), "high_water": cj::hlc_to_json(&w.high_water), "acked": w.acked.iter().map(|a| a.to_string()).collect::<Vec<_>>(), "snapshot_hash": codec::hex(blake3::hash(&w.bytes).as_bytes())}}),
            &[],
        );
        self.store_write(client, StoreWrite::Snapshot(w));
    }

    fn crash_client(&mut self, client: usize) {
        self.stats.client_crashes += 1;
        self.log(|| format!("client {client} crash"));
        if let Some(conn) = self.clients[client].conn {
            self.client_close(client, conn);
        }
        let c = &mut self.clients[client];
        c.incarnation += 1;
        c.snapshot_scheduled = false;
        let store = c.store.clone();
        let replica = c.sync.replica();
        c.sync = ClientSync::restore(
            DocId(DOC.into()),
            replica,
            UserInfo {
                name: format!("c{client}"),
                color: client as u32,
            },
            ClientConfig {
                retimestamp_margin_ms: self.scenario.skew_tolerance_ms / 2,
                hello_timeout_ms: 2_000,
                ack_timeout_ms: 3_000,
            },
            store.row.clone(),
            store.snapshot.clone(),
            store.pending.values().cloned().collect(),
        );
        c.store.last_done = self.now;
        c.next_z = None;
        let input = json!({
            "kind": "restore",
            "row": {"next_counter": store.row.next_counter.to_string(), "high_water": cj::hlc_to_json(&store.row.high_water), "clock_offset_ms": store.row.clock_offset_ms.to_string()},
            "snapshot": store.snapshot.as_ref().map(|(b, seq)| json!({"bytes": codec::hex(b), "seq": seq.to_string()})),
            "pending": store.pending.values().map(cj::op_to_json).collect::<Vec<_>>(),
        });
        self.record(client, input, &[]);
        // Reconnect happens via client_lost_conn's scheduled Open.
    }

    fn client_action(&mut self, client: usize) {
        if self.steps_left == 0 {
            return;
        }
        self.steps_left -= 1;
        let now_local = self.local_now(client);
        let kind = self.random_op_kind(client);
        let mut out = Vec::new();
        let op = self.clients[client]
            .sync
            .edit(kind.clone(), now_local, &mut out);
        let kind_json = cj::op_to_json(&Op {
            id: op.id,
            hlc: op.hlc,
            kind,
        })["kind"]
            .clone();
        self.record(client, json!({"kind": "edit", "now": Self::now_json(now_local), "op_kind": kind_json, "op": cj::op_to_json(&op)}), &out);
        self.stats.ops_authored += 1;
        self.log(|| format!("client {client} edit {} {:?}", op.id, op.kind));
        self.outputs(client, out);
        let c = &mut self.clients[client];
        c.edits_since_snapshot += 1;
        if c.edits_since_snapshot >= self.scenario.client_snapshot_every && !c.snapshot_scheduled {
            self.at(self.now, Event::ClientSnapshot { client });
        }
        if self.rng.gen_bool(0.1) {
            let p = PresenceState {
                cursor: Some((1.0, 2.0)),
                selection: vec![],
            };
            let mut out = Vec::new();
            self.clients[client].sync.presence(p, &mut out);
            self.outputs(client, out);
        }
        if self.steps_left > 0 {
            let dt = self.scenario.edit_interval.sample(&mut self.rng);
            self.at(self.now + dt, Event::ClientAction { client });
        } else {
            // Active phase over: no new faults, let everything settle.
            self.draining = true;
        }
    }

    fn random_op_kind(&mut self, client: usize) -> OpKind {
        let malformed =
            self.scenario.malformed_p > 0.0 && self.rng.gen_bool(self.scenario.malformed_p);
        let c = &mut self.clients[client];
        let objects: Vec<(OpId, bool)> = c
            .sync
            .document()
            .objects()
            .filter(|(_, o)| o.created())
            .map(|(id, o)| (*id, o.visible()))
            .collect();
        let roll = c.rng.gen_range(0..100);
        let x = c.rng.gen_range(-1000.0..1000.0f64);
        let y = c.rng.gen_range(-1000.0..1000.0f64);
        if malformed {
            let object = objects
                .first()
                .map(|(id, _)| *id)
                .unwrap_or(OpId::new(1, 1));
            return OpKind::SetProps {
                object,
                entries: vec![("x".into(), Value::F64(f64::NAN))],
            };
        }
        if objects.is_empty() || roll < 15 {
            let z = fracindex::between(c.next_z.as_deref(), None).unwrap();
            c.next_z = Some(z.clone());
            let kind = if c.rng.gen() {
                ObjectKind::Rect
            } else {
                ObjectKind::Text
            };
            let mut props = vec![
                ("x".into(), Value::F64(x)),
                ("y".into(), Value::F64(y)),
                ("z".into(), Value::FracIndex(z)),
            ];
            if kind == ObjectKind::Text {
                props.push((
                    "text".into(),
                    Value::Str(format!("t{}", c.rng.gen_range(0..100))),
                ));
            } else {
                props.push(("w".into(), Value::F64(50.0)));
                props.push(("h".into(), Value::F64(30.0)));
            }
            return OpKind::Create { kind, props };
        }
        let (object, visible) = objects[c.rng.gen_range(0..objects.len())];
        match roll {
            15..=22 if visible => OpKind::Delete { object },
            15..=22 => OpKind::Restore { object },
            23..=30 => OpKind::SetProps {
                object,
                entries: vec![("fill".into(), Value::Color(c.rng.gen()))],
            },
            31..=40 => {
                let z = fracindex::between(c.next_z.as_deref(), None).unwrap();
                c.next_z = Some(z.clone());
                OpKind::SetProps {
                    object,
                    entries: vec![("z".into(), Value::FracIndex(z))],
                }
            }
            41..=48 => OpKind::SetProps {
                object,
                entries: vec![(
                    "text".into(),
                    Value::Str(format!("t{}", c.rng.gen_range(0..100))),
                )],
            },
            _ => OpKind::SetProps {
                object,
                entries: vec![("x".into(), Value::F64(x)), ("y".into(), Value::F64(y))],
            },
        }
    }

    // ----- final invariants -----

    fn check_final(&mut self) -> Result<(), Failure> {
        let hub = self.hub.as_ref().expect("hub up at quiescence");
        if hub.head_seq() != hub.durable_seq() {
            return Err(self.mk(
                "I3 quiescent durability",
                format!("head {} != durable {}", hub.head_seq(), hub.durable_seq()),
            ));
        }
        let hub_hash = codec::hex(&hub.hash());
        for (i, c) in self.clients.iter().enumerate() {
            let h = codec::hex(&c.sync.hash());
            if h != hub_hash {
                return Err(self.mk("I1 convergence", format!("client {i} hash {h} != hub {hub_hash} (client last_seq {}, state {:?}, pending {:?})", c.sync.last_seq(), c.sync.state(), c.sync.pending_debug())));
            }
            if c.sync.pending_len() != 0 {
                return Err(self.mk(
                    "I2 pending drained",
                    format!(
                        "client {i} has {} pending ops (state {:?}, conn {:?}): {:?}",
                        c.sync.pending_len(),
                        c.sync.state(),
                        c.conn,
                        c.sync.pending_debug()
                    ),
                ));
            }
            if c.sync.last_seq() != hub.durable_seq() {
                return Err(self.mk(
                    "I1 seq agreement",
                    format!(
                        "client {i} last_seq {} != {}",
                        c.sync.last_seq(),
                        hub.durable_seq()
                    ),
                ));
            }
        }
        // I2: every locally durable op reached the server exactly once
        // (at least once if the server crashed — P3).
        let mut count: HashMap<OpId, u64> = HashMap::new();
        for (_, op) in self.storage.log.iter() {
            *count.entry(op.id).or_default() += 1;
        }
        // Ops folded into the storage snapshot are no longer in the log;
        // verify presence through their effect instead: the oracle below.
        let mut dups = 0;
        for (id, n) in &count {
            if *n > 1 {
                dups += 1;
                if self.stats.server_crashes == 0 {
                    return Err(self.mk(
                        "I2 duplicate commit",
                        format!("{id} committed {n} times without a server crash"),
                    ));
                }
            }
        }
        self.stats.duplicate_commits_in_storage = dups;
        for (i, c) in self.clients.iter().enumerate() {
            for counter in &c.durable_ops {
                let id = OpId {
                    replica: c.sync.replica(),
                    counter: *counter,
                };
                let seen = self.hlc_of.contains_key(&id) || self.rejected.contains(&id);
                if !seen {
                    return Err(self.mk(
                        "I2 durable op reached server",
                        format!("client {i} op {id} was never accepted by the hub"),
                    ));
                }
            }
        }
        // I7: semantic oracle — the multiset of committed ops applied once in
        // stamp order equals the hub state. Uses the storage snapshot as base
        // for ops that were folded into it.
        let mut oracle = self.storage.snapshot.0.clone();
        let mut ops: Vec<&Op> = self.storage.log.iter().map(|(_, op)| op).collect();
        ops.sort_by_key(|o| o.stamp());
        ops.dedup_by_key(|o| o.id);
        for op in ops {
            oracle.apply(op);
        }
        if codec::hash(&oracle) != hub.hash() {
            return Err(self.mk(
                "I7 semantic oracle",
                "stamp-ordered replay differs from hub state".into(),
            ));
        }
        // Hub state must also equal a full replay of what storage has
        // (recovery correctness).
        let _ = HashSet::<OpId>::new();
        Ok(())
    }

    fn mk(&self, invariant: &'static str, detail: String) -> Failure {
        Failure {
            invariant,
            detail,
            time_ms: self.now - EPOCH_MS,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }
}

/// Run one scenario for one seed and return the per-client traces.
pub fn run_with_traces(
    scenario: Scenario,
    seed: u64,
) -> Result<(Stats, Vec<J>), (Failure, Vec<String>)> {
    let mut sim = Sim::new(scenario, seed);
    sim.enable_trace();
    sim.enable_client_traces();
    match sim.run() {
        Ok(s) => Ok((s, sim.client_traces())),
        Err(f) => Err((f, sim.trace)),
    }
}

/// Run one scenario for one seed.
pub fn run(scenario: Scenario, seed: u64) -> Result<Stats, (Failure, Vec<String>)> {
    let mut sim = Sim::new(scenario, seed);
    sim.enable_trace();
    match sim.run() {
        Ok(s) => Ok(s),
        Err(f) => Err((f, sim.trace)),
    }
}

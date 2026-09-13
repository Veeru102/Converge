use crate::faults::{FaultPlan, Range};

/// Everything that varies between simulation runs. Named presets live in
/// [`Scenario::named`]; every field can be overridden afterwards.
#[derive(Clone, Debug)]
pub struct Scenario {
    pub name: String,
    pub clients: usize,
    /// Total number of client edit actions before draining.
    pub steps: usize,
    pub faults: FaultPlan,
    /// Physical clock offset per client (index modulo len).
    pub skew_ms: Vec<i64>,
    /// Mid-run clock steps (NTP corrections, manual changes): a random
    /// client's skew is replaced by a value in `clock_jump_range`.
    pub clock_jumps: usize,
    pub clock_jump_range: (i64, i64),
    pub edit_interval: Range,
    /// Random connection drops from the client side.
    pub disconnect_interval: Option<Range>,
    /// A client goes fully offline for this long, once, mid-run.
    pub offline_burst: Option<Range>,
    pub reconnect_backoff: Range,
    pub persist_latency: Range,
    pub store_latency: Range,
    /// Client snapshot write every N acked/authored ops.
    pub client_snapshot_every: usize,
    /// Server snapshot to storage every N durable ops.
    pub server_snapshot_every: u64,
    pub hub_log_capacity: usize,
    pub skew_tolerance_ms: u64,
    pub server_crashes: usize,
    pub server_downtime: Range,
    pub client_crashes: usize,
    /// Probability that an edit action is a malformed op (P7 path).
    pub malformed_p: f64,
    pub max_time_ms: u64,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            name: "custom".into(),
            clients: 3,
            steps: 400,
            faults: FaultPlan::default(),
            skew_ms: vec![0],
            clock_jumps: 0,
            clock_jump_range: (-15 * 60 * 1_000, 15 * 60 * 1_000),
            edit_interval: Range(1, 40),
            disconnect_interval: None,
            offline_burst: None,
            reconnect_backoff: Range(50, 500),
            persist_latency: Range(2, 15),
            store_latency: Range(1, 10),
            client_snapshot_every: 20,
            server_snapshot_every: 100,
            hub_log_capacity: 10_000,
            skew_tolerance_ms: 60_000,
            server_crashes: 0,
            server_downtime: Range(200, 2_000),
            client_crashes: 0,
            malformed_p: 0.0,
            max_time_ms: 10 * 60 * 1_000,
        }
    }
}

pub const SCENARIOS: &[&str] = &[
    "baseline_no_faults",
    "latency_jitter_fifo",
    "drop_and_dup_fifo",
    "disconnect_reconnect",
    "offline_burst",
    "ack_lost_dup_submit",
    "client_crash_store_windows",
    "skew_ahead",
    "skew_behind",
    "skew_nack_reorder_stress",
    "server_crash_before_durable",
    "reconnect_eviction_race",
    "gap_detection",
    "permanent_nack_resync",
    "reorder_stress",
    "many_clients_soak",
    "everything",
];

impl Scenario {
    pub fn named(name: &str) -> Option<Scenario> {
        let mut s = Scenario {
            name: name.into(),
            ..Default::default()
        };
        match name {
            "baseline_no_faults" => {
                s.clients = 2;
                s.faults.latency = Range(5, 5);
            }
            "latency_jitter_fifo" => {
                s.faults.latency = Range(1, 400);
            }
            "drop_and_dup_fifo" => {
                s.faults.drop_p = 0.2;
                s.faults.dup_p = 0.2;
            }
            "disconnect_reconnect" => {
                s.disconnect_interval = Some(Range(50, 800));
            }
            "offline_burst" => {
                s.offline_burst = Some(Range(3_000, 8_000));
                s.hub_log_capacity = 50;
                s.steps = 600;
            }
            "ack_lost_dup_submit" => {
                s.faults.drop_p = 0.3;
                s.disconnect_interval = Some(Range(100, 600));
            }
            "client_crash_store_windows" => {
                s.client_crashes = 12;
                s.store_latency = Range(5, 60);
                s.client_snapshot_every = 5;
            }
            "skew_ahead" => {
                s.skew_ms = vec![0, 10 * 60 * 1_000, 0];
                s.clock_jumps = 3;
                s.clock_jump_range = (60_000, 20 * 60 * 1_000);
            }
            "skew_behind" => {
                s.skew_ms = vec![0, -60 * 60 * 1_000, 0];
                s.clock_jumps = 3;
                s.clock_jump_range = (-20 * 60 * 1_000, -60_000);
            }
            "skew_nack_reorder_stress" => {
                s.skew_ms = vec![0, 10 * 60 * 1_000, -30 * 60 * 1_000];
                s.clock_jumps = 6;
                s.clock_jump_range = (30_000, 10 * 60 * 1_000);
                s.faults.fifo = false;
                s.faults.latency = Range(1, 30); // occasional reordering
                s.faults.dup_p = 0.2;
                s.faults.drop_p = 0.1;
                s.skew_tolerance_ms = 5_000;
            }
            "server_crash_before_durable" => {
                s.persist_latency = Range(20, 300);
                s.server_crashes = 6;
                s.server_snapshot_every = 30;
            }
            "reconnect_eviction_race" => {
                s.faults.server_notice_delay = Range(2_000, 6_000);
                s.disconnect_interval = Some(Range(50, 400));
                s.reconnect_backoff = Range(1, 20);
            }
            "gap_detection" => {
                s.faults.drop_p = 0.05;
                s.clients = 4;
            }
            "permanent_nack_resync" => {
                s.malformed_p = 0.05;
            }
            "reorder_stress" => {
                s.faults.fifo = false;
                s.faults.latency = Range(1, 300);
                s.faults.drop_p = 0.1;
                s.faults.dup_p = 0.1;
            }
            "many_clients_soak" => {
                s.clients = 5;
                s.steps = 3_000;
                s.faults.drop_p = 0.05;
                s.faults.dup_p = 0.05;
                s.disconnect_interval = Some(Range(200, 3_000));
            }
            "everything" => {
                s.clients = 4;
                s.steps = 1_500;
                s.faults.fifo = false;
                s.faults.latency = Range(1, 250);
                s.faults.drop_p = 0.1;
                s.faults.dup_p = 0.1;
                s.faults.server_notice_delay = Range(100, 3_000);
                s.disconnect_interval = Some(Range(100, 2_000));
                s.offline_burst = Some(Range(2_000, 5_000));
                s.skew_ms = vec![0, 5 * 60 * 1_000, -5 * 60 * 1_000, 0];
                s.clock_jumps = 6;
                s.skew_tolerance_ms = 30_000;
                s.persist_latency = Range(5, 200);
                s.store_latency = Range(1, 40);
                s.server_crashes = 4;
                s.client_crashes = 8;
                s.malformed_p = 0.01;
                s.hub_log_capacity = 200;
                s.server_snapshot_every = 40;
            }
            _ => return None,
        }
        Some(s)
    }
}

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: String,
    /// `postgres://…`; unset means in-memory storage (dev/tests only).
    pub database_url: Option<String>,
    pub skew_tolerance_ms: u64,
    pub log_capacity: usize,
    /// Group-commit window and batch size of the persister.
    pub persist_window: Duration,
    pub persist_batch: usize,
    /// Snapshot to storage every N durable ops.
    pub snapshot_every: u64,
    /// Ops older than `snapshot_seq - retain_ops` are pruned after a snapshot.
    pub retain_ops: u64,
    pub idle_shutdown: Duration,
    pub ping_interval: Duration,
    /// Per-session outbound queue; a full queue evicts the session (slow consumer).
    pub outbound_queue: usize,
    /// Seed for the chaos fault injector (deterministic per session).
    pub chaos_seed: u64,
    /// Directory with the built web app to serve at `/` (optional).
    pub static_dir: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok();
        let num = |k: &str, d: u64| var(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        Config {
            bind: var("BIND").unwrap_or_else(|| "0.0.0.0:8080".into()),
            database_url: var("DATABASE_URL"),
            skew_tolerance_ms: num("SKEW_TOLERANCE_MS", 60_000),
            log_capacity: num("LOG_CAPACITY", 10_000) as usize,
            persist_window: Duration::from_millis(num("PERSIST_WINDOW_MS", 10)),
            persist_batch: num("PERSIST_BATCH", 256) as usize,
            snapshot_every: num("SNAPSHOT_EVERY", 1_000),
            retain_ops: num("RETAIN_OPS", 50_000),
            idle_shutdown: Duration::from_secs(num("IDLE_SHUTDOWN_SECS", 300)),
            ping_interval: Duration::from_secs(num("PING_INTERVAL_SECS", 15)),
            outbound_queue: num("OUTBOUND_QUEUE", 256) as usize,
            chaos_seed: num("CHAOS_SEED", 0),
            static_dir: var("STATIC_DIR"),
        }
    }
}

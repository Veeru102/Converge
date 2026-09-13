//! Server-side fault injection for demos and end-to-end tests. Applied per
//! session to inbound and outbound messages; seeded so a given message
//! sequence yields identical faults across runs.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ChaosConfig {
    pub enabled: bool,
    /// Added latency range in milliseconds (FIFO order is preserved).
    #[serde(default)]
    pub latency_ms: (u64, u64),
    #[serde(default)]
    pub drop_p: f64,
    #[serde(default)]
    pub dup_p: f64,
    /// Close each session after this many messages in either direction (0 = never).
    #[serde(default)]
    pub disconnect_every: u64,
}

#[derive(Clone, Default)]
pub struct Chaos {
    cfg: Arc<RwLock<ChaosConfig>>,
    seed: u64,
}

#[derive(Debug, PartialEq)]
pub enum Fate {
    Deliver { delay: Duration, dup: bool },
    Drop,
    Disconnect,
}

impl Chaos {
    pub fn new(seed: u64) -> Self {
        Chaos {
            cfg: Arc::default(),
            seed,
        }
    }
    pub fn get(&self) -> ChaosConfig {
        self.cfg.read().unwrap().clone()
    }
    pub fn set(&self, c: ChaosConfig) {
        *self.cfg.write().unwrap() = c;
    }
    pub fn session(&self, session_id: u64) -> SessionChaos {
        SessionChaos {
            cfg: self.cfg.clone(),
            rng: ChaCha8Rng::seed_from_u64(
                self.seed ^ session_id.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            ),
            count: 0,
        }
    }
}

pub struct SessionChaos {
    cfg: Arc<RwLock<ChaosConfig>>,
    rng: ChaCha8Rng,
    count: u64,
}

impl SessionChaos {
    pub fn fate(&mut self) -> Fate {
        let cfg = self.cfg.read().unwrap().clone();
        if !cfg.enabled {
            return Fate::Deliver {
                delay: Duration::ZERO,
                dup: false,
            };
        }
        self.count += 1;
        if cfg.disconnect_every > 0 && self.count % cfg.disconnect_every == 0 {
            return Fate::Disconnect;
        }
        if cfg.drop_p > 0.0 && self.rng.gen_bool(cfg.drop_p.min(1.0)) {
            metrics::counter!("converge_chaos_dropped_total").increment(1);
            return Fate::Drop;
        }
        let (lo, hi) = cfg.latency_ms;
        let delay = if hi > lo {
            self.rng.gen_range(lo..=hi)
        } else {
            lo
        };
        let dup = cfg.dup_p > 0.0 && self.rng.gen_bool(cfg.dup_p.min(1.0));
        if dup {
            metrics::counter!("converge_chaos_duplicated_total").increment(1);
        }
        Fate::Deliver {
            delay: Duration::from_millis(delay),
            dup,
        }
    }
}

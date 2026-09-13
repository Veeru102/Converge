use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// Uniform integer distribution in milliseconds (inclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range(pub u64, pub u64);

impl Range {
    pub const ZERO: Range = Range(0, 0);

    pub fn sample(&self, rng: &mut ChaCha8Rng) -> u64 {
        if self.0 >= self.1 {
            self.0
        } else {
            rng.gen_range(self.0..=self.1)
        }
    }
}

/// Per-connection fault plan. Applied to both directions.
#[derive(Clone, Debug)]
pub struct FaultPlan {
    pub latency: Range,
    /// TCP-like: messages on one connection arrive in send order.
    pub fifo: bool,
    pub drop_p: f64,
    pub dup_p: f64,
    pub dup_delay: Range,
    /// How long after a client-side close the server notices it.
    pub server_notice_delay: Range,
}

impl Default for FaultPlan {
    fn default() -> Self {
        FaultPlan {
            latency: Range(5, 20),
            fifo: true,
            drop_p: 0.0,
            dup_p: 0.0,
            dup_delay: Range(1, 50),
            server_notice_delay: Range(10, 100),
        }
    }
}

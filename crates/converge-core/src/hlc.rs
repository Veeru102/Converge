use crate::ids::ReplicaId;

/// Hybrid logical clock timestamp. Ordered by `(wall_ms, logical)`; combined
/// with the authoring replica (see [`Stamp`]) the order is total.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Hlc {
    pub wall_ms: u64,
    pub logical: u16,
}

impl Hlc {
    pub const MIN: Hlc = Hlc {
        wall_ms: 0,
        logical: 0,
    };

    pub const fn new(wall_ms: u64, logical: u16) -> Self {
        Hlc { wall_ms, logical }
    }
}

/// The LWW ordering key of a register write: `(wall_ms, logical, replica)`.
/// Two writes from different ops never compare equal because a replica's
/// HLCs are strictly increasing (invariant H1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Stamp {
    pub hlc: Hlc,
    pub replica: ReplicaId,
}

impl Stamp {
    pub const MIN: Stamp = Stamp {
        hlc: Hlc::MIN,
        replica: ReplicaId(0),
    };

    pub const fn new(hlc: Hlc, replica: ReplicaId) -> Self {
        Stamp { hlc, replica }
    }
}

/// Per-replica HLC generator.
///
/// Invariants (see `docs/INVARIANTS.md` H1, H7):
/// * `tick` returns strictly increasing values, whatever the physical clock does
///   (it may stall or go backwards);
/// * `wall_ms` never decreases;
/// * a `logical` overflow advances `wall_ms` by one instead of wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HlcClock {
    last: Hlc,
}

impl HlcClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restore a clock so that its next tick exceeds `high_water` (H6).
    pub fn from_high_water(high_water: Hlc) -> Self {
        HlcClock { last: high_water }
    }

    /// The highest timestamp issued or observed so far.
    pub fn high_water(&self) -> Hlc {
        self.last
    }

    /// Issue a new timestamp given the current physical clock in ms.
    pub fn tick(&mut self, physical_ms: u64) -> Hlc {
        let next = if physical_ms > self.last.wall_ms {
            Hlc::new(physical_ms, 0)
        } else {
            match self.last.logical.checked_add(1) {
                Some(l) => Hlc::new(self.last.wall_ms, l),
                None => Hlc::new(self.last.wall_ms + 1, 0),
            }
        };
        self.last = next;
        next
    }

    /// Account for a timestamp received from another replica so that every
    /// later `tick` is greater than it.
    pub fn observe(&mut self, remote: Hlc, physical_ms: u64) {
        let wall = self.last.wall_ms.max(remote.wall_ms).max(physical_ms);
        let logical = if wall == self.last.wall_ms && wall == remote.wall_ms {
            self.last.logical.max(remote.logical)
        } else if wall == self.last.wall_ms {
            self.last.logical
        } else if wall == remote.wall_ms {
            remote.logical
        } else {
            0
        };
        self.last = Hlc::new(wall, logical);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_strictly_increasing_when_clock_stalls() {
        let mut c = HlcClock::new();
        let a = c.tick(100);
        let b = c.tick(100);
        let d = c.tick(50); // physical clock went backwards
        assert!(a < b && b < d);
        assert_eq!(d.wall_ms, 100);
    }

    #[test]
    fn logical_overflow_bumps_wall() {
        let mut c = HlcClock::from_high_water(Hlc::new(10, u16::MAX));
        let t = c.tick(10);
        assert_eq!(t, Hlc::new(11, 0));
    }

    #[test]
    fn observe_pulls_forward_and_next_tick_exceeds_remote() {
        let mut c = HlcClock::new();
        c.tick(100);
        c.observe(Hlc::new(500, 7), 100);
        let t = c.tick(100);
        assert!(t > Hlc::new(500, 7));
        assert_eq!(t, Hlc::new(500, 8));
    }
}

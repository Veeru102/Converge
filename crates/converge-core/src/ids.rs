use std::fmt;

/// Identifies one replica (one browser tab). Random 64-bit value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplicaId(pub u64);

/// Identifies one operation: the authoring replica plus a per-replica counter
/// that is strictly increasing in authoring order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OpId {
    pub replica: ReplicaId,
    pub counter: u64,
}

/// An object is identified by the id of the op that created it.
pub type ObjectId = OpId;

impl OpId {
    pub const fn new(replica: u64, counter: u64) -> Self {
        OpId {
            replica: ReplicaId(replica),
            counter,
        }
    }
}

impl fmt::Display for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.replica, self.counter)
    }
}

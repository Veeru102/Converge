//! Wire protocol messages (see `docs/ARCHITECTURE.md` §5).
//!
//! These are the logical message types used by the hub, the client sync
//! engine and the simulator. The binary encoding (protobuf) lives in the
//! server/web adapters and converts to and from these types.

#![forbid(unsafe_code)]

#[cfg(feature = "json")]
pub mod json;
pub mod wire;

use converge_core::{Hlc, ObjectId, Op, OpId, ReplicaId};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DocId(pub String);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SessionId(pub u64);

#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserInfo {
    pub name: String,
    pub color: u32,
}

#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Hello {
    pub version: u32,
    pub doc: DocId,
    pub replica: ReplicaId,
    /// Highest contiguous seq the client holds.
    pub last_seq: u64,
    /// Force a snapshot catch-up (after a permanent NACK; protocol rule P7).
    pub want_snapshot: bool,
    pub user: UserInfo,
}

#[derive(Clone, PartialEq, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PresenceState {
    pub cursor: Option<(f64, f64)>,
    pub selection: Vec<ObjectId>,
}

#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PresenceEntry {
    pub replica: ReplicaId,
    pub user: UserInfo,
    pub state: PresenceState,
}

#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ClientMsg {
    Hello(Hello),
    /// Ops in counter order. Chunked to at most `MAX_SUBMIT_OPS`.
    Submit {
        ops: Vec<Op>,
    },
    Presence(PresenceState),
    Pong {
        nonce: u64,
    },
}

pub const MAX_SUBMIT_OPS: usize = 256;

#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CatchUp {
    /// Contiguous `(last_seq, durable_head]`.
    Ops(Vec<(u64, Op)>),
    /// Full durable state at `seq`.
    Snapshot { bytes: Vec<u8>, seq: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ByeReason {
    VersionMismatch,
    UnknownDoc,
    /// A newer session for the same replica took over (P4).
    Superseded,
    /// An op was rejected for clock skew; reconnect and re-timestamp (H3).
    ClockSkew,
    SlowConsumer,
    Restart,
    ProtocolError,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NackReason {
    /// Retryable after re-timestamping; the session is closed alongside.
    ClockSkew,
    /// Permanent.
    Malformed,
    /// Permanent.
    Rejected,
}

impl NackReason {
    pub fn is_permanent(self) -> bool {
        !matches!(self, NackReason::ClockSkew)
    }
}

#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ServerMsg {
    Welcome {
        session: SessionId,
        server_time_ms: u64,
        durable_head_seq: u64,
        catch_up: CatchUp,
        presence: Vec<PresenceEntry>,
    },
    Bye {
        reason: ByeReason,
    },
    /// Broadcast to every session (originator included) once durable.
    Commit {
        seq: u64,
        op: Op,
    },
    /// Reply to a duplicate `Submit` of an already-durable op.
    Ack {
        op_id: OpId,
        seq: Option<u64>,
    },
    Nack {
        op_id: OpId,
        reason: NackReason,
    },
    PresenceUpdate(PresenceEntry),
    PresenceLeave {
        replica: ReplicaId,
    },
    Ping {
        nonce: u64,
    },
}

impl ServerMsg {
    /// The HLC carried by this message, if any (for clock observation).
    pub fn hlc(&self) -> Option<Hlc> {
        match self {
            ServerMsg::Commit { op, .. } => Some(op.hlc),
            _ => None,
        }
    }
}

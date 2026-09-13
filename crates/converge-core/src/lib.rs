//! Converge document engine.
//!
//! Everything in this crate is pure: no I/O, no clocks, no randomness. The
//! state of a [`Document`] is a function of the *set* of operations applied to
//! it — never of their order or multiplicity. See `docs/ARCHITECTURE.md` §3.

#![forbid(unsafe_code)]

pub mod codec;
pub mod document;
pub mod fracindex;
pub mod hlc;
pub mod ids;
#[cfg(feature = "json")]
pub mod json;
pub mod op;
pub mod value;

pub use document::{Document, ObjectState, Register};
pub use hlc::{Hlc, HlcClock, Stamp};
pub use ids::{ObjectId, OpId, ReplicaId};
pub use op::{Op, OpKind, ValidationError};
pub use value::{ObjectKind, Value};

/// 32-byte BLAKE3 digest of a document's canonical form.
pub type Hash = [u8; 32];

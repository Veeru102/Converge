use crate::hlc::{Hlc, Stamp};
use crate::ids::{ObjectId, OpId};
use crate::value::{ObjectKind, Value};

pub const MAX_PROPS_PER_OP: usize = 64;
pub const MAX_STRING_BYTES: usize = 32 * 1024;
pub const MAX_KEY_BYTES: usize = 64;

/// One atomic intent from one replica.
#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Op {
    pub id: OpId,
    pub hlc: Hlc,
    pub kind: OpKind,
}

#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "op", rename_all = "snake_case"))]
pub enum OpKind {
    /// Creates the object whose id is this op's id.
    Create {
        kind: ObjectKind,
        props: Vec<(String, Value)>,
    },
    /// Writes each entry as an LWW register at this op's stamp.
    SetProps {
        object: ObjectId,
        entries: Vec<(String, Value)>,
    },
    Delete {
        object: ObjectId,
    },
    Restore {
        object: ObjectId,
    },
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ValidationError {
    #[error("too many props in one op ({0} > {MAX_PROPS_PER_OP})")]
    TooManyProps(usize),
    #[error("property key {0:?} is not a valid key")]
    BadKey(String),
    #[error("duplicate property key {0:?} in one op")]
    DuplicateKey(String),
    #[error("non-finite float for key {0:?}")]
    NonFiniteFloat(String),
    #[error("string for key {0:?} exceeds {MAX_STRING_BYTES} bytes")]
    StringTooLong(String),
    #[error("fractional index for key {0:?} is malformed")]
    BadFracIndex(String),
}

/// Keys are short ASCII identifiers so that byte order == code-unit order in
/// every implementation.
pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_BYTES
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

impl Op {
    /// The object this op touches.
    pub fn object(&self) -> ObjectId {
        match &self.kind {
            OpKind::Create { .. } => self.id,
            OpKind::SetProps { object, .. }
            | OpKind::Delete { object }
            | OpKind::Restore { object } => *object,
        }
    }

    pub fn stamp(&self) -> Stamp {
        Stamp::new(self.hlc, self.id.replica)
    }

    /// Shape validation performed by the server before acceptance and by the
    /// client before authoring. Does not depend on document state.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let entries: &[(String, Value)] = match &self.kind {
            OpKind::Create { props, .. } => props,
            OpKind::SetProps { entries, .. } => entries,
            OpKind::Delete { .. } | OpKind::Restore { .. } => &[],
        };
        if entries.len() > MAX_PROPS_PER_OP {
            return Err(ValidationError::TooManyProps(entries.len()));
        }
        for (i, (key, value)) in entries.iter().enumerate() {
            if !is_valid_key(key) {
                return Err(ValidationError::BadKey(key.clone()));
            }
            if entries[..i].iter().any(|(k, _)| k == key) {
                return Err(ValidationError::DuplicateKey(key.clone()));
            }
            match value {
                Value::F64(f) if !f.is_finite() => {
                    return Err(ValidationError::NonFiniteFloat(key.clone()))
                }
                Value::Str(s) if s.len() > MAX_STRING_BYTES => {
                    return Err(ValidationError::StringTooLong(key.clone()))
                }
                Value::FracIndex(s) if crate::fracindex::validate(s).is_err() => {
                    return Err(ValidationError::BadFracIndex(key.clone()))
                }
                _ => {}
            }
        }
        Ok(())
    }
}

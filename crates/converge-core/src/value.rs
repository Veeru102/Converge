use crate::ids::ObjectId;

/// Kind of an object, fixed by the op that creates it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ObjectKind {
    Rect,
    Text,
    Group,
    Connector,
    Ellipse,
    Line,
}

impl ObjectKind {
    pub fn tag(self) -> u8 {
        match self {
            ObjectKind::Rect => 1,
            ObjectKind::Text => 2,
            ObjectKind::Group => 3,
            ObjectKind::Connector => 4,
            ObjectKind::Ellipse => 5,
            ObjectKind::Line => 6,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            1 => ObjectKind::Rect,
            2 => ObjectKind::Text,
            3 => ObjectKind::Group,
            4 => ObjectKind::Connector,
            5 => ObjectKind::Ellipse,
            6 => ObjectKind::Line,
            _ => return None,
        })
    }
}

/// A property value. The engine is schema-agnostic; the UI decides what keys
/// mean. `F64` must be finite (enforced by [`crate::Op::validate`]).
#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(tag = "t", content = "v", rename_all = "snake_case")
)]
pub enum Value {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
    Color(u32),
    FracIndex(String),
    ObjRef(ObjectId),
}

impl Value {
    pub fn tag(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::I64(_) => 2,
            Value::F64(_) => 3,
            Value::Str(_) => 4,
            Value::Color(_) => 5,
            Value::FracIndex(_) => 6,
            Value::ObjRef(_) => 7,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F64(v) => Some(*v),
            Value::I64(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::FracIndex(s) => Some(s),
            _ => None,
        }
    }
}

//! Canonical binary form of a [`Document`].
//!
//! The same bytes serve two purposes: they are the input to the convergence
//! hash, and (with a small header) they are the snapshot format. A snapshot
//! therefore round-trips to exactly the bytes that were hashed. All integers
//! are little-endian and fixed width; objects are ordered by id and props by
//! key, so the encoding is a pure function of the logical state.

use std::collections::BTreeMap;

use crate::document::{Document, ObjectState, Register};
use crate::hlc::{Hlc, Stamp};
use crate::ids::{OpId, ReplicaId};
use crate::value::{ObjectKind, Value};
use crate::Hash;

const SNAPSHOT_MAGIC: &[u8; 4] = b"CVGS";
const SNAPSHOT_VERSION: u8 = 1;

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum DecodeError {
    #[error("unexpected end of input")]
    Eof,
    #[error("bad magic or version")]
    BadHeader,
    #[error("invalid tag {0}")]
    BadTag(u8),
    #[error("invalid utf-8")]
    Utf8,
    #[error("trailing bytes")]
    Trailing,
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn stamp(&mut self, s: &Stamp) {
        self.u64(s.hlc.wall_ms);
        self.u16(s.hlc.logical);
        self.u64(s.replica.0);
    }
    fn value(&mut self, v: &Value) {
        self.u8(v.tag());
        match v {
            Value::Null => {}
            Value::Bool(b) => self.u8(*b as u8),
            Value::I64(i) => self.i64(*i),
            Value::F64(f) => self.u64(f.to_bits()),
            Value::Str(s) | Value::FracIndex(s) => self.bytes(s.as_bytes()),
            Value::Color(c) => self.u32(*c),
            Value::ObjRef(id) => {
                self.u64(id.replica.0);
                self.u64(id.counter);
            }
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.pos + n > self.buf.len() {
            return Err(DecodeError::Eof);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, DecodeError> {
        let n = self.u32()? as usize;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| DecodeError::Utf8)
    }
    fn stamp(&mut self) -> Result<Stamp, DecodeError> {
        let wall = self.u64()?;
        let logical = self.u16()?;
        let replica = self.u64()?;
        Ok(Stamp::new(Hlc::new(wall, logical), ReplicaId(replica)))
    }
    fn value(&mut self) -> Result<Value, DecodeError> {
        let tag = self.u8()?;
        Ok(match tag {
            0 => Value::Null,
            1 => Value::Bool(self.u8()? != 0),
            2 => Value::I64(self.i64()?),
            3 => Value::F64(f64::from_bits(self.u64()?)),
            4 => Value::Str(self.string()?),
            5 => Value::Color(self.u32()?),
            6 => Value::FracIndex(self.string()?),
            7 => {
                let r = self.u64()?;
                let c = self.u64()?;
                Value::ObjRef(OpId::new(r, c))
            }
            t => return Err(DecodeError::BadTag(t)),
        })
    }
}

/// Canonical bytes of the full state (ghosts and tombstones included).
pub fn canonical_bytes(doc: &Document) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.u64(doc.len() as u64);
    for (id, obj) in doc.objects() {
        w.u64(id.replica.0);
        w.u64(id.counter);
        w.u8(obj.kind.map(ObjectKind::tag).unwrap_or(0));
        w.u8(obj.deleted.value as u8);
        w.stamp(&obj.deleted.at);
        w.u32(obj.props.len() as u32);
        for (k, reg) in &obj.props {
            w.bytes(k.as_bytes());
            w.value(&reg.value);
            w.stamp(&reg.at);
        }
    }
    w.0
}

/// BLAKE3 over the canonical bytes — the convergence oracle.
pub fn hash(doc: &Document) -> Hash {
    *blake3::hash(&canonical_bytes(doc)).as_bytes()
}

pub fn hash_hex(doc: &Document) -> String {
    hex(&hash(doc))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Snapshot = header + canonical bytes.
pub fn encode_snapshot(doc: &Document) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.extend_from_slice(SNAPSHOT_MAGIC);
    out.push(SNAPSHOT_VERSION);
    out.extend(canonical_bytes(doc));
    out
}

pub fn decode_snapshot(bytes: &[u8]) -> Result<Document, DecodeError> {
    if bytes.len() < 5 || &bytes[..4] != SNAPSHOT_MAGIC || bytes[4] != SNAPSHOT_VERSION {
        return Err(DecodeError::BadHeader);
    }
    let mut r = Reader { buf: bytes, pos: 5 };
    let mut doc = Document::new();
    let n = r.u64()?;
    for _ in 0..n {
        let replica = r.u64()?;
        let counter = r.u64()?;
        let kind_tag = r.u8()?;
        let kind = if kind_tag == 0 {
            None
        } else {
            Some(ObjectKind::from_tag(kind_tag).ok_or(DecodeError::BadTag(kind_tag))?)
        };
        let deleted_v = r.u8()? != 0;
        let deleted_at = r.stamp()?;
        let np = r.u32()?;
        let mut props = BTreeMap::new();
        for _ in 0..np {
            let key = r.string()?;
            let value = r.value()?;
            let at = r.stamp()?;
            props.insert(key, Register::new(value, at));
        }
        doc.insert_raw(
            OpId::new(replica, counter),
            ObjectState {
                kind,
                deleted: Register::new(deleted_v, deleted_at),
                props,
            },
        );
    }
    if r.pos != bytes.len() {
        return Err(DecodeError::Trailing);
    }
    Ok(doc)
}

use std::collections::BTreeMap;

use crate::hlc::Stamp;
use crate::ids::ObjectId;
use crate::op::{Op, OpKind};
use crate::value::{ObjectKind, Value};

/// A last-writer-wins register.
#[derive(Clone, PartialEq, Debug)]
pub struct Register<T> {
    pub value: T,
    pub at: Stamp,
}

impl<T> Register<T> {
    pub fn new(value: T, at: Stamp) -> Self {
        Register { value, at }
    }

    /// Merge a write. Returns `true` if the register changed. Equal stamps
    /// only arise from the same op delivered twice, so they are a no-op.
    pub fn merge(&mut self, value: T, at: Stamp) -> bool {
        if at > self.at {
            self.value = value;
            self.at = at;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct ObjectState {
    /// `None` while the object is a ghost (ops seen, `Create` not yet seen).
    pub kind: Option<ObjectKind>,
    pub deleted: Register<bool>,
    pub props: BTreeMap<String, Register<Value>>,
}

impl ObjectState {
    fn ghost() -> Self {
        ObjectState {
            kind: None,
            deleted: Register::new(false, Stamp::MIN),
            props: BTreeMap::new(),
        }
    }

    pub fn created(&self) -> bool {
        self.kind.is_some()
    }

    /// Rendered ⇔ created and not tombstoned.
    pub fn visible(&self) -> bool {
        self.created() && !self.deleted.value
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.props.get(key).map(|r| &r.value)
    }

    fn merge_prop(&mut self, key: &str, value: &Value, at: Stamp) -> bool {
        match self.props.get_mut(key) {
            Some(reg) => reg.merge(value.clone(), at),
            None => {
                self.props
                    .insert(key.to_string(), Register::new(value.clone(), at));
                true
            }
        }
    }
}

/// The replicated document state. `apply` is commutative, associative and
/// idempotent over ops, so every replica that has seen the same *set* of ops
/// holds byte-identical state (see [`crate::codec::canonical_bytes`]).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Document {
    objects: BTreeMap<ObjectId, ObjectState>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one op. Returns `true` if any state changed (false for
    /// duplicates and stale writes).
    pub fn apply(&mut self, op: &Op) -> bool {
        let at = op.stamp();
        let object = op.object();
        let entry = self
            .objects
            .entry(object)
            .or_insert_with(ObjectState::ghost);
        match &op.kind {
            OpKind::Create { kind, props } => {
                let mut changed = false;
                if entry.kind.is_none() {
                    entry.kind = Some(*kind);
                    changed = true;
                }
                for (k, v) in props {
                    changed |= entry.merge_prop(k, v, at);
                }
                changed
            }
            OpKind::SetProps { entries, .. } => {
                let mut changed = false;
                for (k, v) in entries {
                    changed |= entry.merge_prop(k, v, at);
                }
                changed
            }
            OpKind::Delete { .. } => entry.deleted.merge(true, at),
            OpKind::Restore { .. } => entry.deleted.merge(false, at),
        }
    }

    pub fn get(&self, id: &ObjectId) -> Option<&ObjectState> {
        self.objects.get(id)
    }

    /// All objects including ghosts and tombstones, in canonical order.
    pub fn objects(&self) -> impl Iterator<Item = (&ObjectId, &ObjectState)> {
        self.objects.iter()
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Visible objects in render order: by `z` (fractional index, missing
    /// sorts first) then by id.
    pub fn render_order(&self) -> Vec<(&ObjectId, &ObjectState)> {
        let mut v: Vec<_> = self.objects.iter().filter(|(_, o)| o.visible()).collect();
        v.sort_by(|(ida, a), (idb, b)| {
            let za = a.get("z").and_then(Value::as_str);
            let zb = b.get("z").and_then(Value::as_str);
            za.cmp(&zb).then(ida.cmp(idb))
        });
        v
    }

    pub(crate) fn insert_raw(&mut self, id: ObjectId, state: ObjectState) {
        self.objects.insert(id, state);
    }
}

#![allow(dead_code)]
//! Shared op generators for property tests and fixture generation.

use converge_core::*;
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use serde_json::{json, Value as J};

pub struct Gen {
    pub rng: ChaCha8Rng,
    pub replicas: Vec<u64>,
    pub clocks: Vec<HlcClock>,
    pub counters: Vec<u64>,
    pub physical: Vec<u64>,
    pub objects: Vec<ObjectId>,
}

impl Gen {
    pub fn new(seed: u64, replicas: Vec<u64>) -> Self {
        let n = replicas.len();
        Gen {
            rng: ChaCha8Rng::seed_from_u64(seed),
            replicas,
            clocks: vec![HlcClock::new(); n],
            counters: vec![1; n],
            physical: vec![1_000; n],
            objects: Vec::new(),
        }
    }

    fn value(&mut self) -> Value {
        match self.rng.gen_range(0..7) {
            0 => Value::Null,
            1 => Value::Bool(self.rng.gen()),
            2 => Value::I64(self.rng.gen_range(-1_000_000..1_000_000)),
            3 => Value::F64(self.rng.gen_range(-500.0..500.0f64)),
            4 => Value::Str(format!("s{}", self.rng.gen_range(0..1000))),
            5 => Value::Color(self.rng.gen()),
            _ => {
                Value::FracIndex(["a0", "a1", "a0V", "Zz", "a0G"][self.rng.gen_range(0..5)].into())
            }
        }
    }

    fn entries(&mut self) -> Vec<(String, Value)> {
        let keys = ["x", "y", "w", "h", "z", "fill", "text"];
        let n = self.rng.gen_range(1..=3);
        let mut out: Vec<(String, Value)> = Vec::new();
        for _ in 0..n {
            let k = keys[self.rng.gen_range(0..keys.len())];
            if out.iter().any(|(kk, _)| kk == k) {
                continue;
            }
            let v = self.value();
            out.push((k.to_string(), v));
        }
        out
    }

    /// Next op from replica index `r`. Physical clocks advance slowly and
    /// overlap heavily across replicas so that HLC ties are common.
    pub fn next_op(&mut self, r: usize) -> Op {
        let step = self.rng.gen_range(0..3);
        self.physical[r] += step;
        let hlc = self.clocks[r].tick(self.physical[r]);
        let id = OpId::new(self.replicas[r], self.counters[r]);
        self.counters[r] += 1;
        let kind = if self.objects.is_empty() || self.rng.gen_range(0..10) < 2 {
            let kind = if self.rng.gen() {
                ObjectKind::Rect
            } else {
                ObjectKind::Text
            };
            self.objects.push(id);
            let props = self.entries();
            OpKind::Create { kind, props }
        } else {
            let object = self.objects[self.rng.gen_range(0..self.objects.len())];
            match self.rng.gen_range(0..10) {
                0 => OpKind::Delete { object },
                1 => OpKind::Restore { object },
                _ => OpKind::SetProps {
                    object,
                    entries: self.entries(),
                },
            }
        };
        Op { id, hlc, kind }
    }

    /// Occasionally let a replica observe another's clock, as a live client would.
    pub fn observe(&mut self, r: usize, hlc: Hlc) {
        let p = self.physical[r];
        self.clocks[r].observe(hlc, p);
    }

    pub fn ops(&mut self, n: usize) -> Vec<Op> {
        let mut ops = Vec::with_capacity(n);
        for _ in 0..n {
            let r = self.rng.gen_range(0..self.replicas.len());
            let op = self.next_op(r);
            if self.rng.gen_range(0..4) == 0 {
                let other = self.rng.gen_range(0..self.replicas.len());
                let h = op.hlc;
                self.observe(other, h);
            }
            ops.push(op);
        }
        ops
    }
}

pub fn apply_all(ops: &[Op]) -> Document {
    let mut d = Document::new();
    for op in ops {
        d.apply(op);
    }
    d
}

pub fn shuffled_with_dups(ops: &[Op], rng: &mut ChaCha8Rng) -> Vec<Op> {
    let mut v: Vec<Op> = ops.to_vec();
    let dups = rng.gen_range(0..=ops.len() / 2);
    for _ in 0..dups {
        let i = rng.gen_range(0..ops.len());
        v.push(ops[i].clone());
    }
    v.shuffle(rng);
    v
}

// ---- JSON fixtures (u64 as decimal strings so JS can use BigInt) ----

fn value_json(v: &Value) -> J {
    match v {
        Value::Null => json!({"t": "null"}),
        Value::Bool(b) => json!({"t": "bool", "v": b}),
        Value::I64(i) => json!({"t": "i64", "v": i.to_string()}),
        Value::F64(f) => json!({"t": "f64", "v": f}),
        Value::Str(s) => json!({"t": "str", "v": s}),
        Value::Color(c) => json!({"t": "color", "v": c}),
        Value::FracIndex(s) => json!({"t": "frac", "v": s}),
        Value::ObjRef(id) => json!({"t": "ref", "v": id_json(id)}),
    }
}

pub fn id_json(id: &OpId) -> J {
    json!({"replica": id.replica.0.to_string(), "counter": id.counter.to_string()})
}

fn entries_json(e: &[(String, Value)]) -> J {
    J::Array(e.iter().map(|(k, v)| json!([k, value_json(v)])).collect())
}

pub fn op_json(op: &Op) -> J {
    let kind = match &op.kind {
        OpKind::Create { kind, props } => json!({
            "op": "create",
            "kind": match kind { ObjectKind::Rect => "rect", ObjectKind::Text => "text", ObjectKind::Group => "group", ObjectKind::Connector => "connector" },
            "props": entries_json(props),
        }),
        OpKind::SetProps { object, entries } => {
            json!({"op": "set_props", "object": id_json(object), "entries": entries_json(entries)})
        }
        OpKind::Delete { object } => json!({"op": "delete", "object": id_json(object)}),
        OpKind::Restore { object } => json!({"op": "restore", "object": id_json(object)}),
    };
    json!({
        "id": id_json(&op.id),
        "hlc": {"wall": op.hlc.wall_ms.to_string(), "logical": op.hlc.logical},
        "kind": kind,
    })
}

pub fn fixture_json(name: &str, ops: &[Op]) -> J {
    let doc = apply_all(ops);
    let render: Vec<J> = doc
        .render_order()
        .iter()
        .map(|(id, _)| id_json(id))
        .collect();
    json!({
        "name": name,
        "ops": ops.iter().map(op_json).collect::<Vec<_>>(),
        "expected_hash": codec::hash_hex(&doc),
        "expected_snapshot": codec::hex(&codec::encode_snapshot(&doc)),
        "render_order": render,
    })
}

//! JSON form of ops shared with the TypeScript engine, the fixtures and the
//! simulator traces. 64-bit integers are decimal strings so that JavaScript
//! can read them as `BigInt` without precision loss.

use serde_json::{json, Value as J};

use crate::{Hlc, ObjectKind, Op, OpId, OpKind, Value};

#[derive(thiserror::Error, Debug)]
#[error("bad op json: {0}")]
pub struct JsonError(pub String);

pub fn id_to_json(id: &OpId) -> J {
    json!({"replica": id.replica.0.to_string(), "counter": id.counter.to_string()})
}

pub fn hlc_to_json(h: &Hlc) -> J {
    json!({"wall": h.wall_ms.to_string(), "logical": h.logical})
}

pub fn value_to_json(v: &Value) -> J {
    match v {
        Value::Null => json!({"t": "null"}),
        Value::Bool(b) => json!({"t": "bool", "v": b}),
        Value::I64(i) => json!({"t": "i64", "v": i.to_string()}),
        Value::F64(f) => json!({"t": "f64", "v": f}),
        Value::Str(s) => json!({"t": "str", "v": s}),
        Value::Color(c) => json!({"t": "color", "v": c}),
        Value::FracIndex(s) => json!({"t": "frac", "v": s}),
        Value::ObjRef(id) => json!({"t": "ref", "v": id_to_json(id)}),
    }
}

fn entries_to_json(e: &[(String, Value)]) -> J {
    J::Array(
        e.iter()
            .map(|(k, v)| json!([k, value_to_json(v)]))
            .collect(),
    )
}

pub fn kind_name(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Rect => "rect",
        ObjectKind::Text => "text",
        ObjectKind::Group => "group",
        ObjectKind::Connector => "connector",
    }
}

pub fn op_to_json(op: &Op) -> J {
    let kind = match &op.kind {
        OpKind::Create { kind, props } => {
            json!({"op": "create", "kind": kind_name(*kind), "props": entries_to_json(props)})
        }
        OpKind::SetProps { object, entries } => {
            json!({"op": "set_props", "object": id_to_json(object), "entries": entries_to_json(entries)})
        }
        OpKind::Delete { object } => json!({"op": "delete", "object": id_to_json(object)}),
        OpKind::Restore { object } => json!({"op": "restore", "object": id_to_json(object)}),
    };
    json!({"id": id_to_json(&op.id), "hlc": hlc_to_json(&op.hlc), "kind": kind})
}

fn err(m: &str) -> JsonError {
    JsonError(m.into())
}

pub fn u64_from_json(j: &J) -> Result<u64, JsonError> {
    j.as_str()
        .ok_or_else(|| err("expected u64 string"))?
        .parse()
        .map_err(|_| err("bad u64"))
}

pub fn id_from_json(j: &J) -> Result<OpId, JsonError> {
    Ok(OpId::new(
        u64_from_json(&j["replica"])?,
        u64_from_json(&j["counter"])?,
    ))
}

pub fn hlc_from_json(j: &J) -> Result<Hlc, JsonError> {
    let logical = j["logical"].as_u64().ok_or_else(|| err("logical"))?;
    Ok(Hlc::new(
        u64_from_json(&j["wall"])?,
        u16::try_from(logical).map_err(|_| err("logical range"))?,
    ))
}

pub fn value_from_json(j: &J) -> Result<Value, JsonError> {
    Ok(match j["t"].as_str().ok_or_else(|| err("value tag"))? {
        "null" => Value::Null,
        "bool" => Value::Bool(j["v"].as_bool().ok_or_else(|| err("bool"))?),
        "i64" => Value::I64(
            j["v"]
                .as_str()
                .ok_or_else(|| err("i64"))?
                .parse()
                .map_err(|_| err("i64"))?,
        ),
        "f64" => Value::F64(j["v"].as_f64().ok_or_else(|| err("f64"))?),
        "str" => Value::Str(j["v"].as_str().ok_or_else(|| err("str"))?.into()),
        "color" => Value::Color(j["v"].as_u64().ok_or_else(|| err("color"))? as u32),
        "frac" => Value::FracIndex(j["v"].as_str().ok_or_else(|| err("frac"))?.into()),
        "ref" => Value::ObjRef(id_from_json(&j["v"])?),
        t => return Err(JsonError(format!("bad value tag {t}"))),
    })
}

fn entries_from_json(j: &J) -> Result<Vec<(String, Value)>, JsonError> {
    j.as_array()
        .ok_or_else(|| err("entries"))?
        .iter()
        .map(|e| {
            Ok((
                e[0].as_str().ok_or_else(|| err("key"))?.to_string(),
                value_from_json(&e[1])?,
            ))
        })
        .collect()
}

pub fn kind_from_name(s: &str) -> Option<ObjectKind> {
    Some(match s {
        "rect" => ObjectKind::Rect,
        "text" => ObjectKind::Text,
        "group" => ObjectKind::Group,
        "connector" => ObjectKind::Connector,
        _ => return None,
    })
}

pub fn op_from_json(j: &J) -> Result<Op, JsonError> {
    let k = &j["kind"];
    let kind = match k["op"].as_str().ok_or_else(|| err("op"))? {
        "create" => OpKind::Create {
            kind: kind_from_name(k["kind"].as_str().unwrap_or(""))
                .ok_or_else(|| err("object kind"))?,
            props: entries_from_json(&k["props"])?,
        },
        "set_props" => OpKind::SetProps {
            object: id_from_json(&k["object"])?,
            entries: entries_from_json(&k["entries"])?,
        },
        "delete" => OpKind::Delete {
            object: id_from_json(&k["object"])?,
        },
        "restore" => OpKind::Restore {
            object: id_from_json(&k["object"])?,
        },
        o => return Err(JsonError(format!("bad op {o}"))),
    };
    Ok(Op {
        id: id_from_json(&j["id"])?,
        hlc: hlc_from_json(&j["hlc"])?,
        kind,
    })
}

//! Cross-implementation conformance fixtures.
//!
//! `cargo test -p converge-core --test fixtures -- --ignored generate` rewrites
//! `fixtures/`; the non-ignored tests verify the checked-in files, and the
//! TypeScript engine replays the same files in vitest.

mod common;

use common::*;
use converge_core::*;
use serde_json::{json, Value as J};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn op(replica: u64, counter: u64, wall: u64, logical: u16, kind: OpKind) -> Op {
    Op {
        id: OpId::new(replica, counter),
        hlc: Hlc::new(wall, logical),
        kind,
    }
}

fn f(v: f64) -> Value {
    Value::F64(v)
}

fn named_scenarios() -> Vec<(&'static str, Vec<Op>)> {
    let a = 0x1111_0000_0000_0001u64;
    let b = 0x2222_0000_0000_0002u64;
    let mut out = Vec::new();

    // Two replicas move the same object concurrently; the later stamp wins x AND y.
    let create = op(
        a,
        1,
        100,
        0,
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![
                ("x".into(), f(0.0)),
                ("y".into(), f(0.0)),
                ("z".into(), Value::FracIndex("a0".into())),
            ],
        },
    );
    out.push((
        "concurrent_move",
        vec![
            create.clone(),
            op(
                a,
                2,
                200,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("x".into(), f(10.0)), ("y".into(), f(10.0))],
                },
            ),
            op(
                b,
                1,
                200,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("x".into(), f(-5.0)), ("y".into(), f(-5.0))],
                },
            ),
        ],
    ));

    // Update concurrent with delete: object ends deleted but keeps the update; restore reveals it.
    out.push((
        "delete_vs_update",
        vec![
            create.clone(),
            op(
                b,
                1,
                150,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("x".into(), f(42.0))],
                },
            ),
            op(a, 2, 150, 0, OpKind::Delete { object: create.id }),
        ],
    ));
    out.push((
        "restore_after_update",
        vec![
            create.clone(),
            op(
                b,
                1,
                150,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("x".into(), f(42.0))],
                },
            ),
            op(a, 2, 150, 0, OpKind::Delete { object: create.id }),
            op(b, 2, 160, 0, OpKind::Restore { object: create.id }),
        ],
    ));

    // SetProps and Delete arrive before Create.
    out.push((
        "ghost_before_create",
        vec![
            op(
                b,
                1,
                120,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("fill".into(), Value::Color(0xff00ff))],
                },
            ),
            op(b, 2, 130, 0, OpKind::Delete { object: create.id }),
            create.clone(),
        ],
    ));
    out.push((
        "ghost_never_created",
        vec![op(
            b,
            1,
            120,
            0,
            OpKind::SetProps {
                object: OpId::new(a, 99),
                entries: vec![("x".into(), f(1.0))],
            },
        )],
    ));

    // Same (wall, logical); replica ids that differ only in low bits beyond 2^53, and at u64::MAX.
    let r_lo = (1u64 << 53) + 1;
    let r_hi = 1u64 << 53;
    let c2 = op(
        r_hi,
        1,
        100,
        0,
        OpKind::Create {
            kind: ObjectKind::Text,
            props: vec![("text".into(), Value::Str("base".into()))],
        },
    );
    out.push((
        "hlc_tie_replica_tiebreak",
        vec![
            c2.clone(),
            op(
                r_lo,
                1,
                500,
                3,
                OpKind::SetProps {
                    object: c2.id,
                    entries: vec![("text".into(), Value::Str("from 2^53+1".into()))],
                },
            ),
            op(
                r_hi,
                2,
                500,
                3,
                OpKind::SetProps {
                    object: c2.id,
                    entries: vec![("text".into(), Value::Str("from 2^53".into()))],
                },
            ),
            op(
                u64::MAX,
                1,
                500,
                3,
                OpKind::SetProps {
                    object: c2.id,
                    entries: vec![("size".into(), Value::I64(12))],
                },
            ),
            op(
                u64::MAX - 1,
                1,
                500,
                3,
                OpKind::SetProps {
                    object: c2.id,
                    entries: vec![("size".into(), Value::I64(13))],
                },
            ),
            op(
                1,
                1,
                500,
                2,
                OpKind::SetProps {
                    object: c2.id,
                    entries: vec![("color".into(), Value::Color(1))],
                },
            ),
            op(
                2,
                1,
                499,
                65535,
                OpKind::SetProps {
                    object: c2.id,
                    entries: vec![("color".into(), Value::Color(2))],
                },
            ),
        ],
    ));

    // Equal fractional keys: render order falls back to object id.
    let ka = op(
        b,
        1,
        100,
        0,
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("z".into(), Value::FracIndex("a1".into()))],
        },
    );
    let kb = op(
        a,
        1,
        100,
        1,
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("z".into(), Value::FracIndex("a1".into()))],
        },
    );
    let kc = op(
        a,
        2,
        100,
        2,
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("z".into(), Value::FracIndex("a0V".into()))],
        },
    );
    let kd = op(
        b,
        2,
        100,
        3,
        OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![],
        },
    );
    out.push(("frac_index_ties_and_missing", vec![ka, kb, kc, kd]));

    // Value edge cases: negative zero, large ints, empty strings, refs.
    out.push((
        "value_edge_cases",
        vec![op(
            a,
            1,
            1,
            0,
            OpKind::Create {
                kind: ObjectKind::Connector,
                props: vec![
                    ("neg_zero".into(), f(-0.0)),
                    ("tiny".into(), f(5e-324)),
                    ("big".into(), f(1.7976931348623157e308)),
                    ("i_min".into(), Value::I64(i64::MIN)),
                    ("i_max".into(), Value::I64(i64::MAX)),
                    ("empty".into(), Value::Str(String::new())),
                    ("uni".into(), Value::Str("héllo ✓ 𝄞".into())),
                    ("null".into(), Value::Null),
                    ("t".into(), Value::Bool(true)),
                    ("from".into(), Value::ObjRef(OpId::new(u64::MAX, u64::MAX))),
                    ("color".into(), Value::Color(u32::MAX)),
                ],
            },
        )],
    ));

    // Same op delivered three times plus a stale write.
    out.push((
        "duplicates_and_stale",
        vec![
            create.clone(),
            create.clone(),
            op(
                a,
                2,
                300,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("x".into(), f(3.0))],
                },
            ),
            op(
                b,
                1,
                250,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("x".into(), f(2.5))],
                },
            ),
            op(
                a,
                2,
                300,
                0,
                OpKind::SetProps {
                    object: create.id,
                    entries: vec![("x".into(), f(3.0))],
                },
            ),
            create,
        ],
    ));

    out.push(("empty", vec![]));
    out
}

#[test]
#[ignore]
fn generate() {
    let dir = fixtures_dir();
    std::fs::create_dir_all(dir.join("engine")).unwrap();
    for entry in std::fs::read_dir(dir.join("engine")).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let mut all = named_scenarios();
    for seed in 0..40u64 {
        let replicas = vec![
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            (seed + 1).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) | 1,
            seed + 3,
        ];
        let mut g = Gen::new(seed, replicas);
        let n = 5 + (seed as usize * 7) % 60;
        let ops = g.ops(n);
        all.push((Box::leak(format!("random_{seed:02}").into_boxed_str()), ops));
    }
    for (name, ops) in &all {
        let j = fixture_json(name, ops);
        std::fs::write(
            dir.join("engine").join(format!("{name}.json")),
            serde_json::to_string_pretty(&j).unwrap(),
        )
        .unwrap();
    }

    // Fractional index vectors: a deterministic insertion sequence plus error cases.
    let mut keys: Vec<String> = vec![];
    let mut cases: Vec<J> = vec![];
    let push = |a: Option<&str>, b: Option<&str>, cases: &mut Vec<J>| -> String {
        let r = fracindex::between(a, b).unwrap();
        cases.push(json!({"a": a, "b": b, "expected": r}));
        r
    };
    keys.push(push(None, None, &mut cases));
    for i in 0..120usize {
        let k = match i % 4 {
            0 => push(keys.last().map(String::as_str), None, &mut cases),
            1 => push(None, Some(&keys[0]), &mut cases),
            2 => {
                let m = keys.len() / 2;
                push(Some(&keys[m - 1]), Some(&keys[m]), &mut cases)
            }
            _ => {
                let m = keys.len() - 1;
                push(Some(&keys[m - 1]), Some(&keys[m]), &mut cases)
            }
        };
        match i % 4 {
            0 => keys.push(k),
            1 => keys.insert(0, k),
            2 => keys.insert(keys.len() / 2, k),
            _ => keys.insert(keys.len() - 1, k),
        }
    }
    for (a, b) in [
        (Some("a0"), Some("a1")),
        (Some("Zz"), Some("a0")),
        (None, Some("Y00")),
        (Some("bzz"), None),
        (Some("b125"), Some("b129")),
        (Some("zzzzzzzzzzzzzzzzzzzzzzzzzzz"), None),
    ] {
        push(a, b, &mut cases);
    }
    let errors = json!([
        {"a": null, "b": "A00000000000000000000000000"},
        {"a": "a00", "b": null},
        {"a": "a1", "b": "a0"},
        {"a": "a0", "b": "a0"},
        {"a": "!", "b": null},
    ]);
    std::fs::write(
        dir.join("fracindex.json"),
        serde_json::to_string_pretty(&json!({"cases": cases, "errors": errors})).unwrap(),
    )
    .unwrap();

    // Stamp ordering vectors.
    let stamps = [
        Stamp::new(Hlc::new(5, 5), ReplicaId((1u64 << 53) + 1)),
        Stamp::new(Hlc::new(5, 5), ReplicaId(1u64 << 53)),
        Stamp::new(Hlc::new(5, 5), ReplicaId(u64::MAX)),
        Stamp::new(Hlc::new(5, 5), ReplicaId(u64::MAX - 1)),
        Stamp::new(Hlc::new(5, 6), ReplicaId(0)),
        Stamp::new(Hlc::new(4, 65535), ReplicaId(u64::MAX)),
        Stamp::new(Hlc::new(u64::MAX, 0), ReplicaId(0)),
        Stamp::new(Hlc::new(0, 0), ReplicaId(0)),
    ];
    let mut pairs = vec![];
    for x in &stamps {
        for y in &stamps {
            pairs.push(json!({
                "a": {"wall": x.hlc.wall_ms.to_string(), "logical": x.hlc.logical, "replica": x.replica.0.to_string()},
                "b": {"wall": y.hlc.wall_ms.to_string(), "logical": y.hlc.logical, "replica": y.replica.0.to_string()},
                "cmp": match x.cmp(y) { std::cmp::Ordering::Less => -1, std::cmp::Ordering::Equal => 0, std::cmp::Ordering::Greater => 1 },
            }));
        }
    }
    // HLC clock trace: (physical, optional observe) -> issued hlc.
    let mut clock = HlcClock::new();
    let mut trace = vec![];
    let script: Vec<(u64, Option<Hlc>)> = vec![
        (100, None),
        (100, None),
        (50, None),
        (100, Some(Hlc::new(500, 7))),
        (100, None),
        (600, None),
        (600, Some(Hlc::new(600, 9))),
        (600, None),
        (10, Some(Hlc::new(10, 65535))),
        (10, None),
        (700, None),
    ];
    for (phys, obs) in &script {
        if let Some(o) = obs {
            clock.observe(*o, *phys);
        }
        let t = clock.tick(*phys);
        trace.push(json!({"physical": phys.to_string(), "observe": obs.map(|o| json!({"wall": o.wall_ms.to_string(), "logical": o.logical})), "tick": {"wall": t.wall_ms.to_string(), "logical": t.logical}}));
    }
    std::fs::write(
        dir.join("hlc.json"),
        serde_json::to_string_pretty(&json!({"stamp_pairs": pairs, "clock_trace": trace})).unwrap(),
    )
    .unwrap();
}

#[test]
fn checked_in_engine_fixtures_match() {
    let dir = fixtures_dir().join("engine");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures/engine exists (run the generate test)") {
        let path = entry.unwrap().path();
        let j: J = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let ops: Vec<Op> = j["ops"]
            .as_array()
            .unwrap()
            .iter()
            .map(op_from_json)
            .collect();
        let doc = apply_all(&ops);
        assert_eq!(
            codec::hash_hex(&doc),
            j["expected_hash"],
            "{}",
            path.display()
        );
        assert_eq!(
            codec::hex(&codec::encode_snapshot(&doc)),
            j["expected_snapshot"],
            "{}",
            path.display()
        );
        count += 1;
    }
    assert!(count >= 10);
}

fn op_from_json(j: &J) -> Op {
    converge_core::json::op_from_json(j).unwrap()
}

mod common;

use common::*;
use converge_core::*;
use proptest::prelude::*;
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

fn hlc_strategy() -> impl Strategy<Value = Hlc> {
    (0u64..50, any::<u16>()).prop_map(|(w, l)| Hlc::new(w, l))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..Default::default() })]

    /// The fundamental property: any permutation of any multiset of the same
    /// op set yields identical canonical bytes.
    #[test]
    fn permutations_and_duplicates_converge(seed in any::<u64>(), n in 1usize..80, k in 1usize..5) {
        let replicas: Vec<u64> = (0..k as u64).map(|i| (i + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        let mut g = Gen::new(seed, replicas);
        let ops = g.ops(n);
        let reference = codec::canonical_bytes(&apply_all(&ops));
        let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0xABCD);
        for _ in 0..4 {
            let perm = shuffled_with_dups(&ops, &mut rng);
            let doc = apply_all(&perm);
            prop_assert_eq!(codec::canonical_bytes(&doc), reference.clone());
        }
    }

    #[test]
    fn apply_is_idempotent(seed in any::<u64>(), n in 1usize..40) {
        let mut g = Gen::new(seed, vec![1, 2, 3]);
        let ops = g.ops(n);
        let mut doc = apply_all(&ops);
        let before = codec::canonical_bytes(&doc);
        for op in &ops {
            prop_assert!(!doc.apply(op), "second apply of {:?} changed state", op.id);
        }
        prop_assert_eq!(codec::canonical_bytes(&doc), before);
    }

    #[test]
    fn snapshot_round_trips(seed in any::<u64>(), n in 0usize..60) {
        let mut g = Gen::new(seed, vec![7, 8]);
        let ops = g.ops(n);
        let doc = apply_all(&ops);
        let bytes = codec::encode_snapshot(&doc);
        let back = codec::decode_snapshot(&bytes).unwrap();
        prop_assert_eq!(&back, &doc);
        prop_assert_eq!(codec::hash(&back), codec::hash(&doc));
    }

    /// Applying in stamp order is the "obvious" LWW semantics; any order must
    /// agree with it (semantic oracle, risk H-R1 class).
    #[test]
    fn matches_sorted_stamp_oracle(seed in any::<u64>(), n in 1usize..60) {
        let mut g = Gen::new(seed, vec![10, 20, 30]);
        let ops = g.ops(n);
        let mut sorted = ops.clone();
        sorted.sort_by_key(|o| o.stamp());
        prop_assert_eq!(codec::hash(&apply_all(&sorted)), codec::hash(&apply_all(&ops)));
    }

    /// H1/H7: ticks strictly increase and wall never decreases, whatever the
    /// physical clock and remote observations do.
    #[test]
    fn hlc_clock_is_monotone(steps in prop::collection::vec((0u64..100, prop::option::of(hlc_strategy())), 1..200)) {
        let mut c = HlcClock::new();
        let mut last = None;
        for (phys, remote) in steps {
            if let Some(r) = remote {
                c.observe(r, phys);
                prop_assert!(c.high_water() >= r);
            }
            let wall_before = c.high_water().wall_ms;
            let t = c.tick(phys);
            prop_assert!(t.wall_ms >= wall_before);
            if let Some(prev) = last {
                prop_assert!(t > prev, "{t:?} !> {prev:?}");
            }
            if let Some(r) = remote {
                prop_assert!(t > r);
            }
            last = Some(t);
        }
    }

    #[test]
    fn frac_between_is_strict(ops in prop::collection::vec((0usize..1000, 0u8..3), 1..150)) {
        let mut keys = vec![fracindex::between(None, None).unwrap()];
        for (pos, mode) in ops {
            let k = match mode {
                0 => fracindex::between(keys.last().map(String::as_str), None).unwrap(),
                1 => fracindex::between(None, Some(&keys[0])).unwrap(),
                _ => {
                    let i = pos % keys.len();
                    let lo = if i == 0 { None } else { Some(keys[i - 1].as_str()) };
                    fracindex::between(lo, Some(&keys[i])).unwrap()
                }
            };
            fracindex::validate(&k).unwrap();
            match mode {
                0 => keys.push(k),
                1 => keys.insert(0, k),
                _ => keys.insert(pos % keys.len(), k),
            }
            for w in keys.windows(2) {
                prop_assert!(w[0] < w[1]);
            }
        }
    }
}

#[test]
fn stamp_order_uses_full_u64_replica() {
    let h = Hlc::new(5, 5);
    let a = Stamp::new(h, ReplicaId((1u64 << 53) + 1));
    let b = Stamp::new(h, ReplicaId(1u64 << 53));
    assert!(a > b);
    let c = Stamp::new(h, ReplicaId(u64::MAX));
    let d = Stamp::new(h, ReplicaId(u64::MAX - 1));
    assert!(c > d);
    assert!(Stamp::new(Hlc::new(5, 6), ReplicaId(0)) > c);
}

#[test]
fn delete_keeps_registers_for_restore() {
    let r1 = 1;
    let r2 = 2;
    let create = Op {
        id: OpId::new(r1, 1),
        hlc: Hlc::new(10, 0),
        kind: OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("x".into(), Value::F64(1.0))],
        },
    };
    let del = Op {
        id: OpId::new(r1, 2),
        hlc: Hlc::new(20, 0),
        kind: OpKind::Delete { object: create.id },
    };
    let upd = Op {
        id: OpId::new(r2, 1),
        hlc: Hlc::new(15, 0),
        kind: OpKind::SetProps {
            object: create.id,
            entries: vec![("x".into(), Value::F64(2.0))],
        },
    };
    let restore = Op {
        id: OpId::new(r2, 2),
        hlc: Hlc::new(30, 0),
        kind: OpKind::Restore { object: create.id },
    };
    let mut d = Document::new();
    for op in [&upd, &del, &create] {
        d.apply(op);
    }
    let o = d.get(&create.id).unwrap();
    assert!(!o.visible());
    assert_eq!(o.get("x"), Some(&Value::F64(2.0)));
    d.apply(&restore);
    assert!(d.get(&create.id).unwrap().visible());
    assert_eq!(d.render_order().len(), 1);
}

#[test]
fn ghost_is_not_rendered_until_created() {
    let set = Op {
        id: OpId::new(2, 1),
        hlc: Hlc::new(5, 0),
        kind: OpKind::SetProps {
            object: OpId::new(1, 1),
            entries: vec![("x".into(), Value::F64(3.0))],
        },
    };
    let mut d = Document::new();
    d.apply(&set);
    assert_eq!(d.len(), 1);
    assert!(d.render_order().is_empty());
    let create = Op {
        id: OpId::new(1, 1),
        hlc: Hlc::new(1, 0),
        kind: OpKind::Create {
            kind: ObjectKind::Text,
            props: vec![("x".into(), Value::F64(0.0))],
        },
    };
    d.apply(&create);
    assert_eq!(d.render_order().len(), 1);
    // The later SetProps wins over the create's initial value.
    assert_eq!(d.get(&create.id).unwrap().get("x"), Some(&Value::F64(3.0)));
}

#[test]
fn validation_rejects_bad_ops() {
    let bad = Op {
        id: OpId::new(1, 1),
        hlc: Hlc::MIN,
        kind: OpKind::Create {
            kind: ObjectKind::Rect,
            props: vec![("x".into(), Value::F64(f64::NAN))],
        },
    };
    assert_eq!(
        bad.validate(),
        Err(ValidationError::NonFiniteFloat("x".into()))
    );
    let bad = Op {
        id: OpId::new(1, 1),
        hlc: Hlc::MIN,
        kind: OpKind::SetProps {
            object: OpId::new(1, 1),
            entries: vec![("héllo".into(), Value::Null)],
        },
    };
    assert!(matches!(bad.validate(), Err(ValidationError::BadKey(_))));
    let bad = Op {
        id: OpId::new(1, 1),
        hlc: Hlc::MIN,
        kind: OpKind::SetProps {
            object: OpId::new(1, 1),
            entries: vec![("z".into(), Value::FracIndex("a00".into()))],
        },
    };
    assert!(matches!(
        bad.validate(),
        Err(ValidationError::BadFracIndex(_))
    ));
}

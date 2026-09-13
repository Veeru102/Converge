//! Every named scenario over a seed band. `SIM_SEEDS` widens the band.

use converge_sim::{run, Scenario, SCENARIOS};

fn seeds() -> u64 {
    std::env::var("SIM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12)
}

#[test]
fn all_scenarios_converge() {
    let mut failures = Vec::new();
    for name in SCENARIOS {
        let scenario = Scenario::named(name).unwrap();
        for seed in 1..=seeds() {
            if let Err((f, trace)) = run(scenario.clone(), seed) {
                let tail: Vec<&String> = trace.iter().rev().take(15).collect();
                failures.push(format!(
                    "{name} seed={seed}: {f}\n   {}",
                    tail.iter()
                        .rev()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("\n   ")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failing runs:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The fault paths actually fire in the scenarios meant to exercise them.
#[test]
fn scenarios_exercise_their_fault_paths() {
    // A path must fire in at least one of the first few seeds.
    let check = |name: &str, f: &dyn Fn(&converge_sim::Stats) -> bool| {
        let mut last = None;
        for seed in 1..=6 {
            let stats = run(Scenario::named(name).unwrap(), seed)
                .unwrap_or_else(|(e, _)| panic!("{name} seed {seed}: {e}"));
            if f(&stats) {
                return;
            }
            last = Some(stats);
        }
        panic!("{name} did not exercise its fault path: {last:?}");
    };
    check("drop_and_dup_fifo", &|s| {
        s.messages_dropped > 0 && s.messages_duplicated > 0 && s.timeouts > 0
    });
    check("disconnect_reconnect", &|s| s.reconnects > 10);
    check("offline_burst", &|s| s.snapshot_catch_ups > 0);
    check("client_crash_store_windows", &|s| s.client_crashes > 0);
    check("skew_ahead", &|s| {
        s.skew_nacks > 0 && s.snapshot_catch_ups > 0
    });
    check("skew_nack_reorder_stress", &|s| {
        s.gap_resumes > 0 && s.clock_jumps > 0
    });
    check("reorder_stress", &|s| s.gap_resumes > 0);
    check("everything", &|s| {
        s.skew_nacks > 0 && s.server_crashes > 0 && s.client_crashes > 0
    });
    check("server_crash_before_durable", &|s| s.server_crashes > 0);
    check("reconnect_eviction_race", &|s| s.superseded > 0);
    check("gap_detection", &|s| s.gap_resumes > 0);
    check("permanent_nack_resync", &|s| s.permanent_nacks > 0);
}

#[test]
fn runs_are_deterministic() {
    let a = run(Scenario::named("everything").unwrap(), 7).unwrap();
    let b = run(Scenario::named("everything").unwrap(), 7).unwrap();
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}

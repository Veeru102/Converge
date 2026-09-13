use converge_sim::{run, Scenario, SCENARIOS};

fn usage() -> ! {
    eprintln!("usage: sim [--scenario NAME|all] [--seed N] [--seeds K] [--clients N] [--steps S] [--dump FILE] [--quiet]");
    eprintln!("scenarios: {}", SCENARIOS.join(", "));
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut scenario = "all".to_string();
    let mut seed = 1u64;
    let mut seeds = 1u64;
    let mut clients: Option<usize> = None;
    let mut steps: Option<usize> = None;
    let mut dump: Option<String> = None;
    let mut quiet = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--scenario" => scenario = args.next().unwrap_or_else(|| usage()),
            "--seed" => {
                seed = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            "--seeds" => {
                seeds = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            "--clients" => clients = args.next().and_then(|s| s.parse().ok()),
            "--steps" => steps = args.next().and_then(|s| s.parse().ok()),
            "--dump" => dump = args.next(),
            "--quiet" => quiet = true,
            _ => usage(),
        }
    }
    let names: Vec<&str> = if scenario == "all" {
        SCENARIOS.to_vec()
    } else {
        vec![scenario.as_str()]
    };
    let mut failures = 0;
    for name in names {
        let Some(mut sc) = Scenario::named(name) else {
            eprintln!("unknown scenario {name}");
            usage();
        };
        if let Some(c) = clients {
            sc.clients = c;
        }
        if let Some(s) = steps {
            sc.steps = s;
        }
        for s in seed..seed + seeds {
            match run(sc.clone(), s) {
                Ok(stats) => {
                    if !quiet {
                        println!(
                            "ok   {name:<28} seed={s:<6} t={}ms authored={} committed={} delivered={} dropped={} dup={} reconnects={} gaps={} snapcatch={} skewnack={} permnack={} superseded={} timeouts={} scrash={} ccrash={}",
                            stats.end_time_ms, stats.ops_authored, stats.ops_committed, stats.messages_delivered, stats.messages_dropped,
                            stats.messages_duplicated, stats.reconnects, stats.gap_resumes, stats.snapshot_catch_ups, stats.skew_nacks,
                            stats.permanent_nacks, stats.superseded, stats.timeouts, stats.server_crashes, stats.client_crashes
                        );
                    }
                }
                Err((f, trace)) => {
                    failures += 1;
                    println!("FAIL {name:<28} seed={s:<6} {f}");
                    if let Some(path) = &dump {
                        let p = format!("{path}.{name}.{s}.txt");
                        std::fs::write(&p, trace.join("\n")).ok();
                        println!("     trace written to {p}");
                    } else {
                        for line in trace
                            .iter()
                            .rev()
                            .take(30)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                        {
                            println!("     {line}");
                        }
                    }
                }
            }
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}

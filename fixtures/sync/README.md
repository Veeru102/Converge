# Sync traces

Per-client recordings of every `ClientSync` call made by the Rust simulator
(`crates/converge-sim`): the input, the outputs the reference engine produced,
and the state hash afterwards. `@converge/sync` replays them in vitest and
must produce identical outputs and hashes step by step.

Regenerate (from the repo root):

    cargo run --release -p converge-sim -- --scenario baseline_no_faults --seed 1 --steps 100 --trace-dir fixtures/sync --quiet
    for sc in drop_and_dup_fifo disconnect_reconnect client_crash_store_windows skew_ahead \
              server_crash_before_durable gap_detection permanent_nack_resync everything; do
      cargo run --release -p converge-sim -- --scenario $sc --seed 3 --steps 100 --trace-dir fixtures/sync --quiet
    done

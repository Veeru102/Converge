# Converge

A local-first multiplayer canvas with a custom CRDT-style synchronization
engine: many users edit one document in real time, keep editing while
offline, and deterministically converge on reconnect. The drawing UI is
intentionally thin; the engineering is in the sync model and the machinery
that proves it converges under hostile networks.

* Design: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
* Risk plan, invariants and what the simulator found:
  [`docs/IMPLEMENTATION_RISKS.md`](docs/IMPLEMENTATION_RISKS.md)

## Layout

| path | what |
|------|------|
| `crates/converge-core` | document engine: ids, HLC, LWW registers, fractional indexing, canonical hash/snapshot |
| `crates/converge-proto` | protocol messages, JSON form, protobuf wire codec |
| `crates/converge-hub` | sans-I/O per-document server state machine |
| `crates/converge-client-sync` | sans-I/O reference client (pending queue, resume, re-timestamping) |
| `crates/converge-sim` | deterministic discrete-event simulator, fault model, named scenarios, invariants |
| `crates/converge-storage` | `Storage` trait: Postgres (sqlx, migrations) and in-memory |
| `crates/converge-server` | axum server: WebSocket sessions, document actors, persister, chaos API, metrics |
| `web/packages/engine` | TypeScript port of the engine, pinned to Rust by `fixtures/engine` |
| `web/packages/protocol` | TS messages + protobuf-es codec, pinned by `fixtures/wire.json` |
| `web/packages/sync` | TS `SyncEngine` (pinned to Rust by `fixtures/sync` traces), IndexedDB store, client driver |
| `web/apps/canvas` | React canvas + debug/chaos panel; Playwright end-to-end tests |
| `proto/` | the protobuf schema |
| `deploy/` | Dockerfile, docker-compose (Postgres, server, Prometheus) |

## Run locally

```sh
# server (in-memory storage; set DATABASE_URL=postgres://… for Postgres)
cargo run -p converge-server

# web app with hot reload, proxied to the server on :8080
cd web && pnpm install && pnpm --filter @converge/canvas dev
# open http://localhost:5173  (a new document is created; share the URL)
```

Or the packaged stack: `docker compose -f deploy/docker-compose.yml up --build`
(app on :8080, Prometheus on :9090).

Useful endpoints: `GET /docs/:id/hash` (durable state hash — the convergence
oracle), `PUT /admin/chaos` (`{"enabled":true,"latency_ms":[5,120],"drop_p":0.1,"dup_p":0.1,"disconnect_every":20}`),
`GET /metrics`.

## Tests

```sh
cargo test --workspace --release                 # engine, hub, client, sim scenarios, storage, server
SIM_SEEDS=200 cargo test --release -p converge-sim
cargo run --release -p converge-sim -- --scenario everything --seed 7   # reproduce any run
cd web && pnpm -r test                           # fixture conformance, trace replay, IndexedDB crash windows
cd web/apps/canvas && pnpm exec playwright test  # builds the app, runs the server, drives 2–3 browsers under chaos
```

Regenerating cross-language fixtures after an engine change:
`cargo test -p converge-core --test fixtures -- --ignored generate`,
`cargo test -p converge-proto --test wire -- --ignored generate`, and the
commands in `fixtures/sync/README.md`.

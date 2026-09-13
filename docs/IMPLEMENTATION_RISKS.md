# Converge — Implementation-Risk Plan (4 highest-risk areas)

## Context

`docs/ARCHITECTURE.md` fixes the v1 design. Before code is written, this plan
audits the four areas where a subtle bug would silently break the one property
the product is built on — *every replica converges* — and turns each into
invariants, concrete tests, and an order of work. It is not a redesign; where
the audit found a real hole, the fix is listed under **Amendments** (§5) as a
small edit to the existing doc.

Notation: `Hn` = HLC invariant, `Dn` = durability, `Pn` = protocol, `In` =
simulator invariant.

---

## 1. HLC semantics & clock-skew handling

### Risks / potential bugs

| # | Risk | Consequence |
|---|------|-------------|
| H-R1 | **Later-counter op accepted, earlier-counter op skew-NACKed** (server `now` advanced between checks, or ops in flight when the NACK arrives). Re-timestamped earlier op gets a fresh HLC > the accepted later op → stale value wins on the same prop. | Wrong final value; all replicas agree, so hash check does *not* catch it. |
| H-R2 | **Two HLCs for one OpId.** Original (old-HLC) copy of a NACKed op delayed in the network, re-timestamped copy accepted; then the old copy is deduped (fine) — *or* the old copy arrives first on a reconnected session, is accepted because server time moved on, and the new copy is deduped. Client holds `V@fresh`, server holds `V@old`. | Permanent hash divergence. |
| H-R3 | Re-timestamping the pending queue out of counter order, or with non-strictly-increasing HLCs. | Own earlier edit beats own later edit. |
| H-R4 | `hlc_high_water` persisted lazily; crash + physical clock stepped backwards (NTP) → new op gets HLC < own earlier op. | Own later edit loses. |
| H-R5 | `logical: u16` overflow under a frozen wall clock (fake clock in tests, or > 65k ops/ms). | Panic or non-monotone HLC. |
| H-R6 | TS compares `ReplicaId` (random u64) as a JS `number` → precision loss above 2⁵³ → tie-break differs from Rust. | Cross-language divergence on `(wall, logical)` ties. |
| H-R7 | Client ≤ 60 s ahead is never NACKed and wins every concurrent conflict for that window. | Bounded unfairness, not divergence — accept, document the bound. |
| H-R8 | Offset from `Welcome.server_time_ms` applied naively lets `wall` move backwards. | Non-monotone HLC. |

### Required invariants

- **H1** Per replica, HLCs are strictly increasing in counter order — across ticks, observes, reloads and re-timestamps.
- **H2** An OpId has exactly one HLC anywhere in the system (client memory, IDB, hub log, Postgres).
- **H3** If op `(r,k)` is skew-rejected, no op `(r,k')`, `k' > k`, is ever accepted with its original HLC. *Enforcement:* a skew NACK terminates the session (`Bye{ClockSkew}`); the server evaluates a whole `Submit` batch against one `now`.
- **H4** Re-timestamping is done at the `Welcome` boundary, before the pending flush, over the *known-unaccepted suffix* of the pending queue (from the first skew-NACKed counter on, plus never-sent ops), in counter order, using the corrected clock. Never per-op mid-stream. Found in implementation: re-stamped ops carry *lower* stamps than the skewed originals, so they cannot be re-applied over the local document — the client must rebuild `doc = snapshot ∪ pending` via a forced snapshot catch-up (the P7 path). Ops that were sent and not NACKed may be on the server and are never re-stamped.
- **H5** Message delivery is connection-scoped: nothing from a closed connection is ever processed (server and client both tag by connection generation). This is what closes H-R2 — same as TCP semantics, and the simulator must model it.
- **H6** `hlc_high_water` is written in the same IDB transaction as any pending op append; on startup `wall = max(physical + offset, high_water)`.
- **H7** `wall` never decreases: `wall = max(wall, physical + offset, observed)`; logical overflow bumps `wall` by 1 and resets `logical`.
- **H8** LWW key compare is `(wall u64, logical u16, replica u64)` with full 64-bit unsigned semantics in both languages (TS: `BigInt` or two-u32 compare).

### Concrete tests

- `core`: proptest — arbitrary interleaving of `tick(phys)` / `observe(remote)` with a physical clock that may jump backwards and freeze; assert H1 and H7; include forced u16 overflow.
- `core`+`engine` fixture: replica ids that differ only below bit 53 and above; identical `(wall, logical)`; assert identical winner and identical hash bytes (H8).
- `client-sync` unit: pending queue `[Create@t1, Set@t2, Set@t3]` re-timestamped → counter order preserved, strictly increasing, same OpIds; local state after re-apply equals state from a fresh replica applying the re-timestamped ops (H3/H4).
- `hub` unit: batch with mixed-skew ops → after first rejection, all later ops in the batch rejected and session closed (H3); second `Hello` from the same replica succeeds.
- Sim scenario `skew_ahead` (client +10 min) and `skew_behind` (client −1 h): converge; assert I4 (one HLC per OpId) online at every delivery, not just at the end.
- Sim scenario `skew_nack_reorder_stress`: skew + non-FIFO connection + dup; asserts H2/H3 via I4 and the "no later op accepted" check.
- Store unit (fake-indexeddb): crash after op append → restart with physical clock 1 h earlier → next op's HLC still > persisted ops (H6).

---

## 2. IndexedDB durability & crash ordering

### Crash windows (local edit path: `apply → IDB pending write → send`; remote path: `Commit → apply → mark acked → debounced snapshot`)

| # | Window | Outcome today | Verdict |
|---|--------|---------------|---------|
| W1 | after in-memory apply, before pending txn commits | edit lost locally, never sent | Acceptable (bounded, ms). UI must not show "saved" before the txn resolves. Counter reuse is safe **only because** the OpId was never transmitted (D1). |
| W2 | pending written, sent, server committed, `Commit`/ack never received | resend on restart → dedup `Ack` → removed | Correct. Needs `Ack` for duplicates and own-op detection in `Welcome.ops`. |
| W3 | `Commit` applied, pending entry deleted, snapshot not yet flushed | restart offline: **own committed edit is missing** until reconnect | Bug (UX regression, not divergence). Fix D2. |
| W4 | `meta.last_seq` written ahead of the snapshot that contains those ops | resume skips ops the client never persisted | **Permanent divergence.** Fix D3. |
| W5 | remote ops applied, snapshot not flushed | resume re-delivers by seq (idempotent) | Correct. |
| W6 | two tabs write `snapshot` with different `seq` (last writer wins) | a lower-seq snapshot overwrites a higher one | Correct after resume, but W3 reappears for the other tab's ops. Fix D4. |
| W7 | orphan flush: tab B submits tab A's ops under A's replica id from B's session | server must accept foreign-replica ops; two live flushers of one queue | Unnecessary complexity + trust surface. Fix D5 (simplify). |
| W8 | IDB write rejects (quota, private mode) | op sent anyway? | Must not send (D1). Surface as "not saved". |
| W9 | tab closed with pending debounce timer | up to 2 s of remote ops + acked-marks unflushed | Flush on `pagehide`/`visibilitychange`; otherwise W5 heals it. |

### Required invariants

- **D1** An op is transmitted only after its pending write (op + `next_counter` + `hlc_high_water`, one transaction) has committed. Corollary: an untransmitted counter may be reused after a crash; a transmitted one never is.
- **D2** A pending entry is deleted only inside the transaction that persists a snapshot taken *after* the op was applied locally. (Since local state always contains local ops, "acked" is sufficient; no seq comparison needed.) Restart state `snapshot ∪ pending` therefore always contains every op the user has made.
- **D3** `last_seq` lives inside the snapshot record (one key, one write). There is no separately-written `last_seq`.
- **D4** Snapshot `seq` in IDB is monotone: the snapshot txn reads the stored seq and aborts the write if the new seq is lower (readwrite txn on the store is serializable).
- **D5** One replica id per live tab, enforced by a Web Lock on `converge:<doc>:replica:<id>`. On startup a tab *adopts* an unlocked replica slot (identity, counter, pending queue) instead of forwarding a foreign replica's ops. No foreign-replica `Submit` exists in v1. (Fallback without Web Locks: always mint a new replica; orphans wait for a browser that has locks.)
- **D6** Client processes inbound messages strictly sequentially through one async queue; no `Commit` is applied while a snapshot decode/write for the same connection is in progress (also P6).

### Concrete tests (vitest + `fake-indexeddb`, `FakeTransport`, a crash harness that discards the in-memory engine and re-hydrates from the same fake IDB at a chosen hook)

- T2.1 crash before pending txn → op absent after restart; `FakeTransport` recorded zero sends for that OpId (D1).
- T2.2 crash after pending txn, before send → resent verbatim after restart (same bytes, same HLC).
- T2.3 crash after send, before `Commit` → resend → server dedup `Ack` → pending emptied at next snapshot (W2, D2).
- T2.4 `Commit` applied, crash before snapshot flush, restart **offline** → op visible (D2). Restart online → hash equals server.
- T2.5 the snapshot write is a single transaction containing `{state, last_seq}` and the pending deletes — assert via fake-indexeddb transaction spy (D2, D3).
- T2.6 two stores (two tabs) racing snapshot writes with seq 100 and 90 → stored seq stays 100 (D4).
- T2.7 model-based property test: random interleaving of `{localEdit, commitOwn, commitRemote, snapshotFlush, crash, restart, resume}` vs. an abstract model; invariant checked after every restart: every op whose pending write resolved is present in `snapshot ∪ pending` and in the engine state.
- T2.8 IDB write rejects → op not sent, status = unsaved, counter not advanced in IDB (D1, W8).
- T2.9 replica-slot adoption: tab A crashes with pending; tab B starts, acquires A's slot, resumes A's counter and flushes A's queue as itself (D5); with a lock held, B mints a new replica.

---

## 3. Duplicate / reordered / delayed / lost messages

Fault model to design against: connections are FIFO while open and may die at any moment; a dead connection loses everything in flight (H5). Dups/reorders arise from *retransmission across connections*. The simulator additionally offers a non-FIFO **stress** mode; under stress the required property is convergence, not efficiency.

### Race conditions (enumerated)

| # | Race | Design status | Required behaviour |
|---|------|---------------|--------------------|
| R1 | Ack/Commit lost → duplicate `Submit` | handled (`SeenSet` → `Ack`) | `Ack{seq: None}` when outside the index window → client marks acked. |
| R2 | `Commit`s arrive with a gap (broadcast `Lagged`, or stress reorder) | **hole** — `last_seq = max(...)` would skip ops forever | **P1**: advance `last_seq` only when `seq == last_seq + 1`; on `seq > last_seq + 1` apply the op (harmless) and trigger resume from `last_seq`; `seq ≤ last_seq` ignore. |
| R3 | Session joins between accept and durable; `Welcome` serves `head_seq` including non-durable ops; worker crashes | **hole** — joiner holds a phantom op / seq ahead of the recovered server | **P2**: the server reveals only durable state: `Welcome.head = durable_seq`, `Welcome.ops ≤ durable_seq`, `Ack` only for durable ops, `Commit` only when durable. |
| R4 | Worker restart rebuilds `SeenSet` from retained ops only; a long-offline client resends an op committed before the retention floor | re-accepted as new | **P3**: re-accepting a byte-identical committed op is harmless end-to-end — state apply idempotent, Postgres insert `ON CONFLICT (doc_id, replica_id, counter) DO NOTHING` treated as durable, no seq allocated if the conflict is detected. Do not rely on `SeenSet` for correctness. |
| R5 | Client reconnects before the server notices the old socket died; §2 says reject `Hello` for a connected replica | **bug** — up to 30 s of `Bye` storms | **P4**: newest `Hello` wins; the old session is evicted with `Bye{Superseded}` and its late messages are dropped (H5). |
| R6 | `Welcome.ops` served from Postgres while pruning runs → non-contiguous range | hole | **P5**: `Welcome.ops` must be contiguous `last_seq+1 ..= durable_head`, verified at build; otherwise fall back to snapshot. |
| R7 | Client flushes pending before processing `Welcome` (needs clock offset + own-op detection) | state machine | **P6**: client sync states `HELLO_SENT → SYNCING` gate the flush; messages processed sequentially. |
| R8 | Permanent `Nack{Malformed|Rejected}` for an op already applied locally | **hole** — client keeps a phantom effect (LWW cannot "undo") | **P7**: permanent NACK ⇒ drop from pending and force a snapshot resync (`Hello{last_seq: 0}` or explicit flag); `doc = snapshot ∪ pending`. |
| R9 | `RateLimited` NACK → retried later with original HLC | correct under LWW (order by HLC, not arrival) but adds a state | Drop the NACK; use bounded-channel backpressure (TCP) in v1. |
| R10 | Partial batch processed when session dies | fine | Client resends all; dedup. Chunk `Submit` to ≤ 256 ops. |
| R11 | Duplicate `Commit` | fine | idempotent apply; P1 ignores `seq ≤ last_seq`. |
| R12 | Presence from an evicted session | — | ignore by session id. |
| R13 | Two sessions for one replica (P4 eviction) with in-flight `Submit` on both | fine | `SeenSet` is per replica, not per session. |

### Required invariants

- **P1** client `last_seq` is contiguous; gaps trigger resume.
- **P2** the server never reveals a seq that is not durable.
- **P3** re-accepting an identical committed op is harmless (state, DB, log).
- **P4** newest `Hello` per replica wins; superseded session is closed.
- **P5** `Welcome.ops` contiguous and ending at `durable_head`.
- **P6** client sync is a strict state machine with sequential message processing; no `Submit` before `Welcome` is applied.
- **P7** permanently rejected op ⇒ snapshot resync.
- **P8** (server) `seq` is assigned in acceptance order, persisted in seq order, broadcast in seq order; `durable_seq` is a prefix of `head_seq`.

### Concrete tests

- `hub` unit: `SeenSet` proptest — random insertion order with duplicates; `contains ⇔ inserted`; contiguous prefix compaction; memory bounded.
- `hub` unit: `Welcome` builder with a `FakeStorage` that returns a range with a hole → snapshot branch chosen (P5).
- `hub` unit: `Hello` for a connected replica → old session receives `Bye{Superseded}`, new session receives `Welcome` (P4).
- `hub` unit: accept N ops, `Durable{up_to = N−3}` → `Welcome` to a new joiner shows `head = N−3` (P2).
- `client-sync` unit: `Commit` sequence `1,2,4` → op 4 applied, `last_seq` stays 2, resume requested; then `Welcome.ops = [3,4]` → `last_seq = 4` (P1).
- `client-sync` unit: `Nack{Malformed}` → resync request emitted, op removed (P7).
- Sim scenarios (FIFO unless noted): `ack_lost_dup_submit`, `gap_detection` (drop one broadcast to one client), `welcome_only_durable` (slow persist + joiner + server crash), `server_crash_recover` (random crash points, upsert path), `reconnect_eviction_race`, `long_offline_snapshot_branch` (pending includes already-committed ops), `partial_batch_session_death`, `permanent_nack_resync`, `reorder_stress` (non-FIFO, dup, drop).

---

## 4. Deterministic simulator (`crates/converge-sim`)

### Minimal architecture

```
Sim {
  now: u64 (virtual ms),
  rng: ChaCha8Rng            // master; each component gets a forked stream at construction
  events: BinaryHeap<(time, insertion_no, Event)>   // total order; never iterate a HashMap
  hub: Hub, storage: FakeStorage { durable_seq, log, persist_latency },
  clients: Vec<RefClient { sync: ClientSync, store: FakeStore, gen: ActionGen, conn: Option<ConnId> }>,
  conns: BTreeMap<ConnId, Conn { client, open, gen, faults: FaultPlan, last_delivery: [u64; 2] }>,
  trace: Vec<TraceEntry>,   // every event applied, for JSON dump / replay
}
Event = Deliver{conn, dir, msg} | ClientAction{client} | Open{client} | Close{conn}
      | StoreWriteDone{client, txn_id} | PersistDone{up_to} | Tick | CrashServer | CrashClient{i} | Stop
FaultPlan { latency: Dist, fifo: bool, drop_p, dup_p, dup_delay: Dist,
            disconnect_at: Vec<u64>, partition: Vec<(u64,u64)>, persist_latency: Dist }
```

Determinism rules: no wall clock, no `HashMap` iteration in any path that emits events or output, all randomness from forked `ChaCha8Rng` streams (one per client, one per connection direction, one for the schedule) so adding a fault type does not perturb unrelated streams; message payloads are round-tripped through the protobuf codec (`encode → decode`) on delivery so the codec is under test too; `FakeStore` models IDB with explicit write completion events so D1/D2 windows are reachable by `CrashClient`.

Connection semantics (mirrors TCP, H5): `fifo=true` ⇒ delivery time `= max(last_delivery, now + latency)`; `Close` discards every `Deliver` for that conn still in the heap (lazy: `Deliver` checks `conn.open && conn.gen`); reconnect creates a new `ConnId` after the client's backoff.

Server crash: drop `hub`, discard pending `PersistDone`, rebuild `Hub` from `FakeStorage` (snapshot + log ≤ `durable_seq`, `SeenSet` from retained log only — exercises P3), close all conns.

Quiescence: no events left except `Tick`, every client connected, every pending queue empty; if not reached by `--max-time`, fail with `DidNotQuiesce` (liveness bug) and dump trace.

CLI: `sim --scenario <name> --seed <u64> --clients N --steps S [--dump trace.json] [--replay trace.json]`. Failures print the seed and scenario; CI runs a seed band per PR (200 seeds × core scenarios) and nightly (5 000 seeds).

### Invariants checked

Online (at every event):
- **I3** hub `seq` contiguous; `durable_seq ≤ head_seq`; no `Commit`/`Ack`/`Welcome` reveals `seq > durable_seq` (P2).
- **I4** a global `OpId → Hlc` map; any second sighting with a different HLC fails immediately (H2).
- **I5** client `last_seq` never exceeds `durable_seq` at the moment of processing (P1/P2).
- **I8** client `last_seq` contiguous (P1) — assert inside `ClientSync` in debug builds.

At quiescence:
- **I1** every client `hash() == hub.hash()`.
- **I2** every op whose `FakeStore` write completed is present in the hub log (count 1 in crash-free scenarios; ≥ 1 with `CrashServer`), and every client `pending` is empty.
- **I6** quiescence reached within `--max-time` (liveness).
- **I7** semantic oracle: recompute final state by sorting the multiset of all accepted ops by `(hlc, replica)` and applying once; must equal hub state — catches engine bugs where all replicas agree on the wrong answer (H-R1 class).

### First named scenarios (build in this order)

1. `baseline_no_faults` (2 clients) → proves harness + I1/I7.
2. `latency_jitter_fifo`.
3. `drop_and_dup_fifo` (p = 0.2 each; retransmission across reconnects).
4. `disconnect_reconnect` (random closes, resume-by-ops path).
5. `offline_burst` (one client offline long with many edits; resume-by-snapshot when past the retained window).
6. `ack_lost_dup_submit`.
7. `client_crash_store_windows` (crash between apply / store write / send; D1–D3).
8. `skew_ahead`, `skew_behind`, `skew_nack_reorder_stress`.
9. `server_crash_before_durable` (P2, P3, recovery).
10. `reconnect_eviction_race` (P4).
11. `gap_detection` (drop one broadcast; P1).
12. `permanent_nack_resync` (P7).
13. `reorder_stress` (non-FIFO + dup + drop; convergence only).
14. `many_clients_soak` (5 clients, 20 k steps, wide seed band).

---

## 5. Amendments to `docs/ARCHITECTURE.md` (small, targeted)

| § | Change |
|---|--------|
| 2 | "server rejects a `Hello` whose replica id is currently connected" → newest `Hello` wins; old session gets `Bye{Superseded}` (P4). |
| 3.2 | Skew handling: per-op NACK closes the session; re-timestamp happens at `Welcome` over the whole pending queue in counter order (H3/H4); add H2 and connection-scoped delivery (H5). |
| 4 | Remove `RateLimited` NACK; rely on backpressure (R9). |
| 5 | Add gap rule (P1) and permanent-NACK resync (P7); `Welcome.head_seq` and `Ack` are durable-only (P2). |
| 5.2 | `head` → `durable_head`; contiguity check (P5). |
| 8 | Pending deleted in the snapshot transaction (D2); `last_seq` stored in the snapshot record (D3); monotone snapshot seq (D4); orphan flush → replica-slot adoption via Web Lock (D5); write high-water with each pending append (H6). |
| 9 | Persister insert is `ON CONFLICT DO NOTHING` and reports durable (P3). |
| 12 | Add I7 semantic oracle and online I3–I5 checks; note FIFO vs stress fault modes. |

Add one risk-mitigation test asset (test infra, not product scope): the Rust sim can dump per-client `(inbound messages, local actions) → (outbound messages, hash)` traces; the TS `SyncEngine` replays them in vitest with a `FakeTransport`. This keeps the two sync-state-machine implementations honest the same way engine fixtures keep the two engines honest.

---

## 6. Recommended implementation order

1. **HLC + LWW compare + fixtures** (Rust and TS) — H1, H7, H8. Half a day; everything else depends on it.
2. **Sans-I/O `Hub` and `ClientSync`** with `SeenSet`, contiguous `last_seq`, durable-only reveal, eviction, skew-closes-session, whole-queue re-timestamp at `Welcome`, permanent-NACK resync — unit tests from §1 and §3.
3. **Simulator skeleton**: virtual clock, forked RNGs, FIFO connections, `FakeStorage`, `FakeStore`; scenarios 1–4; invariants I1, I3, I4, I7 from the first run.
4. **Faults, in scenario order 5–14**, adding I2, I5, I6, I8 as they become meaningful. Fix the protocol *here*, before any browser code exists.
5. **`IndexedDbStore`** implementing D1–D6 with the fake-indexeddb crash harness (T2.1–T2.9); TS `SyncEngine` validated against sim-dumped traces.
6. **Tokio/WebSocket adapter + Postgres persister** (P2/P3/P8 at the adapter boundary), then Playwright chaos e2e.
7. Apply §5 amendments to `ARCHITECTURE.md` as step 0 so the doc and code never disagree.

## Verification

- `cargo test -p converge-core -p converge-hub -p converge-client-sync` green, including proptests at ≥ 1 000 cases.
- `cargo run -p converge-sim -- --scenario all --seeds 200` green; any failure reproducible with `--seed`.
- `pnpm -F @converge/engine -F @converge/sync test` green: fixtures, T2.1–T2.9, sim-trace replays.
- Manual: two browsers + `/admin/chaos` profile `drop=0.2,dup=0.1,latency=200±150`; kill server mid-drag; `window.__converge.hash()` equals `GET /docs/:id/hash` on both after reconnect.

---

## 7. Findings from the simulator (implementation log)

Each of these was surfaced by a failing seed in `crates/converge-sim` and is
now covered by the scenario band (`cargo test -p converge-sim`, widen with
`SIM_SEEDS=200`). Reproduce any run with
`cargo run -p converge-sim -- --scenario <name> --seed <n> --dump trace`.

| # | Finding | Fix | Invariant |
|---|---------|-----|-----------|
| F1 | Re-stamped ops carry *lower* stamps than the skewed originals, so re-applying them over the local document is a LWW no-op; the client kept the skewed stamps and diverged from the server. | A needed re-timestamp forces a snapshot catch-up and rebuilds `doc = snapshot ∪ pending` (the P7 path). | H2, H4 |
| F2 | Re-stamped ops were rewritten in memory only; after a client crash the store restored the *original* stamp and resent it — two HLCs for one OpId. | Re-stamped ops go back through `Persist` and are not transmitted until the store confirms the rewrite. | H2, D1 |
| F3 | Under in-connection reordering, op *k+1* can be accepted before op *k* is skew-rejected, breaking the client's "everything from k on is unaccepted" assumption. | The hub rejects *k* permanently (`Rejected` → resync) when a later counter from that replica is already accepted; a `ClockSkew` NACK therefore always implies H3. | H3 |
| F4 | A duplicated `Hello` produced a second `Welcome` built from a stale `last_seq`. | `Hello` on an already-open session is ignored; `Welcome` outside `HelloSent` is ignored. | P5 |
| F5 | A transport that drops a message without closing (or a lost `Welcome`) left the client waiting forever. | `hello_timeout` and `ack_timeout` on the client trigger the reconnect/resume path. | I6 |
| F6 | Timeouts measured on the wall clock misfire across clock steps (a −20 min step made a 3 s timeout take 20 min). | Client time is `Now { wall_ms, mono_ms }`: HLC from `wall`, timers from `mono` (`performance.now()` in the browser). | I6 |
| F7 | Server-side close must not outrun buffered frames: closing the connection immediately dropped the `Nack`/`Bye` that told the client *why*, causing an endless reconnect loop. Protocol-level: the server always sends `Nack`+`Bye` before closing, and clients must read them. | (Simulator model fix; documented as a server requirement.) | H3 |
| F8 | A static clock skew never reaches the server: the client corrects its offset at the first `Welcome`. Only *mid-session clock jumps* exercise the server-side skew path. | Added `clock_jumps` to the fault model. | H3 |

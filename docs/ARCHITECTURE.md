# Converge — Architecture & Milestone Plan

Converge is a local-first, multiplayer canvas. Many users edit one document in
real time; each user can keep editing while disconnected; on reconnect every
replica reaches the same state without human conflict resolution. The drawing
UI is deliberately thin. The engineering investment is in the synchronization
model, its determinism, and the machinery that proves it converges under
hostile network conditions.

This document fixes the design decisions for v1 and the milestones to reach
it. Section 15 lists the decisions worth confirming before code is written.

---

## 0. Design principles

1. **State is a pure function of the set of accepted operations.**
   Not their arrival order, not how many times each arrived. Every merge rule
   below is commutative, associative and idempotent. This is what makes
   duplication, reordering and offline replay safe, and what makes
   convergence mechanically checkable (apply any permutation of any multiset
   of ops → identical state hash).
2. **The server sequences and persists; it does not arbitrate.**
   The server assigns a monotonically increasing `seq` to each accepted op so
   clients can resume ("give me everything after seq N"), but conflict
   resolution never depends on `seq`. A client's optimistic local state is
   therefore already the final state — there is no rebase step.
3. **Sans-I/O protocol cores.** The server hub and the client sync engine are
   synchronous state machines that take `(message, now)` and return
   `(effects)`. Tokio, WebSockets, IndexedDB and Postgres are thin adapters.
   The same state machines run inside a deterministic discrete-event
   simulator under seeded fault injection.
4. **acked ⇒ durable.** A client removes an op from its pending queue only
   after the server confirms it, and the server confirms only after the op is
   committed to Postgres. This single invariant is what makes worker crash
   recovery correct later without redesign.
5. **Tight v1 scope.** Rectangles and text, property-level LWW, fractional
   z-order, tombstones, offline queues, resume, chaos, convergence tests.
   Groups, connectors, layers, sequence-CRDT text, undo, multi-worker are
   designed-for but not built in v1.

---

## 1. System overview

```
 ┌──────────────── Browser tab (replica) ────────────────┐
 │  React canvas UI                                       │
 │     │ intents (move, recolor, type…)                   │
 │     ▼                                                  │
 │  engine (TS)  ── apply(op) ──►  Document state ──► render
 │     ▲                                                  │
 │  sync (TS, sans-I/O): pending queue, resume, HLC       │
 │     │            │                                     │
 │  IndexedDB store │ Transport (WebSocket | Chaos | Fake) │
 └──────────────────┼─────────────────────────────────────┘
                    │  binary WS frames (protobuf Envelope)
                    ▼
 ┌──────────────── converge-server (Rust, one worker) ────┐
 │  axum: /ws upgrade, /healthz, /metrics, /admin/chaos   │
 │  Session task (per socket) ──mpsc──► Doc actor (per doc)│
 │        ▲                                │              │
 │        └──── broadcast Commit/Presence ◄┘              │
 │                                         │ Persist batch │
 │                                         ▼              │
 │  Persister task ──► Postgres (ops log, snapshots, docs)│
 └────────────────────────────────────────────────────────┘
```

Every replica (each browser tab, and the server itself) runs the same
`Document` merge semantics: Rust in `converge-core`, TypeScript in
`@converge/engine`, kept identical by a shared conformance fixture suite.

---

## 2. Object identity

* **`ReplicaId`** — 64-bit random integer generated once per browser tab
  (`crypto.getRandomValues`), persisted in IndexedDB alongside that tab's
  counter and pending queue. Two live tabs are two replicas; a tab holds a
  Web Lock on its replica id, and a starting tab *adopts* an unlocked slot
  (identity, counter, queue) before minting a new one (§8). Collision
  probability at 2⁻⁶⁴ per pair is accepted. If a `Hello` arrives for a replica
  that already has a live session (a reconnect the server has not yet noticed),
  the **newest `Hello` wins**: the old session is closed with `Bye{Superseded}`
  and its late messages are dropped.
* **`OpId = (replica: ReplicaId, counter: u64)`** — counter is per-replica,
  strictly increasing, persisted so it survives reloads. Globally unique with
  no coordination.
* **`ObjectId = OpId of the op that created the object.`** No separate id
  space, 16 bytes, sortable, and the create op is trivially idempotent because
  the id *is* the op. String form for URLs/debugging: `"<replica hex>:<counter>"`.
* `DocId` — UUIDv7 minted by the server on document creation.
* Object `kind` (`rect`, `text`, later `group`, `connector`) is immutable,
  fixed by the create op.

---

## 3. Document model & conflict semantics

### 3.1 State shape

```
Document {
  objects: BTreeMap<ObjectId, ObjectState>,   // ordered → canonical
}
ObjectState {
  kind:    ObjectKind,                        // immutable
  created: bool,                              // false ⇒ "ghost" (see 3.4)
  deleted: Register<bool>,                    // tombstone flag
  props:   BTreeMap<PropKey, Register<Value>>,
}
Register<T> { value: T, at: Hlc, by: ReplicaId }
Value = Null | Bool | I64 | F64 | Str | Color(u32) | FracIndex(String) | ObjRef(ObjectId)
```

The engine is schema-agnostic: it merges arbitrary `(key, value)` registers.
The UI defines which keys mean what:

| kind | props (v1) |
|------|-----------|
| common | `z: FracIndex` |
| `rect` | `x y w h: F64`, `fill stroke: Color` |
| `text` | `x y: F64`, `text: Str`, `size: F64`, `color: Color` |

Later kinds only add props (`parent: ObjRef`, `from/to: ObjRef`) — no engine
change.

### 3.2 Hybrid Logical Clock

Every op carries `Hlc { wall_ms: u64, logical: u16 }`; combined with the op's
`replica` this forms a total order `(wall_ms, logical, replica)`.

```
tick(physical):       if physical > wall { wall = physical; logical = 0 } else { logical += 1 }
observe(remote, phys): m = max(wall, remote.wall, phys)
                       logical = match m { == wall && == remote.wall → max(l, rl)+1,
                                           == wall → l+1, == remote.wall → rl+1, _ → 0 }
                       wall = m
```

Clients `observe()` every op they receive, so a client with a slow clock is
pulled forward the moment it sees any newer edit. `wall` never decreases
(`wall = max(wall, physical + offset, observed)`); a `logical` overflow bumps
`wall` by 1 and resets `logical`. The client's physical source is
`Date.now() + offset`, with `offset` taken from `Welcome.server_time_ms`.
`hlc_high_water` is written in the same IndexedDB transaction as every pending
op, so HLCs stay strictly increasing per replica across reloads and NTP steps.

**Skew rejection.** The server evaluates a whole `Submit` batch against one
`now` and rejects an op with `wall_ms > now + 60 s` (`Nack::ClockSkew`). A
skew NACK also **closes the session** (`Bye{ClockSkew}`), which guarantees
that no later-counter op from that replica is accepted with its original
timestamp. On the next `Welcome` the client re-timestamps the
*known-unaccepted suffix* of its pending queue (ops from the first skew NACK
on, plus ops never transmitted) — same `OpId`s, fresh `Hlc`s assigned in
counter order using the corrected clock — and only then flushes. Because the
new stamps are *lower* than the skewed originals they cannot be re-applied
over the local state under LWW; the client instead requests a snapshot
catch-up and rebuilds `doc = snapshot ∪ pending` (the same path as P7). Ops
sent earlier and not NACKed may already be on the server, so their stamps
are never touched. Re-timestamping never happens per op mid-stream. Together
with connection-scoped delivery (nothing from a
closed connection is ever processed, on either side) this keeps the invariant
that an `OpId` has exactly one `Hlc` anywhere in the system. Clients up to 60 s
ahead are not rejected and win concurrent conflicts for that window; that is
the accepted unfairness bound.

### 3.3 Merge rules (all commutative, idempotent)

| situation | rule |
|-----------|------|
| Two writes to the same prop | Higher `(hlc, replica)` wins. Ties impossible (replica in key). |
| Concurrent writes to different props of one object | Both survive (property-level LWW). |
| Move `(x,y)` vs concurrent move `(x,y)` | Each op writes all its props with one `Hlc`, so the winner wins *both* x and y — no tearing. |
| Update vs concurrent delete | Object ends deleted; the update is still recorded in its registers, so a later `Restore` shows the latest props. |
| Delete vs concurrent delete / restore | `deleted` is itself an LWW register: highest timestamp wins. |
| Create arrives twice | Idempotent (`ObjectId == OpId`). |
| `SetProp` arrives before `Create` (reordering) | Registers are written into a *ghost* object (`created=false`); ghosts are never rendered or exported. `Create` later flips `created`. |
| Op targets an object that was garbage-collected | Becomes a ghost, never rendered, dropped at next GC. |

Text content in v1 is a single `Str` register (whole-string LWW, coalesced
per keystroke burst). This is the one place where concurrent edits lose work,
and it is an explicit scope cut; §14 describes the upgrade path to a
per-object sequence CRDT without changing the envelope.

### 3.4 Ordering / layer semantics

Z-order uses **fractional indexing**: each object's `z` prop is a base-62
string key; `render order = sort by (z, ObjectId)`. Moving an object emits a
single `SetProps{z: keyBetween(a, b)}`. Concurrent moves that produce equal
keys are disambiguated by `ObjectId`, so the order is total and identical on
every replica. Key length grows with repeated insertion between the same
neighbours; a `rebalance` (rewrite all `z` in a doc) is a normal batch of
`SetProps` and is a v1.5 item.

Layers and groups (post-v1) are the same mechanism one level deeper: a
`parent: ObjRef` register plus `z` *within* that parent. Cycles created by
concurrent re-parenting are resolved deterministically at read time: walk to
root; if the walk revisits a node, the object with the lowest `ObjectId` in
the cycle is treated as parented to root. State stays a pure merge; only the
projection knows about cycles.

### 3.5 Deletion & tombstones

* `Delete{obj}` sets `deleted := true @ hlc`; `Restore{obj}` sets false.
* Tombstoned objects stay in state and in snapshots. They are excluded from
  render, hit-testing and export, but their registers still merge.
* **GC policy (M5):** the server may drop tombstones whose `deleted.at` is
  older than the retention horizon *and* older than the oldest op the server
  still retains for resume. A client that has been offline longer than the
  retained window gets a full snapshot anyway; any op it then replays against
  a GC'd object produces a ghost, which is harmless and itself GC'd. Clients
  never GC on their own; they inherit GC via snapshots.

### 3.6 State hash (convergence oracle)

`hash(doc) = blake3(canonical_bytes(doc))` where canonical bytes are:
objects in `ObjectId` order → `kind, created, deleted{value,wall,logical,replica}`,
then props in key order → `key, value_tag, value_bytes, wall, logical, replica`.
Floats are encoded as IEEE-754 bits; NaN/±Inf are rejected at ingest
(`Nack::Malformed`) so canonical form is unambiguous. Both engines implement
this and the fixture suite checks they agree byte-for-byte.

---

## 4. Operation schema

```protobuf
message Op {
  OpId   id     = 1;          // replica + counter
  Hlc    hlc    = 2;
  oneof kind {
    Create   create   = 10;  // object_kind, initial props (each becomes a register @ hlc)
    SetProps set      = 11;  // object, repeated (key, Value)  — atomic, one hlc
    Delete   delete   = 12;  // object
    Restore  restore  = 13;  // object
    // reserved 14..19 for v2: SetParent, TextSplice, Rebalance
  }
}
```

Rules:
* One `Op` = one atomic intent from one replica. Multi-prop writes share the
  op's `Hlc`. There is no multi-object transaction in v1.
* `Create` carries initial props so that an object never exists in a
  half-initialised state on any replica.
* Ops are immutable once accepted by the server. Pending (unacked) ops may be
  coalesced or re-timestamped by their own replica only.
* Limits enforced by server: ≤ 64 KiB per op, ≤ 64 props per op, ≤ 32 KiB per
  string. There is no rate-limit NACK in v1: overload is handled by bounded
  channels and TCP backpressure, so a retried op never changes order.

**Local coalescing.** A drag emits `SetProps{x,y}` at ~30 Hz. While an op is
still in the *unsent* part of the pending queue, a new op writing the same
`(object, keys)` replaces it in place (keeping the newer `Hlc`). This keeps
the log proportional to user intent, not frame rate, and is safe because
nobody else has seen the replaced op.

---

## 5. Collaboration protocol (wire)

Binary WebSocket frames; each frame is a protobuf `Envelope { oneof msg }`.
`PROTOCOL_VERSION = 1` is checked in `Hello`.

| dir | message | fields | purpose |
|-----|---------|--------|---------|
| C→S | `Hello` | version, doc_id, replica_id, last_seq, user{name,color} | open/resume session |
| S→C | `Welcome` | session_id, server_time_ms, durable_head_seq, `oneof { ops[], snapshot }`, presence[] | catch-up payload (durable state only) |
| S→C | `Bye` | reason ∈ {VersionMismatch, UnknownDoc, Superseded, ClockSkew, SlowConsumer, Restart} | session terminated |
| C→S | `Submit` | ops[] (in counter order) | propose ops |
| S→C | `Commit` | seq, op | broadcast to *every* session incl. originator once durable; originator treats it as ack |
| S→C | `Ack` | op_id, seq? | reply to a duplicate `Submit` of an op that is already durable (`seq` absent when outside the index window) |
| S→C | `Nack` | op_id, reason ∈ {ClockSkew, Malformed, Rejected} | op not accepted; `ClockSkew` also closes the session; `Malformed`/`Rejected` are permanent |
| C→S | `Presence` | cursor{x,y}?, selection[ObjectId], viewport? | ephemeral, ≤ 30 Hz |
| S→C | `PresenceUpdate` | replica, user, cursor, selection | fan-out (coalesced) |
| S→C | `PresenceLeave` | replica | session ended |
| S↔C | `Ping/Pong` | nonce | server pings every 15 s; 2 misses ⇒ close |

### 5.1 Server-side per-document `Hub` (sans-I/O)

```
Hub {
  doc: Document, head_seq: u64,
  log: VecDeque<(seq, Op)>            // last N ops in memory (N = 10_000)
  index: HashMap<OpId, seq>           // for retained window (dup → Ack)
  seen: HashMap<ReplicaId, SeenSet>   // contiguous_max + sparse set
  sessions: HashMap<SessionId, SessionState{replica, user, last_presence}>
}
handle(Submit{ops}, now) → for op:
   validate size/shape/skew → Nack on failure (ClockSkew ⇒ also Bye, stop batch)
   if seen.contains(op.id)  → Ack{op.id, index.get(op.id)} only if durable, else silent (Commit follows)
   else seen.insert; doc.apply(op); seq = ++head_seq; log.push; index.insert
        effects += Persist{seq, op}
        // Commit is emitted by the actor only after Persist is confirmed;
        // durable_seq is always a prefix of head_seq
```

`SeenSet` is an optimisation, not a correctness mechanism: after a worker
restart it is rebuilt from the retained log only, so a byte-identical op can
be re-accepted. That must be harmless end to end — `apply` is idempotent and
the Postgres insert is `ON CONFLICT (doc_id, replica_id, counter) DO NOTHING`
and reported as durable.

`SeenSet` handles gaps (op 6 arriving before op 5 under reordering) exactly:
`contiguous: u64` plus a small `BTreeSet<u64>` of out-of-order counters that
folds back into `contiguous` as gaps fill.

### 5.2 Resume

```
on Hello{last_seq}:
  head  = durable_seq                       // never reveal non-durable ops
  floor = min(seq in memory log) or min(seq in Postgres for doc)
  if last_seq == head:                 Welcome{ops: []}
  elif floor <= last_seq < head:       ops = log[last_seq+1 ..= head] (memory, else Postgres)
                                       if ops is contiguous and ends at head: Welcome{ops}
                                       else (pruned underneath us):           Welcome{snapshot}
  else (too far behind, or ahead):     Welcome{snapshot: {state, seq: head}}
```

Client on `Welcome`:
1. If snapshot: `doc = snapshot; for op in pending: doc.apply(op)`.
   If ops: `for op in ops: doc.apply(op)` (own committed ops found here are
   moved out of pending — this is how a lost ack is healed).
2. `last_seq = durable_head_seq`; `hlc.observe(...)` each op; store offset from
   `server_time_ms`.
3. `Submit` the entire pending queue in counter order. Server dedups; each op
   returns as `Commit` or `Ack`.

Every step is order-independent, so a `Commit` racing a `Welcome` cannot
diverge state — at worst an op is applied twice, which is a no-op.

Two further client rules:
* **Gap rule.** `last_seq` advances only when `Commit.seq == last_seq + 1`.
  A `seq > last_seq + 1` (broadcast lag, or reordering under stress) applies
  the op — harmless — but does not advance `last_seq` and triggers a resume
  from `last_seq`; `seq ≤ last_seq` is ignored.
* **Permanent NACK.** `Malformed`/`Rejected` cannot be undone under LWW, so
  the client drops the op from pending and forces a snapshot resync
  (`Hello{last_seq: 0}`), after which `doc = snapshot ∪ pending`.
* The client processes inbound messages strictly sequentially and never
  flushes pending before `Welcome` has been applied (it needs the clock
  offset and own-op detection).

---

## 6. Presence, cursors, selections

Presence is ephemeral and never enters the op log. Each session sends
`Presence` at most every 33 ms (client-side throttle); the hub keeps only the
latest per session and fans out on a 33 ms tick (coalescing). Selections are
lists of `ObjectId`s — remote selection outlines simply look up local state.
`Welcome` carries the current presence table; `PresenceLeave` is emitted when
a session closes for any reason. Presence bypasses persistence and the
ack-after-durable gate, so it is not slowed by Postgres.

---

## 7. Client / server responsibilities

| concern | client | server |
|---------|--------|--------|
| merge semantics | ✔ (TS engine) | ✔ (Rust engine) — identical by fixtures |
| op authorship, HLC, counters | ✔ | validates skew only |
| conflict resolution | pure merge — no arbitration anywhere | |
| sequencing (`seq`) | | ✔ |
| dedup | own pending queue; idempotent apply | ✔ `SeenSet` per replica |
| durability | IndexedDB snapshot + pending queue | Postgres ops log + snapshots |
| validation | shape only | size, skew, value finiteness (rate via backpressure) |
| presence | throttle & render | coalesce & fan out |
| resume | remembers `last_seq`, resends pending | serves ops-since or snapshot |
| chaos | `ChaosTransport` for demos/tests | `FaultInjector` per session via admin API |

Auth/identity is out of scope for v1: `Hello.user` is self-declared. The
server is structured so a token check slots into the upgrade handler.

---

## 8. Offline reconciliation (client)

IndexedDB, one database per document, via the `idb` wrapper:

| store | key | value |
|-------|-----|-------|
| `replicas` | `replica_id` | `{ next_counter, hlc_high_water, clock_offset_ms }` — one row per replica slot |
| `snapshot` | `"doc"` | `{ seq, state_bytes }` — `seq` lives *inside* the record; there is no separate `last_seq` |
| `pending` | `[replica_id, counter]` | encoded `Op` |

Lifecycle:
1. **Local edit** → op is applied to `doc` immediately, then written to
   `pending` together with the replica row (`next_counter`,
   `hlc_high_water`) in **one transaction**; only after that commits is the
   op handed to the transport. Until then the UI shows it as unsaved, and a
   failed write (quota, private mode) means the op is never sent.
2. **Remote `Commit`** → `doc.apply`, gap rule from §5.2; if it is our own
   op, mark it acked *in memory only*. A debounced task (2 s idle / 500 ops,
   plus `pagehide`) writes, in one transaction: the new `snapshot` record
   (`{seq, state}`), and the deletion of every acked `pending` entry. The
   transaction first reads the stored `seq` and aborts if the new one is
   lower, so the snapshot `seq` is monotone even with several tabs writing.
3. **Startup** → acquire a replica slot: try each `replicas` row's Web Lock
   `converge:<doc>:replica:<id>`, slots with orphaned pending ops first; the
   first one obtained is this tab's identity (its counter and its pending
   queue continue as-is); otherwise mint a new replica. Then `doc = snapshot ∪ pending`; UI is interactive
   before any network activity. Then connect and run §5.2.
4. **Disconnect** → nothing changes for the user except a status pill.
   Reconnect uses exponential backoff (500 ms → 30 s, full jitter).

Why these orderings:
* **pending-before-transport** — an op that was ever sent is always locally
  resendable, and a counter can only be reused after a crash if its op was
  never transmitted.
* **pending deleted only with a newer snapshot** — restart state
  `snapshot ∪ pending` always contains every op the user made, even offline.
* **`seq` inside the snapshot record** — a `last_seq` written ahead of the
  state it describes would make resume skip ops forever.
* **slot adoption instead of orphan forwarding** — no session ever submits
  ops under a replica id it does not own, and two tabs can never share a
  counter.

---

## 9. Persistence & snapshot strategy (server)

```sql
documents (id uuid pk, title text, created_at, head_seq bigint, snapshot_seq bigint)
ops       (doc_id text, seq bigint, replica_id bigint, counter bigint,
           hlc_wall bigint, hlc_logical int, payload bytea, committed_at,
           primary key (doc_id, seq), index (doc_id, replica_id, counter))
           -- no uniqueness on (replica, counter): a byte-identical op may be
           -- re-accepted after a worker restart (P3); apply is idempotent
snapshots (doc_id uuid, seq bigint, payload bytea, created_at, primary key (doc_id, seq))
```

* **Write path**: the doc actor sends accepted ops to a `Persister` task over
  an mpsc; the persister group-commits every 10 ms or 256 ops in a single
  multi-row `INSERT … ON CONFLICT (doc_id, replica_id, counter) DO NOTHING`,
  then signals `Durable{up_to_seq}` back. Only then does the actor advance
  `durable_seq` and broadcast the corresponding `Commit`s; `Welcome` and
  `Ack` are likewise served from `durable_seq`, never `head_seq`. (Config
  flag `ack_mode = durable | eager` for experiments; default `durable`.)
* **Snapshot**: every 1 000 ops or 60 s of activity the actor serialises the
  `Document` (the engine's canonical byte encoding with a small header — the
  same bytes that are hashed, carried as opaque `bytes` on the wire) into
  `snapshots` and advances `documents.snapshot_seq`. Snapshots are written
  from a cheap `clone()` of the state on a blocking thread so the actor keeps
  serving.
* **Pruning**: ops with `seq < snapshot_seq − 50 000` are deleted nightly;
  three most recent snapshots are kept. `floor` in §5.2 comes from the oldest
  retained op.
* **Load**: on first `Hello` for a cold doc, the actor loads the newest
  snapshot and replays ops after it. Replay throughput target ≥ 200k ops/s
  (ops are tiny; it is bounded by decode).
* **Client-visible invariant**: `Commit` seen ⇒ present in Postgres.

---

## 10. WebSocket session lifecycle

```
CONNECTING ──open──► HELLO_SENT ──Welcome──► SYNCING ──pending flushed──► LIVE
     ▲                    │Bye                   │                          │
     │                    ▼                      │ close / ping timeout     │
     └──backoff──── DISCONNECTED ◄───────────────┴──────────────────────────┘
                (editing continues; ops queue in IndexedDB)
```

Server side per socket: a `Session` task owns the sink/stream, decodes frames,
forwards `Submit`/`Presence`/`Pong` to the doc actor, and drains a bounded
outbound mpsc (256 messages). If the outbound queue is full the session is
closed with `Bye{SlowConsumer}`; the client reconnects and resumes by `seq`,
which is strictly cheaper than buffering unboundedly. The actor removes the
session and emits `PresenceLeave` on any close.

---

## 11. Concurrency model (Rust)

* **Runtime**: Tokio multi-threaded. Axum handles HTTP and the WS upgrade.
* **One actor per open document.** A `tokio::task` exclusively owns `Hub`
  (§5.1) and consumes a bounded `mpsc<DocCommand>` (capacity 1 024 →
  natural backpressure). No `Mutex` around document state, ever.
* **Registry**: `DashMap<DocId, DocHandle>` (a clone-able struct holding the
  command sender). First `Hello` for a doc spawns the actor (loading from
  Postgres); an actor with zero sessions for 5 min snapshots and exits, and
  the registry entry is removed under the map's shard lock to avoid a race
  with a concurrent open.
* **Fan-out**: `tokio::sync::broadcast<Arc<ServerMsg>>` per doc (capacity
  4 096). A session that observes `RecvError::Lagged` does not try to
  patch the gap; it re-runs resume from its own `last_seq` through the
  actor — the same code path as reconnect.
* **Persister**: one task per doc actor, mpsc in, `Durable{seq}` out via a
  second mpsc; failures retry with backoff and the actor simply withholds
  `Commit`s meanwhile (clients keep ops pending — no data loss window).
* **Blocking work** (snapshot encode, large Postgres reads) goes through
  `spawn_blocking`.
* **Shutdown**: `SIGTERM` → stop accepting upgrades, ask each actor to flush
  persister and write a final snapshot, close sockets with `Bye{Restart}`.
* **Timing**: everything that needs a clock takes `now: Instant/u64` as an
  argument; the `Hub` never calls `SystemTime::now()` itself.

### 11.1 Multiple workers & failover (post-v1, designed now)

* `doc_leases (doc_id pk, worker_id, expires_at)` in Postgres. A worker
  acquires a lease (`INSERT … ON CONFLICT DO UPDATE WHERE expires_at < now()`)
  before opening a doc and renews it every lease/3.
* A `Hello` arriving at a worker that does not hold the lease gets
  `Bye{Redirect{url}}` (v2) — sticky routing at the load balancer keeps this
  rare. Redis pub/sub or an internal gRPC hop are alternatives if redirects
  prove awkward; the lease table stays either way.
* **Worker crash**: lease expires (10 s), next `Hello` elsewhere acquires it,
  loads snapshot + ops from Postgres. Because acked ⇒ durable, no confirmed
  op is lost; unconfirmed ops are still in clients' pending queues and are
  resent. The convergence property makes duplicate delivery during the
  handover harmless. This scenario is a named simulation test from M4
  onward, so the single-worker build already satisfies the recovery contract.

---

## 12. Deterministic testing strategy

Four layers, each strictly deterministic given a seed.

1. **Engine property tests** (`converge-core`, proptest).
   Generate a random multiset of ops from K virtual replicas (with valid
   HLCs and counters), then apply every op set under several random
   permutations *and with random duplication*; assert equal `hash()`. Also:
   `apply` is idempotent; snapshot → decode → hash round-trips; fractional
   index `keyBetween` is strictly ordered and total.
2. **Cross-implementation conformance fixtures** (`fixtures/*.json`).
   The Rust test suite emits traces: `{ops: [...], expected_hash, expected_state}`
   for hand-picked scenarios (concurrent move, delete-vs-update, ghost
   before create, frac-index ties, HLC ties across replicas) plus 200 random
   seeds. Vitest replays them through the TS engine. CI fails on any
   mismatch. Fixtures are the contract between the two engines.
3. **Protocol simulation** (`converge-sim`).
   A discrete-event simulator with a virtual clock, `ChaCha8Rng(seed)`, one
   `Hub` and N sans-I/O reference clients (`converge-client-sync`, Rust).
   The `Network` applies a fault plan per link:
   `latency ~ dist`, `drop p`, `duplicate p`, `reorder window w`,
   `disconnect [t₀,t₁)`, `partition`, and `server_crash_at t` (drops the Hub,
   rebuilds it from the in-memory "Postgres" up to the last durable seq).
   Connections are FIFO while open (TCP-like) and lose everything in flight
   when closed; a non-FIFO **stress** mode exists, under which the required
   property is convergence, not efficiency. Clients perform random edits
   including while disconnected. Checked online at every event: `seq`
   contiguous and `durable_seq ≤ head_seq`; nothing revealed above
   `durable_seq`; one `Hlc` per `OpId` system-wide; client `last_seq`
   contiguous. The run ends when the network is drained and everyone is
   connected; then `∀ client: client.hash() == hub.hash()`,
   `pending.is_empty()`, every locally-durable op is in the hub log, and a
   **semantic oracle** — all accepted ops sorted by `(hlc, replica)` and
   applied once — equals the hub state, so a bug where every replica agrees
   on the *wrong* answer is still caught.
   `cargo run -p converge-sim -- --seed 42 --clients 5 --steps 20000`
   reproduces any failure; CI runs a fixed seed band nightly and a small band
   on every PR. Traces are dumped as JSON on failure for the visual replayer.
4. **Browser/e2e** (Playwright).
   Server started with `CHAOS_SEED`; three browser contexts edit concurrently
   while the test toggles `/admin/chaos` profiles (latency, 20 % drop,
   duplication, forced disconnects) and one context goes offline via
   `context.setOffline(true)`. Final check: every page's
   `window.__converge.hash()` equals `GET /docs/:id/hash`. IndexedDB paths
   are unit-tested with `fake-indexeddb`.

**Chaos mode in the real server** is the same `FaultPlan` type wrapping each
session's inbound and outbound streams (`FaultInjector<S: Stream>`), seeded
per session from `CHAOS_SEED ⊕ session_id`, so a given message sequence
yields identical faults across runs. The client `ChaosTransport` is the TS
port, exposed in a debug panel (latency slider, drop %, "go offline",
"duplicate next op").

**What determinism requires and how it is enforced:**
no `HashMap` iteration in any code path that produces ordered output
(`BTreeMap` in engine and hub log paths); no wall-clock reads inside state
machines; all randomness from an injected RNG; `f64` values validated
finite; protobuf is *not* treated as canonical bytes — hashes are computed
over the engine's canonical form, never over encoded frames.

---

## 13. Package / module boundaries

```
converge/
├─ Cargo.toml                    # workspace
├─ proto/converge/v1/*.proto     # single source of truth for wire + snapshot format
├─ fixtures/                     # engine conformance vectors (generated by Rust, consumed by TS)
├─ crates/
│  ├─ converge-core          # ids, Hlc, Value, Document, apply(), FracIndex, hash, snapshot codec.
│  │                         #   no_std-friendly, no async, no I/O, forbid(unsafe). proptest suite.
│  ├─ converge-proto         # prost-generated types (protoc vendored at build) + From<> conversions
│  ├─ converge-hub           # sans-I/O per-doc server state machine (Hub, SeenSet, resume, presence)
│  ├─ converge-client-sync   # sans-I/O reference client (pending queue, resume, HLC offset) for sim
│  ├─ converge-sim           # discrete-event simulator, Network/FaultPlan, `sim` CLI, trace dump
│  ├─ converge-storage       # `Storage` trait + Postgres (sqlx, migrations) + InMemory impl
│  └─ converge-server        # axum binary: WS adapter, doc actor + registry, persister,
│                            #   snapshots, FaultInjector + /admin/chaos, metrics, config
├─ web/  (pnpm workspace)
│  ├─ packages/protocol      # protobuf-es generated code + Envelope codec
│  ├─ packages/engine        # TS port of converge-core; fixture conformance tests (vitest)
│  ├─ packages/sync          # sans-I/O SyncEngine; Transport iface; WsTransport, ChaosTransport,
│  │                         #   FakeTransport; IndexedDbStore (idb) + MemoryStore
│  └─ apps/canvas            # React + Vite: SVG canvas, selection/drag, presence layer,
│                            #   debug panel (hash, pending, chaos controls, replica id)
├─ deploy/  docker-compose.yml (postgres, server, web, prometheus), Dockerfiles, prometheus.yml
└─ docs/    this file, PROTOCOL.md (generated tables), TESTING.md
```

Dependency direction is strictly downward: `core ← proto ← hub ← server`,
`core ← client-sync ← sim`, and on the web `protocol ← engine ← sync ← canvas`.
Nothing above `core`/`engine` may reimplement a merge rule.

---

## 14. Custom-built vs delegated

| custom (the product) | delegated (commodity) |
|----------------------|-----------------------|
| LWW register map, tombstones, ghosts, `apply()` | Tokio, Axum, tungstenite (via axum), tower |
| HLC + skew handling | prost / protobuf-es codegen (`protoc-bin-vendored`, `@bufbuild/buf`) |
| Fractional indexing (both languages, one spec) | sqlx + migrations, `deadpool`/sqlx pool |
| Canonical state hash | blake3 (Rust), `@noble/hashes` blake3 (TS) |
| `Hub` sequencing, `SeenSet`, resume | `metrics` + `metrics-exporter-prometheus`, `tracing` |
| Client `SyncEngine`, pending queue, coalescing | `idb` (IndexedDB wrapper), Web Locks API |
| Discrete-event simulator, `FaultPlan`, `FaultInjector` | proptest, `rand_chacha`, vitest, Playwright, `fake-indexeddb` |
| Snapshot encoding *layout* (as `.proto`) | React, Vite, zustand (UI-only state), pnpm |

Why not Yjs/Automerge: the point is owning the semantics (property-level LWW
with atomic multi-prop writes, ghosts, fractional z-order, deterministic GC)
and being able to simulate the *whole* protocol in one process. Why not a
Rust→WASM engine in the browser: one implementation would remove the
conformance-fixture burden, but costs wasm build plumbing, an opaque state
boundary for React, and harder in-browser debugging; the engine is ~600
lines per language and fixtures make drift a CI failure. Revisit if a
sequence-CRDT for text lands (that is where duplication gets expensive).

Encoding: **Protobuf** (compact varints, schema is the contract for wire,
IndexedDB and Postgres payloads, evolution rules are explicit). MessagePack
via serde would cut codegen friction; it is the fallback if the proto
pipeline proves painful in the first milestone.

---

## 15. Decisions to confirm

1. **Conflict order = HLC, not server arrival order.** Gives "latest edit
   wins" semantics that behave sensibly for offline replicas and removes any
   rebase step. Cost: the skew-NACK path in §3.2. (Alternative: Figma-style
   server-order LWW — simpler clocks, but stale offline edits overwrite fresh
   online ones on reconnect.)
2. **Text = whole-string LWW in v1.** Sequence-CRDT text is a v2 op kind.
3. **TS engine + conformance fixtures** rather than Rust→WASM.
4. **Protobuf** over MessagePack.
5. **acked ⇒ durable** (group-commit before broadcast) as the default ack mode.

---

## 16. Milestones

Each milestone ends with a demo and a green CI; nothing in a later milestone
changes the wire format of an earlier one without a `PROTOCOL_VERSION` bump.

### M0 — Skeleton (foundation)
* Cargo workspace with the seven crates (empty but compiling), pnpm workspace
  with four packages, proto pipeline generating both sides, `fixtures/`
  dir, CI (fmt, clippy `-D warnings`, test, vitest, buf lint), docker-compose
  with Postgres + Prometheus, `converge-server` serving `/healthz`,
  `/metrics` and echoing a `Hello`/`Welcome`.
* Exit: `cargo test && pnpm test` green; `docker compose up` brings up the
  stack (Docker not required for local dev — a local Postgres URL works).

### M1 — Engine (both languages)
* `converge-core`: ids, `Hlc`, `Value`, `Document::apply`, ghosts,
  tombstones, `FracIndex::between`, canonical hash, snapshot codec.
* proptest convergence suite; fixture generator (`cargo test --features gen-fixtures`).
* `@converge/engine`: port + vitest conformance run over fixtures.
* Exit: 10 000 random permutation/duplication trials converge; every fixture
  passes in TS; hash agrees byte-for-byte.

### M2 — Realtime server + first UI
* `converge-hub` (`Submit`/`Commit`/`Ack`/`Nack`, `SeenSet`, presence
  coalescing, in-memory log + resume-by-ops), `converge-storage` (Postgres
  ops/snapshots, in-memory impl), `converge-server` (doc actor, session
  task, persister with durable acks, periodic snapshots, cold load,
  metrics).
* `@converge/sync` with `WsTransport` and `MemoryStore`; `apps/canvas`
  with rectangles + text, drag, resize, colour, z-order, cursors and
  selection outlines, hash/pending debug pill.
* Exit: two browsers edit the same doc live; kill and restart the server;
  both browsers reconnect, resume by `seq` and hashes match `/docs/:id/hash`.

### M3 — Offline & reconnect
* `IndexedDbStore` (meta/snapshot/pending), pending-before-send ordering,
  local coalescing, startup from local snapshot, orphan-queue flush via Web
  Locks, clock-offset + `ClockSkew` re-timestamp path, backoff reconnect,
  snapshot-resume branch (client behind the retained window).
* Exit: edit offline for minutes in two tabs, reconnect in either order,
  converge; reload mid-drag loses nothing; `fake-indexeddb` unit suite.

### M4 — Deterministic simulation & chaos
* `converge-client-sync` reference client; `converge-sim` with `Network`,
  `FaultPlan`, virtual clock, seeded RNG, quiescence detection, JSON trace
  dump, `sim` CLI; named scenarios: reorder-before-create, dup ack loss,
  partition heal, worker crash + rebuild from durable log, long offline →
  snapshot resume.
* `FaultInjector` in the server + `/admin/chaos` API; `ChaosTransport` and
  debug panel in the client; Playwright suite under chaos profiles.
* Exit: 1 000 seeds × 5 clients converge in CI; a chaos e2e run converges;
  any failing seed reproduces from the CLI.

### M5 — Hardening & operations
* Op pruning, tombstone GC horizon, snapshot retention, z-key rebalance,
  rate limits, slow-consumer eviction, backpressure metrics, graceful
  shutdown, Prometheus dashboards (ops/s, apply/persist latency, resume kind,
  sessions per doc, chaos counters), load test (50 sessions × 20 ops/s on one
  doc).
* Exit: p99 submit→commit < 50 ms locally with durable acks; 24 h soak with
  chaos enabled shows no divergence.

### M6 — Post-v1 (designed, not scheduled)
* `SetParent` + groups/layers with deterministic cycle projection;
  connectors (`from/to: ObjRef`, dangling refs render nothing);
  sequence-CRDT text (`TextSplice`) behind a new op kind; local undo/redo
  as inverse ops with fresh HLCs; multi-worker leases, redirect, failover
  drills in the simulator; auth.

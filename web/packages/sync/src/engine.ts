/**
 * Sans-I/O client sync engine — a faithful port of `crates/converge-client-sync`.
 * The Rust version is the reference; `fixtures/sync` traces pin this port to it.
 */
import {
  Document,
  HlcClock,
  HLC_MIN,
  compareHlc,
  decodeSnapshot,
  encodeSnapshot,
  hash,
  type Hlc,
  type Op,
  type OpId,
  type OpKind,
  type ReplicaId,
} from "@converge/engine";
import {
  MAX_SUBMIT_OPS,
  PROTOCOL_VERSION,
  type CatchUp,
  type ClientMsg,
  type NackReason,
  type PresenceState,
  type ServerMsg,
  type UserInfo,
} from "@converge/protocol";

/** `wall` feeds the HLC and may jump; `mono` is monotonic and drives timeouts. */
export interface Now {
  wallMs: bigint;
  monoMs: number;
}

export type Output =
  { type: "send"; msg: ClientMsg } | { type: "persist"; op: Op } | { type: "reconnect" };

export type ConnState = "disconnected" | "hello_sent" | "live";

interface Pending {
  op: Op;
  durable: boolean;
  sent: boolean;
  acked: boolean;
  sentAt: number | null;
}

export interface SnapshotWrite {
  bytes: Uint8Array;
  seq: bigint;
  highWater: Hlc;
  acked: bigint[];
}

export interface ReplicaRow {
  nextCounter: bigint;
  highWater: Hlc;
  clockOffsetMs: bigint;
}

export interface SyncConfig {
  retimestampMarginMs: bigint;
  helloTimeoutMs: number;
  ackTimeoutMs: number;
}

export const DEFAULT_SYNC_CONFIG: SyncConfig = {
  retimestampMarginMs: 30_000n,
  helloTimeoutMs: 5_000,
  ackTimeoutMs: 10_000,
};

function maxHlc(doc: Document): Hlc {
  let m = HLC_MIN;
  for (const [, obj] of doc.entries()) {
    if (compareHlc(obj.deleted.at.hlc, m) > 0) m = obj.deleted.at.hlc;
    for (const r of obj.props.values()) if (compareHlc(r.at.hlc, m) > 0) m = r.at.hlc;
  }
  return m;
}

function bigMax(a: bigint, b: bigint): bigint {
  return a > b ? a : b;
}

export class SyncEngine {
  private doc = new Document();
  private clock = new HlcClock();
  private nextCounter = 1n;
  private readonly pending = new Map<bigint, Pending>();
  private lastSeq = 0n;
  private state: ConnState = "disconnected";
  private gen = 0;
  private offsetMs = 0n;
  private firstSkewNack: bigint | null = null;
  private wantSnapshot = false;
  private resyncPending = false;
  private helloSentAt: number | null = null;

  constructor(
    readonly docId: string,
    readonly replica: ReplicaId,
    readonly user: UserInfo,
    readonly cfg: SyncConfig = DEFAULT_SYNC_CONFIG,
  ) {}

  /** Rebuild from the local store. Pending ops are treated as possibly sent. */
  static restore(
    docId: string,
    replica: ReplicaId,
    user: UserInfo,
    cfg: SyncConfig,
    row: ReplicaRow,
    snapshot: { bytes: Uint8Array; seq: bigint } | null,
    pending: Op[],
  ): SyncEngine {
    const c = new SyncEngine(docId, replica, user, cfg);
    c.nextCounter = row.nextCounter > 1n ? row.nextCounter : 1n;
    c.clock = new HlcClock(row.highWater);
    c.offsetMs = row.clockOffsetMs;
    if (snapshot) {
      c.doc = decodeSnapshot(snapshot.bytes);
      c.lastSeq = snapshot.seq;
    }
    const sorted = [...pending].sort((a, b) =>
      a.id.counter < b.id.counter ? -1 : a.id.counter > b.id.counter ? 1 : 0,
    );
    for (const op of sorted) {
      if (op.id.replica !== replica) throw new Error("pending op from another replica");
      c.doc.apply(op);
      c.clock.observe(op.hlc, 0n);
      c.pending.set(op.id.counter, { op, durable: true, sent: true, acked: false, sentAt: null });
    }
    return c;
  }

  // ----- accessors -----

  get document(): Document {
    return this.doc;
  }
  hash(): Uint8Array {
    return hash(this.doc);
  }
  get lastSeqValue(): bigint {
    return this.lastSeq;
  }
  get connState(): ConnState {
    return this.state;
  }
  get generation(): number {
    return this.gen;
  }
  get pendingCount(): number {
    return this.pending.size;
  }
  unackedCount(): number {
    let n = 0;
    for (const p of this.pending.values()) if (!p.acked) n++;
    return n;
  }
  pendingDebug(): Array<[bigint, boolean, boolean, boolean]> {
    return this.sortedPending().map((p) => [p.op.id.counter, p.durable, p.sent, p.acked]);
  }
  replicaRow(): ReplicaRow {
    return {
      nextCounter: this.nextCounter,
      highWater: this.clock.highWater(),
      clockOffsetMs: this.offsetMs,
    };
  }
  private serverNow(wallMs: bigint): bigint {
    const v = wallMs + this.offsetMs;
    return v < 0n ? 0n : v;
  }
  private sortedPending(): Pending[] {
    return [...this.pending.values()].sort((a, b) =>
      a.op.id.counter < b.op.id.counter ? -1 : a.op.id.counter > b.op.id.counter ? 1 : 0,
    );
  }

  // ----- connection lifecycle -----

  connected(now: Now, out: Output[]): number {
    this.gen += 1;
    this.state = "hello_sent";
    this.helloSentAt = now.monoMs;
    for (const p of this.pending.values()) p.sentAt = null;
    out.push({
      type: "send",
      msg: {
        type: "hello",
        hello: {
          version: PROTOCOL_VERSION,
          doc: this.docId,
          replica: this.replica,
          lastSeq: this.lastSeq,
          wantSnapshot: this.wantSnapshot,
          user: this.user,
        },
      },
    });
    return this.gen;
  }

  disconnected(): void {
    this.state = "disconnected";
    this.helloSentAt = null;
  }

  tick(now: Now, out: Output[]): void {
    const nowMs = now.monoMs;
    switch (this.state) {
      case "hello_sent":
        if (this.helloSentAt !== null && nowMs - this.helloSentAt >= this.cfg.helloTimeoutMs) {
          this.state = "disconnected";
          out.push({ type: "reconnect" });
        }
        break;
      case "live": {
        let stale = false;
        for (const p of this.pending.values()) {
          if (!p.acked && p.sentAt !== null && nowMs - p.sentAt >= this.cfg.ackTimeoutMs)
            stale = true;
        }
        if (stale) {
          this.state = "disconnected";
          out.push({ type: "reconnect" });
        }
        break;
      }
      case "disconnected":
        break;
    }
  }

  nextDeadline(): number | null {
    switch (this.state) {
      case "hello_sent":
        return this.helloSentAt === null ? null : this.helloSentAt + this.cfg.helloTimeoutMs;
      case "live": {
        let min: number | null = null;
        for (const p of this.pending.values()) {
          if (!p.acked && p.sentAt !== null && (min === null || p.sentAt < min)) min = p.sentAt;
        }
        return min === null ? null : min + this.cfg.ackTimeoutMs;
      }
      case "disconnected":
        return null;
    }
  }

  // ----- local edits and store -----

  edit(kind: OpKind, now: Now, out: Output[]): Op {
    const hlc = this.clock.tick(this.serverNow(now.wallMs));
    const id: OpId = { replica: this.replica, counter: this.nextCounter };
    this.nextCounter += 1n;
    const op: Op = { id, hlc, kind };
    this.doc.apply(op);
    this.pending.set(id.counter, { op, durable: false, sent: false, acked: false, sentAt: null });
    out.push({ type: "persist", op });
    return op;
  }

  persisted(counter: bigint, now: Now, out: Output[]): void {
    const p = this.pending.get(counter);
    if (p) p.durable = true;
    if (this.state === "live") this.flush(now.monoMs, out);
  }

  snapshot(): SnapshotWrite {
    return {
      bytes: encodeSnapshot(this.doc),
      seq: this.lastSeq,
      highWater: this.clock.highWater(),
      acked: this.sortedPending()
        .filter((p) => p.acked)
        .map((p) => p.op.id.counter),
    };
  }

  snapshotPersisted(acked: bigint[]): void {
    for (const c of acked) {
      const p = this.pending.get(c);
      if (p?.acked) this.pending.delete(c);
    }
  }

  private flush(monoMs: number, out: Output[]): void {
    let batch: Op[] = [];
    for (const p of this.sortedPending()) {
      if (p.acked) continue;
      if (!p.durable) break;
      if (!p.sent || this.resyncPending) {
        p.sent = true;
        p.sentAt = monoMs;
        batch.push(p.op);
      }
      if (batch.length === MAX_SUBMIT_OPS) {
        out.push({ type: "send", msg: { type: "submit", ops: batch } });
        batch = [];
      }
    }
    if (batch.length > 0) out.push({ type: "send", msg: { type: "submit", ops: batch } });
    this.resyncPending = false;
  }

  presence(state: PresenceState, out: Output[]): void {
    if (this.state === "live") out.push({ type: "send", msg: { type: "presence", state } });
  }

  // ----- inbound -----

  message(gen: number, msg: ServerMsg, now: Now, out: Output[]): void {
    if (gen !== this.gen || this.state === "disconnected") return;
    switch (msg.type) {
      case "welcome":
        if (this.state !== "hello_sent") return; // duplicate
        this.welcome(msg.serverTimeMs, msg.durableHeadSeq, msg.catchUp, now, out);
        return;
      case "commit":
        this.commit(msg.seq, msg.op, now, out);
        return;
      case "ack": {
        if (msg.opId.replica === this.replica) {
          const p = this.pending.get(msg.opId.counter);
          if (p) p.acked = true;
        }
        return;
      }
      case "nack":
        this.nack(msg.opId.counter, msg.reason, out);
        return;
      case "bye":
        this.state = "disconnected";
        return;
      case "presence_update":
      case "presence_leave":
        return;
      case "ping":
        out.push({ type: "send", msg: { type: "pong", nonce: msg.nonce } });
        return;
    }
  }

  private welcome(
    serverTimeMs: bigint,
    durableHead: bigint,
    catchUp: CatchUp,
    now: Now,
    out: Output[],
  ): void {
    this.offsetMs = serverTimeMs - now.wallMs;
    const serverNow = this.serverNow(now.wallMs);
    if (catchUp.kind === "ops") {
      for (const { seq, op } of catchUp.ops) {
        this.absorb(op, serverNow);
        this.lastSeq = seq;
      }
      void durableHead;
      if (this.retimestampNeeded(serverNow)) {
        this.wantSnapshot = true;
        this.state = "disconnected";
        out.push({ type: "reconnect" });
        return;
      }
      this.firstSkewNack = null;
    } else {
      const doc = decodeSnapshot(catchUp.bytes);
      this.clock.observe(maxHlc(doc), serverNow);
      if (this.retimestampNeeded(serverNow)) this.retimestamp(doc, serverNow, out);
      this.firstSkewNack = null;
      for (const p of this.sortedPending()) doc.apply(p.op);
      this.doc = doc;
      this.lastSeq = catchUp.seq;
    }
    this.wantSnapshot = false;
    this.state = "live";
    this.helloSentAt = null;
    this.resyncPending = true;
    this.flush(now.monoMs, out);
  }

  private retimestampStart(): bigint | null {
    let firstUnsent: bigint | null = null;
    for (const p of this.sortedPending()) {
      if (!p.sent) {
        firstUnsent = p.op.id.counter;
        break;
      }
    }
    if (this.firstSkewNack !== null && firstUnsent !== null)
      return this.firstSkewNack < firstUnsent ? this.firstSkewNack : firstUnsent;
    return this.firstSkewNack ?? firstUnsent;
  }

  private retimestampNeeded(serverNow: bigint): boolean {
    const start = this.retimestampStart();
    if (start === null) return false;
    const limit = serverNow + this.cfg.retimestampMarginMs;
    for (const p of this.pending.values()) {
      if (p.op.id.counter >= start && p.op.hlc.wall > limit) return true;
    }
    return false;
  }

  private retimestamp(base: Document, serverNow: bigint, out: Output[]): void {
    const start = this.retimestampStart();
    if (start === null) return;
    let floor = maxHlc(base);
    const sn: Hlc = { wall: serverNow, logical: 0 };
    if (compareHlc(sn, floor) > 0) floor = sn;
    const sorted = this.sortedPending();
    for (const p of sorted)
      if (p.op.id.counter < start && compareHlc(p.op.hlc, floor) > 0) floor = p.op.hlc;
    this.clock = new HlcClock(floor);
    for (const p of sorted) {
      if (p.op.id.counter < start) continue;
      const hlc = this.clock.tick(serverNow);
      if (p.acked) throw new Error("acked op in re-timestamp suffix (H3)");
      p.op = { ...p.op, hlc };
      p.sent = false;
      p.durable = false;
      out.push({ type: "persist", op: p.op });
    }
  }

  private absorb(op: Op, serverNow: bigint): void {
    this.clock.observe(op.hlc, serverNow);
    if (op.id.replica === this.replica) {
      const p = this.pending.get(op.id.counter);
      if (p) p.acked = true;
    }
    this.doc.apply(op);
  }

  private commit(seq: bigint, op: Op, now: Now, out: Output[]): void {
    if (seq <= this.lastSeq) return;
    this.absorb(op, this.serverNow(now.wallMs));
    if (seq === this.lastSeq + 1n) {
      this.lastSeq = seq;
    } else {
      this.state = "disconnected";
      out.push({ type: "reconnect" });
    }
  }

  private nack(counter: bigint, reason: NackReason, out: Output[]): void {
    if (reason === "clock_skew") {
      this.firstSkewNack =
        this.firstSkewNack === null || counter < this.firstSkewNack ? counter : this.firstSkewNack;
    } else {
      this.pending.delete(counter);
      this.wantSnapshot = true;
      this.state = "disconnected";
      out.push({ type: "reconnect" });
    }
  }

  /** @internal for tests */
  static _bigMax = bigMax;
}

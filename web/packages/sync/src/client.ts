/**
 * Client driver: wires the sans-I/O SyncEngine to a Store, a Transport and
 * timers. All engine interaction runs through one async queue so that store
 * awaits never interleave with message handling (D6/P6).
 */
import { Document, hex, type Op, type OpKind, type ReplicaId } from "@converge/engine";
import type { PresenceEntry, PresenceState, ServerMsg, UserInfo } from "@converge/protocol";
import { DEFAULT_SYNC_CONFIG, SyncEngine, type ConnState, type Now, type Output, type SyncConfig } from "./engine.js";
import type { ReplicaSlot, Store } from "./store.js";
import type { Connection, Transport } from "./transport.js";

export interface Clock {
  wallMs(): bigint;
  monoMs(): number;
}

export const systemClock: Clock = {
  wallMs: () => BigInt(Date.now()),
  monoMs: () => (typeof performance !== "undefined" ? performance.now() : Number(process.hrtime.bigint() / 1_000_000n)),
};

export interface ClientOptions {
  docId: string;
  user: UserInfo;
  store: Store;
  transport: Transport;
  clock?: Clock;
  config?: SyncConfig;
  /** Snapshot after this many local/remote ops or this much idle time. */
  snapshotEveryOps?: number;
  snapshotIdleMs?: number;
  reconnectBackoffMs?: [number, number];
  mintReplica?: () => ReplicaId;
  /** Test hook: called with the timer id when the client schedules work. */
  setTimeout?: typeof setTimeout;
  clearTimeout?: typeof clearTimeout;
}

export interface ClientStatus {
  state: ConnState;
  replica: ReplicaId;
  lastSeq: bigint;
  pending: number;
  unacked: number;
  /** Ops authored but whose local write has not completed yet. */
  unsaved: number;
}

export function randomReplicaId(): ReplicaId {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  let v = 0n;
  for (const b of bytes) v = (v << 8n) | BigInt(b);
  return v | 1n;
}

type Listener<T> = (v: T) => void;

export class Client {
  private engine!: SyncEngine;
  private slot!: ReplicaSlot;
  private conn: Connection | null = null;
  private gen = 0;
  private queue: Promise<void> = Promise.resolve();
  private stopped = false;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private tickTimer: ReturnType<typeof setTimeout> | null = null;
  private snapshotTimer: ReturnType<typeof setTimeout> | null = null;
  private opsSinceSnapshot = 0;
  private snapshotInFlight = false;
  private unsaved = 0;
  private backoffAttempt = 0;
  private readonly clock: Clock;
  private readonly cfg: SyncConfig;
  private readonly presenceTable = new Map<string, PresenceEntry>();
  private changeListeners: Listener<Document>[] = [];
  private statusListeners: Listener<ClientStatus>[] = [];
  private presenceListeners: Listener<PresenceEntry[]>[] = [];
  private readonly setTimer: typeof setTimeout;
  private readonly clearTimer: typeof clearTimeout;

  constructor(private readonly opts: ClientOptions) {
    this.clock = opts.clock ?? systemClock;
    this.cfg = opts.config ?? DEFAULT_SYNC_CONFIG;
    this.setTimer = opts.setTimeout ?? setTimeout;
    this.clearTimer = opts.clearTimeout ?? clearTimeout;
  }

  // ----- lifecycle -----

  /** Load local state (usable offline at once), then start connecting. */
  async start(): Promise<void> {
    this.slot = await this.opts.store.acquireReplica(this.opts.mintReplica ?? randomReplicaId);
    this.engine = SyncEngine.restore(
      this.opts.docId, this.slot.replica, this.opts.user, this.cfg, this.slot.row, this.slot.snapshot, this.slot.pending,
    );
    this.emitChange();
    this.connect();
  }

  async stop(): Promise<void> {
    this.stopped = true;
    for (const t of [this.reconnectTimer, this.tickTimer, this.snapshotTimer]) if (t) this.clearTimer(t);
    this.conn?.close();
    this.conn = null;
    await this.flush();
    this.slot?.release();
  }

  /** Wait for queued work (store writes) to finish. */
  async idle(): Promise<void> {
    await this.queue;
  }

  /**
   * Test hook: behave like a tab that died — no flush, no clean close. The
   * replica lock is released the way the browser would release it.
   */
  simulateCrash(): void {
    this.stopped = true;
    for (const t of [this.reconnectTimer, this.tickTimer, this.snapshotTimer]) if (t) this.clearTimer(t);
    const c = this.conn;
    this.conn = null;
    c?.close();
    this.slot?.release();
  }

  // ----- public API -----

  get document(): Document {
    return this.engine.document;
  }
  get replica(): ReplicaId {
    return this.engine.replica;
  }
  hashHex(): string {
    return hex(this.engine.hash());
  }
  status(): ClientStatus {
    return {
      state: this.engine.connState, replica: this.engine.replica, lastSeq: this.engine.lastSeqValue,
      pending: this.engine.pendingCount, unacked: this.engine.unackedCount(), unsaved: this.unsaved,
    };
  }
  onChange(l: Listener<Document>): () => void {
    this.changeListeners.push(l);
    return () => (this.changeListeners = this.changeListeners.filter((x) => x !== l));
  }
  onStatus(l: Listener<ClientStatus>): () => void {
    this.statusListeners.push(l);
    return () => (this.statusListeners = this.statusListeners.filter((x) => x !== l));
  }
  onPresence(l: Listener<PresenceEntry[]>): () => void {
    this.presenceListeners.push(l);
    return () => (this.presenceListeners = this.presenceListeners.filter((x) => x !== l));
  }
  presence(): PresenceEntry[] {
    return [...this.presenceTable.values()];
  }

  /** Author an op. Applied locally at once; sent after the local write. */
  edit(kind: OpKind): Op {
    const out: Output[] = [];
    const op = this.engine.edit(kind, this.now(), out);
    this.opsSinceSnapshot++;
    this.emitChange();
    this.handle(out);
    return op;
  }

  sendPresence(state: PresenceState): void {
    const out: Output[] = [];
    this.engine.presence(state, out);
    this.handle(out);
  }

  /** Write a snapshot now (D2: also drops acked pending entries). */
  flush(): Promise<void> {
    return this.enqueue(() => this.writeSnapshot());
  }

  /** Drop the connection (tests / offline toggle). */
  disconnect(): void {
    if (this.conn) {
      const c = this.conn;
      this.conn = null;
      c.close();
      this.lostConnection();
    }
  }

  reconnectNow(): void {
    if (this.reconnectTimer) {
      this.clearTimer(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.connect();
  }

  // ----- internals -----

  private now(): Now {
    return { wallMs: this.clock.wallMs(), monoMs: this.clock.monoMs() };
  }

  private enqueue(fn: () => Promise<void> | void): Promise<void> {
    this.queue = this.queue.then(fn, fn).catch((e) => console.error("[converge] queued work failed", e));
    return this.queue;
  }

  private connect(): void {
    if (this.stopped || this.conn) return;
    const gen = ++this.gen;
    const conn = this.opts.transport.connect({
      onOpen: () => this.enqueue(() => {
        if (this.gen !== gen || this.conn !== conn) return;
        const out: Output[] = [];
        this.engineGen = this.engine.connected(this.now(), out);
        this.handle(out);
        this.armTick();
        this.emitStatus();
      }),
      onMessage: (msg) => this.enqueue(() => {
        if (this.gen !== gen || this.conn !== conn) return;
        this.onServerMessage(msg);
      }),
      onClose: () => this.enqueue(() => {
        if (this.conn !== conn) return;
        this.conn = null;
        this.lostConnection();
      }),
    });
    this.conn = conn;
  }

  private engineGen = 0;

  private lostConnection(): void {
    this.engine.disconnected();
    this.emitStatus();
    if (this.stopped) return;
    const [lo, hi] = this.opts.reconnectBackoffMs ?? [500, 30_000];
    const delay = Math.min(hi, lo * 2 ** this.backoffAttempt) * (0.5 + Math.random() * 0.5);
    this.backoffAttempt = Math.min(this.backoffAttempt + 1, 10);
    this.reconnectTimer = this.setTimer(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }

  private onServerMessage(msg: ServerMsg): void {
    const out: Output[] = [];
    const before = this.engine.lastSeqValue;
    this.engine.message(this.engineGen, msg, this.now(), out);
    switch (msg.type) {
      case "welcome":
        this.backoffAttempt = 0;
        this.presenceTable.clear();
        for (const e of msg.presence) this.presenceTable.set(e.replica.toString(), e);
        this.emitPresence();
        this.emitChange();
        break;
      case "commit":
        this.opsSinceSnapshot++;
        this.emitChange();
        break;
      case "presence_update":
        this.presenceTable.set(msg.entry.replica.toString(), msg.entry);
        this.emitPresence();
        break;
      case "presence_leave":
        this.presenceTable.delete(msg.replica.toString());
        this.emitPresence();
        break;
      default:
        break;
    }
    if (this.engine.lastSeqValue !== before || msg.type === "ack" || msg.type === "nack") this.emitStatus();
    this.handle(out);
    this.armTick();
    this.scheduleSnapshot();
  }

  private handle(out: Output[]): void {
    for (const o of out) {
      switch (o.type) {
        case "send":
          this.conn?.send(o.msg);
          break;
        case "persist": {
          const op = o.op;
          this.unsaved++;
          this.emitStatus();
          void this.enqueue(async () => {
            try {
              await this.opts.store.appendPending(this.engine.replica, op, this.engine.replicaRow());
            } catch (e) {
              // D1: never transmit an op the store did not accept.
              console.error("[converge] local write failed; op stays unsaved", e);
              this.unsaved--;
              this.emitStatus();
              return;
            }
            this.unsaved--;
            const more: Output[] = [];
            this.engine.persisted(op.id.counter, this.now(), more);
            this.handle(more);
            this.armTick();
            this.emitStatus();
            this.scheduleSnapshot();
          });
          break;
        }
        case "reconnect":
          this.disconnect();
          if (this.reconnectTimer) this.clearTimer(this.reconnectTimer);
          this.reconnectTimer = this.setTimer(() => {
            this.reconnectTimer = null;
            this.connect();
          }, 10);
          break;
      }
    }
  }

  private armTick(): void {
    if (this.tickTimer) {
      this.clearTimer(this.tickTimer);
      this.tickTimer = null;
    }
    const d = this.engine.nextDeadline();
    if (d === null || this.stopped) return;
    const delay = Math.max(0, d - this.clock.monoMs());
    this.tickTimer = this.setTimer(() => {
      this.tickTimer = null;
      void this.enqueue(() => {
        const out: Output[] = [];
        this.engine.tick(this.now(), out);
        this.handle(out);
        this.armTick();
      });
    }, delay);
  }

  private scheduleSnapshot(): void {
    const every = this.opts.snapshotEveryOps ?? 200;
    const idle = this.opts.snapshotIdleMs ?? 2_000;
    if (this.snapshotTimer) this.clearTimer(this.snapshotTimer);
    if (this.stopped) return;
    if (this.opsSinceSnapshot >= every) {
      void this.enqueue(() => this.writeSnapshot());
      return;
    }
    if (this.opsSinceSnapshot > 0) {
      this.snapshotTimer = this.setTimer(() => {
        this.snapshotTimer = null;
        void this.enqueue(() => this.writeSnapshot());
      }, idle);
    }
  }

  private async writeSnapshot(): Promise<void> {
    if (this.snapshotInFlight || !this.engine) return;
    this.snapshotInFlight = true;
    try {
      const w = this.engine.snapshot();
      this.opsSinceSnapshot = 0;
      const ok = await this.opts.store.writeSnapshot(this.engine.replica, w);
      if (ok) this.engine.snapshotPersisted(w.acked);
    } finally {
      this.snapshotInFlight = false;
      this.emitStatus();
    }
  }

  private emitChange(): void {
    for (const l of this.changeListeners) l(this.engine.document);
    this.emitStatus();
  }
  private emitStatus(): void {
    if (!this.engine) return;
    const s = this.status();
    for (const l of this.statusListeners) l(s);
  }
  private emitPresence(): void {
    const p = this.presence();
    for (const l of this.presenceListeners) l(p);
  }
}

/**
 * Durable local store. The IndexedDB implementation enforces the ordering
 * invariants from `docs/INVARIANTS.md` §2:
 *
 * - D1  an op and its replica row (counter, HLC high-water) are written in
 *       one transaction, and the engine only transmits after it commits;
 * - D2  pending entries are deleted in the same transaction as a snapshot
 *       taken after they were applied;
 * - D3  `seq` lives inside the snapshot record;
 * - D4  the snapshot seq is monotone (the write aborts if a newer one exists);
 * - D5  one replica id per live tab, enforced by a lock; a starting tab
 *       adopts an unlocked slot before minting a new one.
 */
import { openDB, type DBSchema, type IDBPDatabase } from "idb";
import {
  hlcFromJson,
  hlcToJson,
  opFromJson,
  opToJson,
  type HlcJson,
  type Op,
  type OpJson,
  type ReplicaId,
} from "@converge/engine";
import type { ReplicaRow, SnapshotWrite } from "./engine.js";

export interface ReplicaSlot {
  replica: ReplicaId;
  row: ReplicaRow;
  snapshot: { bytes: Uint8Array; seq: bigint } | null;
  pending: Op[];
  release: () => void;
}

export interface Store {
  /** Adopt an unlocked replica slot or mint a new one (D5). */
  acquireReplica(mint: () => ReplicaId): Promise<ReplicaSlot>;
  /** One transaction: pending entry + replica row (D1, H6). */
  appendPending(replica: ReplicaId, op: Op, row: ReplicaRow): Promise<void>;
  /**
   * One transaction: snapshot record (with seq), delete acked pending
   * entries, raise the row's high-water. Returns false when a newer
   * snapshot is already stored (D4) — nothing is written then.
   */
  writeSnapshot(replica: ReplicaId, w: SnapshotWrite): Promise<boolean>;
  close(): void;
}

// ----- locks -----

export interface Locks {
  /** Try to take `name`; resolves to a release function or null if held. */
  tryAcquire(name: string): Promise<(() => void) | null>;
}

/** In-process lock table (tests, and browsers without the Web Locks API). */
export class MemoryLocks implements Locks {
  private held = new Set<string>();
  async tryAcquire(name: string): Promise<(() => void) | null> {
    if (this.held.has(name)) return null;
    this.held.add(name);
    return () => this.held.delete(name);
  }
}

/** Web Locks API: the lock is released when the tab dies. */
export class WebLocks implements Locks {
  static available(): boolean {
    return typeof navigator !== "undefined" && !!navigator.locks;
  }
  tryAcquire(name: string): Promise<(() => void) | null> {
    return new Promise((resolve) => {
      let release: (() => void) | null = null;
      const held = new Promise<void>((r) => (release = r));
      void navigator.locks.request(name, { ifAvailable: true }, async (lock) => {
        if (!lock) {
          resolve(null);
          return;
        }
        resolve(() => release?.());
        await held;
      });
    });
  }
}

// ----- IndexedDB -----

interface RowRecord {
  replica: string;
  nextCounter: string;
  highWater: HlcJson;
  clockOffsetMs: string;
}
interface SnapshotRecord {
  key: "doc";
  seq: string;
  bytes: Uint8Array;
}
interface PendingRecord {
  replica: string;
  counter: number;
  op: OpJson;
}

interface Schema extends DBSchema {
  replicas: { key: string; value: RowRecord };
  snapshot: { key: "doc"; value: SnapshotRecord };
  pending: { key: [string, number]; value: PendingRecord; indexes: { byReplica: string } };
}

function rowFromRecord(r: RowRecord): ReplicaRow {
  return {
    nextCounter: BigInt(r.nextCounter),
    highWater: hlcFromJson(r.highWater),
    clockOffsetMs: BigInt(r.clockOffsetMs),
  };
}
function rowToRecord(replica: ReplicaId, row: ReplicaRow): RowRecord {
  return {
    replica: replica.toString(),
    nextCounter: row.nextCounter.toString(),
    highWater: hlcToJson(row.highWater),
    clockOffsetMs: row.clockOffsetMs.toString(),
  };
}

export class IndexedDbStore implements Store {
  private constructor(
    private readonly db: IDBPDatabase<Schema>,
    private readonly docId: string,
    private readonly locks: Locks,
  ) {}

  /** Uses the global `indexedDB` (tests install `fake-indexeddb/auto`). */
  static async open(docId: string, opts: { locks?: Locks } = {}): Promise<IndexedDbStore> {
    const db = await openDB<Schema>(`converge:${docId}`, 1, {
      upgrade(db) {
        db.createObjectStore("replicas", { keyPath: "replica" });
        db.createObjectStore("snapshot", { keyPath: "key" });
        const p = db.createObjectStore("pending", { keyPath: ["replica", "counter"] });
        p.createIndex("byReplica", "replica");
      },
    });
    const locks = opts.locks ?? (WebLocks.available() ? new WebLocks() : new MemoryLocks());
    return new IndexedDbStore(db, docId, locks);
  }

  private lockName(replica: ReplicaId): string {
    return `converge:${this.docId}:replica:${replica}`;
  }

  async acquireReplica(mint: () => ReplicaId): Promise<ReplicaSlot> {
    const rows = await this.db.getAll("replicas");
    // Prefer slots with orphaned pending ops so they get flushed first.
    const pendingCount = new Map<string, number>();
    for (const r of rows)
      pendingCount.set(r.replica, await this.db.countFromIndex("pending", "byReplica", r.replica));
    rows.sort(
      (a, b) =>
        pendingCount.get(b.replica)! - pendingCount.get(a.replica)! ||
        (a.replica < b.replica ? -1 : 1),
    );
    for (const r of rows) {
      const replica = BigInt(r.replica);
      const release = await this.locks.tryAcquire(this.lockName(replica));
      if (!release) continue;
      return { replica, ...(await this.load(replica, rowFromRecord(r))), release };
    }
    // Mint a fresh replica; retry on the (astronomically unlikely) collision.
    for (;;) {
      const replica = mint();
      if (rows.some((r) => r.replica === replica.toString())) continue;
      const release = await this.locks.tryAcquire(this.lockName(replica));
      if (!release) continue;
      const row: ReplicaRow = {
        nextCounter: 1n,
        highWater: { wall: 0n, logical: 0 },
        clockOffsetMs: 0n,
      };
      await this.db.put("replicas", rowToRecord(replica, row));
      return { replica, ...(await this.load(replica, row)), release };
    }
  }

  private async load(
    replica: ReplicaId,
    row: ReplicaRow,
  ): Promise<Omit<ReplicaSlot, "replica" | "release">> {
    const snap = await this.db.get("snapshot", "doc");
    const pendingRecords = await this.db.getAllFromIndex(
      "pending",
      "byReplica",
      replica.toString(),
    );
    pendingRecords.sort((a, b) => a.counter - b.counter);
    return {
      row,
      snapshot: snap ? { bytes: snap.bytes, seq: BigInt(snap.seq) } : null,
      pending: pendingRecords.map((p) => opFromJson(p.op)),
    };
  }

  async appendPending(replica: ReplicaId, op: Op, row: ReplicaRow): Promise<void> {
    const tx = this.db.transaction(["pending", "replicas"], "readwrite");
    await Promise.all([
      tx
        .objectStore("pending")
        .put({ replica: replica.toString(), counter: Number(op.id.counter), op: opToJson(op) }),
      tx.objectStore("replicas").put(rowToRecord(replica, row)),
      tx.done,
    ]);
  }

  async writeSnapshot(replica: ReplicaId, w: SnapshotWrite): Promise<boolean> {
    const tx = this.db.transaction(["snapshot", "pending", "replicas"], "readwrite");
    const snapStore = tx.objectStore("snapshot");
    const existing = await snapStore.get("doc");
    if (existing && BigInt(existing.seq) > w.seq) {
      tx.abort();
      await tx.done.catch(() => undefined);
      return false;
    }
    const rowStore = tx.objectStore("replicas");
    const rec = await rowStore.get(replica.toString());
    const writes: Promise<unknown>[] = [
      snapStore.put({ key: "doc", seq: w.seq.toString(), bytes: w.bytes }),
    ];
    for (const c of w.acked)
      writes.push(tx.objectStore("pending").delete([replica.toString(), Number(c)]));
    if (rec) {
      const row = rowFromRecord(rec);
      const hw = row.highWater;
      const newer =
        w.highWater.wall > hw.wall ||
        (w.highWater.wall === hw.wall && w.highWater.logical > hw.logical);
      if (newer)
        writes.push(rowStore.put(rowToRecord(replica, { ...row, highWater: w.highWater })));
    }
    await Promise.all([...writes, tx.done]);
    return true;
  }

  close(): void {
    this.db.close();
  }
}

/** Volatile store with the same contract, for tests without IndexedDB. */
export class MemoryStore implements Store {
  private rows = new Map<string, ReplicaRow>();
  private snapshot: { bytes: Uint8Array; seq: bigint } | null = null;
  private pending = new Map<string, Map<bigint, Op>>();
  constructor(private readonly locks: Locks = new MemoryLocks()) {}

  async acquireReplica(mint: () => ReplicaId): Promise<ReplicaSlot> {
    const entries = [...this.rows.entries()].sort(
      (a, b) =>
        (this.pending.get(b[0])?.size ?? 0) - (this.pending.get(a[0])?.size ?? 0) ||
        (a[0] < b[0] ? -1 : 1),
    );
    for (const [key, row] of entries) {
      const release = await this.locks.tryAcquire(`mem:${key}`);
      if (!release) continue;
      const replica = BigInt(key);
      return { replica, row, snapshot: this.snapshot, pending: this.pendingOf(replica), release };
    }
    const replica = mint();
    const release = (await this.locks.tryAcquire(`mem:${replica}`))!;
    const row: ReplicaRow = {
      nextCounter: 1n,
      highWater: { wall: 0n, logical: 0 },
      clockOffsetMs: 0n,
    };
    this.rows.set(replica.toString(), row);
    return { replica, row, snapshot: this.snapshot, pending: [], release };
  }
  private pendingOf(replica: ReplicaId): Op[] {
    return [...(this.pending.get(replica.toString())?.values() ?? [])].sort((a, b) =>
      a.id.counter < b.id.counter ? -1 : 1,
    );
  }
  async appendPending(replica: ReplicaId, op: Op, row: ReplicaRow): Promise<void> {
    const key = replica.toString();
    if (!this.pending.has(key)) this.pending.set(key, new Map());
    this.pending.get(key)!.set(op.id.counter, op);
    this.rows.set(key, row);
  }
  async writeSnapshot(replica: ReplicaId, w: SnapshotWrite): Promise<boolean> {
    if (this.snapshot && this.snapshot.seq > w.seq) return false;
    this.snapshot = { bytes: w.bytes, seq: w.seq };
    for (const c of w.acked) this.pending.get(replica.toString())?.delete(c);
    const row = this.rows.get(replica.toString());
    if (row) this.rows.set(replica.toString(), { ...row, highWater: w.highWater });
    return true;
  }
  close(): void {}
}

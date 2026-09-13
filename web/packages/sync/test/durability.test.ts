/**
 * Crash-window tests T2.1–T2.9 from docs/IMPLEMENTATION_RISKS.md §2, against
 * the real IndexedDbStore running on fake-indexeddb.
 */
import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { beforeEach, describe, expect, it } from "vitest";
import { Document, hashHex, hlcEquals, opId, type Op, type OpKind } from "@converge/engine";
import { Client } from "../src/client.js";
import { IndexedDbStore, MemoryLocks, type Store } from "../src/store.js";
import type { ReplicaRow, SnapshotWrite } from "../src/engine.js";
import { TinyServer, settle } from "./harness.js";

const DOC = "durability-doc";
const USER = { name: "t", color: 1 };
let locks: MemoryLocks;
let server: TinyServer;

beforeEach(() => {
  // A fresh browser: new IndexedDB, new lock table, new server.
  (globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory();
  locks = new MemoryLocks();
  server = new TinyServer();
});

const rectKind: OpKind = { op: "create", kind: "rect", props: [["x", { t: "f64", v: 1 }]] };
function move(object: Op["id"], x: number): OpKind {
  return { op: "set_props", object, entries: [["x", { t: "f64", v: x }]] };
}

/** A store wrapper that can hold or fail writes to open crash windows. */
class GatedStore implements Store {
  holdAppends = false;
  failAppends = false;
  private heldAppends: Array<() => void> = [];
  constructor(private readonly inner: Store) {}
  acquireReplica = (mint: () => bigint) => this.inner.acquireReplica(mint);
  async appendPending(replica: bigint, op: Op, row: ReplicaRow): Promise<void> {
    if (this.failAppends) throw new Error("QuotaExceededError");
    if (this.holdAppends) await new Promise<void>((r) => this.heldAppends.push(r));
    return this.inner.appendPending(replica, op, row);
  }
  releaseAppends(): void {
    const h = this.heldAppends;
    this.heldAppends = [];
    for (const r of h) r();
  }
  writeSnapshot = (replica: bigint, w: SnapshotWrite) => this.inner.writeSnapshot(replica, w);
  close = () => this.inner.close();
}

async function newTab(opts: { transport?: TinyServer; gated?: boolean; snapshotIdleMs?: number } = {}) {
  const idb = await IndexedDbStore.open(DOC, { locks });
  const store = new GatedStore(idb);
  const client = new Client({
    docId: DOC, user: USER, store, transport: opts.transport ?? server, snapshotIdleMs: opts.snapshotIdleMs ?? 60_000,
    snapshotEveryOps: 1_000_000, reconnectBackoffMs: [1, 5],
  });
  await client.start();
  await settle();
  return { client, store };
}

function submittedIds(): string[] {
  return server.received.flatMap((m) => (m.type === "submit" ? m.ops.map((o) => `${o.id.replica}:${o.id.counter}`) : []));
}

describe("IndexedDB durability and crash ordering", () => {
  it("T2.1 crash before the pending write: op is gone and was never sent", async () => {
    const { client, store } = await newTab();
    const a = client.edit(rectKind);
    await settle();
    store.holdAppends = true;
    const b = client.edit(move(a.id, 5));
    await settle(5);
    expect(client.status().unsaved).toBe(1);
    client.simulateCrash();
    const ids = submittedIds();
    expect(ids).toContain(`${a.id.replica}:${a.id.counter}`);
    expect(ids).not.toContain(`${b.id.replica}:${b.id.counter}`);

    const { client: again } = await newTab();
    expect(again.replica).toBe(client.replica); // slot adopted (D5)
    expect(again.document.get(a.id)?.get("x")).toEqual({ t: "f64", v: 1 });
    await settle(30);
    expect(again.status().unacked).toBe(0);
    expect(again.document.get(a.id)?.get("x")).toEqual({ t: "f64", v: 1 }); // b never existed
    expect(submittedIds().filter((id) => id.endsWith(":2"))).toEqual([]);
    await again.stop();
  });

  it("T2.2 crash after the pending write, before send: resent verbatim", async () => {
    const { client } = await newTab();
    await settle();
    server.offline = true; // reconnect attempts fail
    client.disconnect(); // offline: writes land, nothing is sent
    const a = client.edit(rectKind);
    await client.idle();
    await settle();
    expect(submittedIds()).toEqual([]);
    client.simulateCrash();
    server.offline = false;

    const { client: again } = await newTab();
    await settle(50);
    const sent = server.received.filter((m) => m.type === "submit").flatMap((m) => (m.type === "submit" ? m.ops : []));
    expect(sent.length).toBe(1);
    expect(sent[0]!.id).toEqual(a.id);
    expect(hlcEquals(sent[0]!.hlc, a.hlc)).toBe(true);
    expect(server.head).toBe(1);
    expect(again.hashHex()).toBe(hashHex(server.doc));
    await again.stop();
  });

  it("T2.3 crash after send, before Commit: resend is deduped with Ack and dropped at the next snapshot", async () => {
    const { client } = await newTab();
    server.holdCommits = true;
    const a = client.edit(rectKind);
    await client.idle();
    await settle();
    expect(submittedIds()).toEqual([`${a.id.replica}:${a.id.counter}`]);
    client.simulateCrash();
    server.releaseCommits(); // committed while the tab was dead

    const { client: again } = await newTab();
    await settle(50);
    expect(again.status().unacked).toBe(0);
    expect(again.status().pending).toBe(1); // acked, awaiting the snapshot txn (D2)
    await again.flush();
    expect(again.status().pending).toBe(0);
    expect(server.head).toBe(1);
    expect(again.hashHex()).toBe(hashHex(server.doc));
    await again.stop();
  });

  it("T2.4 Commit applied, crash before the snapshot: own edit survives an offline restart", async () => {
    const { client } = await newTab();
    const a = client.edit(rectKind);
    await client.idle();
    await settle(30);
    expect(client.status().unacked).toBe(0);
    client.simulateCrash(); // no snapshot ever written

    const offline = new TinyServer();
    offline.offline = true;
    const { client: again } = await newTab({ transport: offline });
    expect(again.document.get(a.id)?.visible()).toBe(true); // from pending, D2
    expect(again.status().pending).toBe(1);
    again.simulateCrash();

    const { client: online } = await newTab();
    await settle(50);
    expect(online.hashHex()).toBe(hashHex(server.doc));
    await online.stop();
  });

  it("T2.5 the snapshot transaction carries seq and deletes acked pending atomically", async () => {
    const { client } = await newTab();
    const a = client.edit(rectKind);
    client.edit(move(a.id, 3));
    await client.idle();
    await settle(30);
    await client.flush();
    client.simulateCrash();
    // Reopen the raw store: the snapshot seq is 2 and no pending remains.
    const raw = await IndexedDbStore.open(DOC, { locks });
    const slot = await raw.acquireReplica(() => 99n);
    expect(slot.replica).toBe(client.replica);
    expect(slot.snapshot?.seq).toBe(2n);
    expect(slot.pending).toEqual([]);
    slot.release();
    raw.close();
  });

  it("T2.6 a lower-seq snapshot never overwrites a newer one (D4)", async () => {
    const raw = await IndexedDbStore.open(DOC, { locks });
    const slotA = await raw.acquireReplica(() => 1n);
    const bytes = new Document();
    const w = (seq: bigint): SnapshotWrite => ({ bytes: new Uint8Array([67, 86, 71, 83, 1, 0, 0, 0, 0, 0, 0, 0, 0]), seq, highWater: { wall: 0n, logical: 0 }, acked: [] });
    void bytes;
    expect(await raw.writeSnapshot(1n, w(100n))).toBe(true);
    expect(await raw.writeSnapshot(1n, w(90n))).toBe(false);
    expect(await raw.writeSnapshot(1n, w(100n))).toBe(true);
    const again = await raw.acquireReplica(() => 2n); // second tab: new replica, shared snapshot
    expect(again.replica).toBe(2n);
    expect(again.snapshot?.seq).toBe(100n);
    slotA.release();
    again.release();
    raw.close();
  });

  it("T2.8 a failing local write keeps the op unsent", async () => {
    const { client, store } = await newTab();
    store.failAppends = true;
    const a = client.edit(rectKind);
    await client.idle();
    await settle();
    expect(submittedIds()).toEqual([]);
    expect(client.status().unsaved).toBe(0);
    expect(client.status().pending).toBe(1); // still in memory, never durable
    store.failAppends = false;
    client.edit(move(a.id, 2)); // a later write does not leapfrog the failed one
    await client.idle();
    await settle();
    expect(submittedIds()).toEqual([]);
    await client.stop();
  });

  it("T2.9 replica slot adoption: a live tab keeps its slot, a dead tab's slot is adopted and flushed", async () => {
    const { client: a } = await newTab();
    a.disconnect();
    const created = a.edit(rectKind);
    await a.idle();
    // Tab B starts while A is alive: must mint a new replica.
    const { client: b } = await newTab();
    expect(b.replica).not.toBe(a.replica);
    await b.stop();
    a.simulateCrash();
    // Tab C after A died: adopts A's identity and flushes A's queue.
    const { client: c } = await newTab();
    expect(c.replica).toBe(a.replica);
    await settle(50);
    expect(server.head).toBe(1);
    expect(server.log[0]!.id).toEqual(created.id);
    expect(c.hashHex()).toBe(hashHex(server.doc));
    await c.stop();
  });

  it("T2.7 random crash/restart interleavings never lose a saved op (model check)", async () => {
    let seed = 12345;
    const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
    let { client, store } = await newTab();
    const saved: Op[] = []; // ops whose local write resolved
    let objects: Op["id"][] = [];
    for (let step = 0; step < 60; step++) {
      const r = rnd();
      if (r < 0.45) {
        const kind = objects.length === 0 || rnd() < 0.3 ? rectKind : move(objects[Math.floor(rnd() * objects.length)]!, rnd() * 100);
        const crashInFlight = rnd() < 0.3;
        if (crashInFlight) store.holdAppends = true; // the write never completes
        const op = client.edit(kind);
        if (kind.op === "create" && !crashInFlight) objects.push(op.id);
        if (!crashInFlight) {
          await client.idle();
          saved.push(op);
        } else {
          await settle(2);
          client.simulateCrash();
          ({ client, store } = await newTab());
          await settle(3);
          continue;
        }
      } else if (r < 0.6) {
        server.holdCommits = !server.holdCommits;
        if (!server.holdCommits) server.releaseCommits();
      } else if (r < 0.75) {
        await client.flush();
      } else if (r < 0.85) {
        client.disconnect();
        await settle(3);
      } else {
        client.simulateCrash();
        ({ client, store } = await newTab());
        // Invariant: every saved op's effect is present after restart.
        const ref = new Document();
        for (const op of server.log) ref.apply(op);
        for (const op of saved) ref.apply(op);
        for (const op of client.document.entries()) void op;
        // The client may additionally hold acked-but-not-snapshotted ops (a superset), so
        // compare register by register on the saved ops' objects.
        for (const op of saved) {
          const id = op.kind.op === "create" ? op.id : op.kind.object;
          const mine = client.document.get(id);
          expect(mine, `object of saved op ${op.id.counter} after restart at step ${step}`).toBeDefined();
          const want = ref.get(id)!;
          for (const [k, reg] of want.props) {
            const have = mine!.props.get(k)!;
            expect(have.value).toEqual(reg.value);
          }
        }
      }
      await settle(3);
    }
    server.releaseCommits();
    server.holdCommits = false;
    client.reconnectNow();
    await settle(80);
    await client.flush();
    expect(client.status().unacked).toBe(0);
    expect(client.hashHex()).toBe(hashHex(server.doc));
    await client.stop();
    void opId;
  });
});

/** Test doubles: a tiny sequencing server and a controllable transport. */
import { Document, idKey, type Op } from "@converge/engine";
import { PROTOCOL_VERSION, type ClientMsg, type ServerMsg } from "@converge/protocol";
import type { Connection, Transport, TransportHandlers } from "../src/transport.js";

interface Live {
  h: TransportHandlers;
  open: boolean;
}

/**
 * Minimal in-memory server with the hub's observable behaviour for these
 * tests: sequencing, dedup with Ack, catch-up by ops, broadcast Commits.
 * `holdCommits` keeps accepted ops un-committed (persist latency); `paused`
 * queues inbound messages; `offline` refuses connections.
 */
export class TinyServer implements Transport {
  log: Op[] = [];
  private seen = new Map<string, number>();
  doc = new Document();
  conns: Live[] = [];
  received: ClientMsg[] = [];
  holdCommits = false;
  private held: Op[] = [];
  paused = false;
  private inbox: Array<[Live, ClientMsg]> = [];
  offline = false;
  serverTimeMs = 1_000_000n;

  connect(h: TransportHandlers): Connection {
    const live: Live = { h, open: false };
    if (this.offline) {
      queueMicrotask(() => h.onClose());
      return { send: () => {}, close: () => {} };
    }
    this.conns.push(live);
    queueMicrotask(() => {
      live.open = true;
      h.onOpen();
    });
    return {
      send: (msg) => {
        if (!live.open) return;
        this.received.push(msg);
        if (this.paused) this.inbox.push([live, msg]);
        else this.handle(live, msg);
      },
      close: () => {
        live.open = false;
        this.conns = this.conns.filter((c) => c !== live);
      },
    };
  }

  resume(): void {
    this.paused = false;
    const q = this.inbox;
    this.inbox = [];
    for (const [live, msg] of q) this.handle(live, msg);
  }

  releaseCommits(): void {
    this.holdCommits = false;
    const ops = this.held;
    this.held = [];
    for (const op of ops) this.commit(op);
  }

  /** Close every live connection from the server side. */
  dropAll(): void {
    for (const c of this.conns) {
      c.open = false;
      queueMicrotask(() => c.h.onClose());
    }
    this.conns = [];
  }

  get head(): number {
    return this.log.length;
  }

  private send(live: Live, msg: ServerMsg): void {
    if (live.open) live.h.onMessage(msg);
  }

  private handle(live: Live, msg: ClientMsg): void {
    switch (msg.type) {
      case "hello": {
        if (msg.hello.version !== PROTOCOL_VERSION) throw new Error("version");
        const last = Number(msg.hello.lastSeq);
        const ops = this.log.slice(last).map((op, i) => ({ seq: BigInt(last + i + 1), op }));
        this.send(live, {
          type: "welcome", session: 1n, serverTimeMs: this.serverTimeMs, durableHeadSeq: BigInt(this.head),
          catchUp: msg.hello.wantSnapshot || last > this.head ? { kind: "snapshot", bytes: encode(this.doc), seq: BigInt(this.head) } : { kind: "ops", ops },
          presence: [],
        });
        break;
      }
      case "submit":
        for (const op of msg.ops) {
          const k = idKey(op.id);
          const prior = this.seen.get(k);
          if (prior !== undefined) {
            if (prior > 0) this.send(live, { type: "ack", opId: op.id, seq: BigInt(prior) });
            continue; // accepted but not yet durable: the Commit will follow
          }
          this.seen.set(k, 0);
          if (this.holdCommits) this.held.push(op);
          else this.commit(op);
        }
        break;
      default:
        break;
    }
  }

  private commit(op: Op): void {
    this.log.push(op);
    this.doc.apply(op);
    const seq = this.head;
    this.seen.set(idKey(op.id), seq);
    for (const c of [...this.conns]) this.send(c, { type: "commit", seq: BigInt(seq), op });
  }
}

import { encodeSnapshot } from "@converge/engine";
function encode(doc: Document): Uint8Array {
  return encodeSnapshot(doc);
}

export async function settle(rounds = 20): Promise<void> {
  for (let i = 0; i < rounds; i++) await new Promise((r) => setTimeout(r, 0));
}

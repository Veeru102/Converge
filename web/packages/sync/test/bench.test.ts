import { describe, expect, it } from "vitest";
import { BenchStats, type ClientEvent, type ClientStatus } from "../src/index.js";

const id = (c: number) => ({ replica: 7n, counter: BigInt(c) });
const status = (over: Partial<ClientStatus>): ClientStatus => ({
  state: "live",
  replica: 7n,
  lastSeq: 0n,
  pending: 0,
  unacked: 0,
  unsaved: 0,
  offline: false,
  retryAt: null,
  ...over,
});

describe("BenchStats", () => {
  it("measures submit → own commit latency from the first transmission", () => {
    const b = new BenchStats(1000);
    const ev: ClientEvent[] = [
      { type: "submit", opIds: [id(1), id(2)], at: 100 },
      { type: "submit", opIds: [id(1)], at: 150 }, // retransmission does not reset the clock
      { type: "commit", opId: id(1), seq: 1n, own: true, at: 180 },
      { type: "commit", opId: { replica: 9n, counter: 1n }, seq: 2n, own: false, at: 190 },
      { type: "commit", opId: id(2), seq: 3n, own: true, at: 300 },
    ];
    ev.forEach((e) => b.feed(e));
    const s = b.stats(300);
    expect(s.commitLatency.n).toBe(2);
    expect(s.commitLatency.p50).toBe(80);
    expect(s.commitLatency.max).toBe(200);
    expect(s.commits).toBe(3);
    expect(s.opsInPerSec).toBe(3);
    expect(s.opsOutPerSec).toBe(3);
  });

  it("measures reconnect and drain time", () => {
    const b = new BenchStats();
    b.feed({ type: "state", from: "live", to: "disconnected", at: 1000 });
    b.feed({ type: "state", from: "disconnected", to: "hello_sent", at: 1400 });
    b.feed({ type: "welcome", at: 1500, unacked: 3 });
    b.feed({ type: "state", from: "hello_sent", to: "live", at: 1500 });
    b.feed({ type: "status", status: status({ unacked: 2 }), at: 1600 });
    b.feed({ type: "status", status: status({ unacked: 0 }), at: 1750 });
    const s = b.stats(2000);
    expect(s.reconnect.last).toBe(500);
    expect(s.drain.last).toBe(250);
  });

  it("drops window samples older than the window", () => {
    const b = new BenchStats(1000);
    b.feed({ type: "commit", opId: id(1), seq: 1n, own: false, at: 0 });
    b.feed({ type: "commit", opId: id(2), seq: 2n, own: false, at: 900 });
    expect(b.stats(1500).opsInPerSec).toBe(1);
    expect(b.stats(1500).commits).toBe(2);
  });
});

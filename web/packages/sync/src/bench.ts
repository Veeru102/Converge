/** Rolling performance statistics derived from `Client.onEvent`. Pure: feed events, read `stats()`. */
import { idKey } from "@converge/engine";
import type { ClientEvent } from "./client.js";

export interface Quantiles {
  n: number;
  p50: number;
  p95: number;
  max: number;
  last: number | null;
}

export interface BenchSnapshot {
  /** submit → own Commit, ms */
  commitLatency: Quantiles;
  /** left live → live again, ms */
  reconnect: Quantiles;
  /** Welcome with unacked ops → unacked = 0, ms */
  drain: Quantiles;
  /** commits observed per second over the window */
  opsInPerSec: number;
  /** ops submitted per second over the window */
  opsOutPerSec: number;
  /** ops committed since start (all replicas) */
  commits: number;
}

const quantile = (sorted: number[], q: number): number =>
  sorted.length ? sorted[Math.max(0, Math.ceil(q * sorted.length) - 1)]! : 0;

class Samples {
  private v: number[] = [];
  constructor(private readonly cap = 2000) {}
  push(x: number): void {
    this.v.push(x);
    if (this.v.length > this.cap) this.v.splice(0, this.v.length - this.cap);
  }
  quantiles(): Quantiles {
    const s = [...this.v].sort((a, b) => a - b);
    return {
      n: s.length,
      p50: quantile(s, 0.5),
      p95: quantile(s, 0.95),
      max: s.length ? s[s.length - 1]! : 0,
      last: this.v.length ? this.v[this.v.length - 1]! : null,
    };
  }
  get values(): readonly number[] {
    return this.v;
  }
}

export class BenchStats {
  private readonly submitAt = new Map<string, number>();
  private readonly latency = new Samples();
  private readonly reconnects = new Samples(200);
  private readonly drains = new Samples(200);
  private commitTimes: number[] = [];
  private submitTimes: number[] = [];
  private leftLiveAt: number | null = null;
  private drainStart: number | null = null;
  private lastUnacked = 0;
  private total = 0;

  constructor(private readonly windowMs = 5000) {}

  feed(e: ClientEvent): void {
    switch (e.type) {
      case "submit":
        for (const id of e.opIds) {
          const k = idKey(id);
          if (!this.submitAt.has(k)) this.submitAt.set(k, e.at); // first transmission counts
          this.submitTimes.push(e.at);
        }
        break;
      case "commit": {
        this.total++;
        this.commitTimes.push(e.at);
        if (e.own) {
          const k = idKey(e.opId);
          const t0 = this.submitAt.get(k);
          if (t0 !== undefined) {
            this.latency.push(e.at - t0);
            this.submitAt.delete(k);
          }
        }
        break;
      }
      case "ack":
        this.submitAt.delete(idKey(e.opId));
        break;
      case "state":
        if (e.from === "live" && e.to !== "live") this.leftLiveAt = e.at;
        if (e.to === "live" && this.leftLiveAt !== null) {
          this.reconnects.push(e.at - this.leftLiveAt);
          this.leftLiveAt = null;
        }
        break;
      case "welcome":
        this.drainStart = e.unacked > 0 ? e.at : null;
        this.lastUnacked = e.unacked;
        break;
      case "status":
        if (this.drainStart !== null && this.lastUnacked > 0 && e.status.unacked === 0) {
          this.drains.push(e.at - this.drainStart);
          this.drainStart = null;
        }
        this.lastUnacked = e.status.unacked;
        break;
    }
  }

  stats(now: number): BenchSnapshot {
    const cut = now - this.windowMs;
    this.commitTimes = this.commitTimes.filter((t) => t >= cut);
    this.submitTimes = this.submitTimes.filter((t) => t >= cut);
    const secs = this.windowMs / 1000;
    return {
      commitLatency: this.latency.quantiles(),
      reconnect: this.reconnects.quantiles(),
      drain: this.drains.quantiles(),
      opsInPerSec: this.commitTimes.length / secs,
      opsOutPerSec: this.submitTimes.length / secs,
      commits: this.total,
    };
  }

  /** Raw latency samples (for reports). */
  get latencies(): readonly number[] {
    return this.latency.values;
  }
}

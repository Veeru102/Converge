import type { ReplicaId } from "./ids.js";
import { compareBig } from "./ids.js";

export interface Hlc {
  readonly wall: bigint;
  readonly logical: number; // u16
}

export interface Stamp {
  readonly hlc: Hlc;
  readonly replica: ReplicaId;
}

export const HLC_MIN: Hlc = { wall: 0n, logical: 0 };
export const STAMP_MIN: Stamp = { hlc: HLC_MIN, replica: 0n };
const LOGICAL_MAX = 0xffff;

export function hlc(wall: bigint, logical: number): Hlc {
  return { wall, logical };
}

export function compareHlc(a: Hlc, b: Hlc): number {
  return compareBig(a.wall, b.wall) || (a.logical < b.logical ? -1 : a.logical > b.logical ? 1 : 0);
}

/** Total order `(wall, logical, replica)` — must match the Rust `Stamp` ordering. */
export function compareStamp(a: Stamp, b: Stamp): number {
  return compareHlc(a.hlc, b.hlc) || compareBig(a.replica, b.replica);
}

export function hlcEquals(a: Hlc, b: Hlc): boolean {
  return a.wall === b.wall && a.logical === b.logical;
}

/**
 * Per-replica HLC generator. Ticks are strictly increasing regardless of the
 * physical clock; `wall` never decreases; logical overflow bumps `wall`.
 */
export class HlcClock {
  private last: Hlc;

  constructor(highWater: Hlc = HLC_MIN) {
    this.last = highWater;
  }

  highWater(): Hlc {
    return this.last;
  }

  tick(physicalMs: bigint): Hlc {
    let next: Hlc;
    if (physicalMs > this.last.wall) {
      next = { wall: physicalMs, logical: 0 };
    } else if (this.last.logical < LOGICAL_MAX) {
      next = { wall: this.last.wall, logical: this.last.logical + 1 };
    } else {
      next = { wall: this.last.wall + 1n, logical: 0 };
    }
    this.last = next;
    return next;
  }

  observe(remote: Hlc, physicalMs: bigint): void {
    const wall = max3(this.last.wall, remote.wall, physicalMs);
    let logical: number;
    if (wall === this.last.wall && wall === remote.wall) {
      logical = Math.max(this.last.logical, remote.logical);
    } else if (wall === this.last.wall) {
      logical = this.last.logical;
    } else if (wall === remote.wall) {
      logical = remote.logical;
    } else {
      logical = 0;
    }
    this.last = { wall, logical };
  }
}

function max3(a: bigint, b: bigint, c: bigint): bigint {
  let m = a;
  if (b > m) m = b;
  if (c > m) m = c;
  return m;
}

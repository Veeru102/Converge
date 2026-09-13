/** Replica id: random u64. Always a bigint — JS numbers lose bits above 2^53. */
export type ReplicaId = bigint;

export interface OpId {
  readonly replica: ReplicaId;
  readonly counter: bigint;
}

export type ObjectId = OpId;

export const U64_MAX = (1n << 64n) - 1n;

export function opId(replica: bigint, counter: bigint): OpId {
  return { replica, counter };
}

export function idKey(id: OpId): string {
  return `${id.replica}:${id.counter}`;
}

export function parseIdKey(key: string): OpId {
  const i = key.indexOf(":");
  return { replica: BigInt(key.slice(0, i)), counter: BigInt(key.slice(i + 1)) };
}

export function idEquals(a: OpId, b: OpId): boolean {
  return a.replica === b.replica && a.counter === b.counter;
}

export function compareBig(a: bigint, b: bigint): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

export function compareId(a: OpId, b: OpId): number {
  return compareBig(a.replica, b.replica) || compareBig(a.counter, b.counter);
}

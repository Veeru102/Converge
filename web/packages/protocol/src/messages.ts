/** Logical protocol messages — mirrors `crates/converge-proto`. */
import type { Op, OpId, ReplicaId } from "@converge/engine";

export const PROTOCOL_VERSION = 1;
export const MAX_SUBMIT_OPS = 256;

export interface UserInfo {
  name: string;
  color: number;
}

export interface Hello {
  version: number;
  doc: string;
  replica: ReplicaId;
  lastSeq: bigint;
  wantSnapshot: boolean;
  user: UserInfo;
}

export interface PresenceState {
  cursor: [number, number] | null;
  selection: OpId[];
}

export interface PresenceEntry {
  replica: ReplicaId;
  user: UserInfo;
  state: PresenceState;
}

export type ClientMsg =
  | { type: "hello"; hello: Hello }
  | { type: "submit"; ops: Op[] }
  | { type: "presence"; state: PresenceState }
  | { type: "pong"; nonce: bigint };

export type CatchUp =
  | { kind: "ops"; ops: Array<{ seq: bigint; op: Op }> }
  | { kind: "snapshot"; bytes: Uint8Array; seq: bigint };

export type ByeReason =
  | "version_mismatch"
  | "unknown_doc"
  | "superseded"
  | "clock_skew"
  | "slow_consumer"
  | "restart"
  | "protocol_error";

export type NackReason = "clock_skew" | "malformed" | "rejected";

export function nackIsPermanent(r: NackReason): boolean {
  return r !== "clock_skew";
}

export type ServerMsg =
  | {
      type: "welcome";
      session: bigint;
      serverTimeMs: bigint;
      durableHeadSeq: bigint;
      catchUp: CatchUp;
      presence: PresenceEntry[];
    }
  | { type: "bye"; reason: ByeReason }
  | { type: "commit"; seq: bigint; op: Op }
  | { type: "ack"; opId: OpId; seq: bigint | null }
  | { type: "nack"; opId: OpId; reason: NackReason }
  | { type: "presence_update"; entry: PresenceEntry }
  | { type: "presence_leave"; replica: ReplicaId }
  | { type: "ping"; nonce: bigint };

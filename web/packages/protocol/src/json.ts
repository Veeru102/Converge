/** JSON form of protocol messages (u64 as strings) — matches `converge-proto::json`. */
import { hex, idFromJson, idToJson, opFromJson, opToJson, unhex, type IdJson, type OpJson } from "@converge/engine";
import type { ByeReason, CatchUp, ClientMsg, NackReason, PresenceEntry, PresenceState, ServerMsg, UserInfo } from "./messages.js";

export interface UserJson { name: string; color: number }
export interface PresenceJson { cursor: [number, number] | null; selection: IdJson[] }
export interface PresenceEntryJson { replica: string; user: UserJson; state: PresenceJson }

export function userToJson(u: UserInfo): UserJson {
  return { name: u.name, color: u.color };
}
export function userFromJson(j: UserJson): UserInfo {
  return { name: j.name ?? "", color: j.color ?? 0 };
}
export function presenceToJson(p: PresenceState): PresenceJson {
  return { cursor: p.cursor, selection: p.selection.map(idToJson) };
}
export function presenceFromJson(j: PresenceJson): PresenceState {
  return { cursor: Array.isArray(j.cursor) ? [j.cursor[0], j.cursor[1]] : null, selection: (j.selection ?? []).map(idFromJson) };
}
function entryToJson(e: PresenceEntry): PresenceEntryJson {
  return { replica: e.replica.toString(), user: userToJson(e.user), state: presenceToJson(e.state) };
}
function entryFromJson(j: PresenceEntryJson): PresenceEntry {
  return { replica: BigInt(j.replica), user: userFromJson(j.user), state: presenceFromJson(j.state) };
}

export type ClientMsgJson =
  | { type: "hello"; version: number; doc: string; replica: string; last_seq: string; want_snapshot: boolean; user: UserJson }
  | { type: "submit"; ops: OpJson[] }
  | { type: "presence"; state: PresenceJson }
  | { type: "pong"; nonce: string };

export function clientMsgToJson(m: ClientMsg): ClientMsgJson {
  switch (m.type) {
    case "hello":
      return {
        type: "hello", version: m.hello.version, doc: m.hello.doc, replica: m.hello.replica.toString(),
        last_seq: m.hello.lastSeq.toString(), want_snapshot: m.hello.wantSnapshot, user: userToJson(m.hello.user),
      };
    case "submit":
      return { type: "submit", ops: m.ops.map(opToJson) };
    case "presence":
      return { type: "presence", state: presenceToJson(m.state) };
    case "pong":
      return { type: "pong", nonce: m.nonce.toString() };
  }
}

export function clientMsgFromJson(j: ClientMsgJson): ClientMsg {
  switch (j.type) {
    case "hello":
      return { type: "hello", hello: { version: j.version, doc: j.doc, replica: BigInt(j.replica), lastSeq: BigInt(j.last_seq), wantSnapshot: !!j.want_snapshot, user: userFromJson(j.user) } };
    case "submit":
      return { type: "submit", ops: j.ops.map(opFromJson) };
    case "presence":
      return { type: "presence", state: presenceFromJson(j.state) };
    case "pong":
      return { type: "pong", nonce: BigInt(j.nonce) };
  }
}

export type CatchUpJson = { ops: Array<{ seq: string; op: OpJson }> } | { snapshot: string; seq: string };
export type ServerMsgJson =
  | { type: "welcome"; session: string; server_time_ms: string; durable_head_seq: string; catch_up: CatchUpJson; presence: PresenceEntryJson[] }
  | { type: "bye"; reason: ByeReason }
  | { type: "commit"; seq: string; op: OpJson }
  | { type: "ack"; op_id: IdJson; seq: string | null }
  | { type: "nack"; op_id: IdJson; reason: NackReason }
  | { type: "presence_update"; entry: PresenceEntryJson }
  | { type: "presence_leave"; replica: string }
  | { type: "ping"; nonce: string };

function catchUpToJson(c: CatchUp): CatchUpJson {
  return c.kind === "ops"
    ? { ops: c.ops.map((e) => ({ seq: e.seq.toString(), op: opToJson(e.op) })) }
    : { snapshot: hex(c.bytes), seq: c.seq.toString() };
}
function catchUpFromJson(j: CatchUpJson): CatchUp {
  if ("ops" in j) return { kind: "ops", ops: j.ops.map((e) => ({ seq: BigInt(e.seq), op: opFromJson(e.op) })) };
  return { kind: "snapshot", bytes: unhex(j.snapshot), seq: BigInt(j.seq) };
}

export function serverMsgToJson(m: ServerMsg): ServerMsgJson {
  switch (m.type) {
    case "welcome":
      return {
        type: "welcome", session: m.session.toString(), server_time_ms: m.serverTimeMs.toString(),
        durable_head_seq: m.durableHeadSeq.toString(), catch_up: catchUpToJson(m.catchUp), presence: m.presence.map(entryToJson),
      };
    case "bye": return { type: "bye", reason: m.reason };
    case "commit": return { type: "commit", seq: m.seq.toString(), op: opToJson(m.op) };
    case "ack": return { type: "ack", op_id: idToJson(m.opId), seq: m.seq === null ? null : m.seq.toString() };
    case "nack": return { type: "nack", op_id: idToJson(m.opId), reason: m.reason };
    case "presence_update": return { type: "presence_update", entry: entryToJson(m.entry) };
    case "presence_leave": return { type: "presence_leave", replica: m.replica.toString() };
    case "ping": return { type: "ping", nonce: m.nonce.toString() };
  }
}

export function serverMsgFromJson(j: ServerMsgJson): ServerMsg {
  switch (j.type) {
    case "welcome":
      return {
        type: "welcome", session: BigInt(j.session), serverTimeMs: BigInt(j.server_time_ms), durableHeadSeq: BigInt(j.durable_head_seq),
        catchUp: catchUpFromJson(j.catch_up), presence: (j.presence ?? []).map(entryFromJson),
      };
    case "bye": return { type: "bye", reason: j.reason };
    case "commit": return { type: "commit", seq: BigInt(j.seq), op: opFromJson(j.op) };
    case "ack": return { type: "ack", opId: idFromJson(j.op_id), seq: j.seq === null || j.seq === undefined ? null : BigInt(j.seq) };
    case "nack": return { type: "nack", opId: idFromJson(j.op_id), reason: j.reason };
    case "presence_update": return { type: "presence_update", entry: entryFromJson(j.entry) };
    case "presence_leave": return { type: "presence_leave", replica: BigInt(j.replica) };
    case "ping": return { type: "ping", nonce: BigInt(j.nonce) };
  }
}

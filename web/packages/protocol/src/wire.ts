/** Protobuf wire codec — must produce the same bytes as `converge-proto::wire` (see fixtures/wire.json). */
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import type { Op, OpId, OpKind, Value, ObjectKind as EngineKind, Hlc } from "@converge/engine";
import * as pb from "./gen/converge/v1/protocol_pb.js";
import type { ByeReason, CatchUp, ClientMsg, NackReason, PresenceEntry, PresenceState, ServerMsg, UserInfo } from "./messages.js";

// ----- core types -----

function idToPb(id: OpId): pb.OpId {
  return create(pb.OpIdSchema, { replica: id.replica, counter: id.counter });
}
function idFromPb(p: pb.OpId | undefined, what: string): OpId {
  if (!p) throw new Error(`missing ${what}`);
  return { replica: BigInt.asUintN(64, p.replica), counter: p.counter };
}
function hlcToPb(h: Hlc): pb.Hlc {
  return create(pb.HlcSchema, { wallMs: h.wall, logical: h.logical });
}
function hlcFromPb(p: pb.Hlc | undefined): Hlc {
  if (!p) throw new Error("missing hlc");
  if (p.logical > 0xffff) throw new Error("logical out of range");
  return { wall: p.wallMs, logical: p.logical };
}
function valueToPb(v: Value): pb.Value {
  switch (v.t) {
    case "null": return create(pb.ValueSchema, { kind: { case: "null", value: create(pb.NullSchema) } });
    case "bool": return create(pb.ValueSchema, { kind: { case: "bool", value: v.v } });
    case "i64": return create(pb.ValueSchema, { kind: { case: "i64", value: v.v } });
    case "f64": return create(pb.ValueSchema, { kind: { case: "f64", value: v.v } });
    case "str": return create(pb.ValueSchema, { kind: { case: "str", value: v.v } });
    case "color": return create(pb.ValueSchema, { kind: { case: "color", value: v.v >>> 0 } });
    case "frac": return create(pb.ValueSchema, { kind: { case: "fracIndex", value: v.v } });
    case "ref": return create(pb.ValueSchema, { kind: { case: "objRef", value: idToPb(v.v) } });
  }
}
function valueFromPb(p: pb.Value | undefined): Value {
  const k = p?.kind;
  if (!k || k.case === undefined) throw new Error("missing value");
  switch (k.case) {
    case "null": return { t: "null" };
    case "bool": return { t: "bool", v: k.value };
    case "i64": return { t: "i64", v: k.value };
    case "f64": return { t: "f64", v: k.value };
    case "str": return { t: "str", v: k.value };
    case "color": return { t: "color", v: k.value >>> 0 };
    case "fracIndex": return { t: "frac", v: k.value };
    case "objRef": return { t: "ref", v: idFromPb(k.value, "obj_ref") };
  }
}
function entriesToPb(e: ReadonlyArray<readonly [string, Value]>): pb.Entry[] {
  return e.map(([key, value]) => create(pb.EntrySchema, { key, value: valueToPb(value) }));
}
function entriesFromPb(e: pb.Entry[]): Array<readonly [string, Value]> {
  return e.map((en) => [en.key, valueFromPb(en.value)] as const);
}
const KIND_TO_PB: Record<EngineKind, pb.ObjectKind> = { rect: pb.ObjectKind.RECT, text: pb.ObjectKind.TEXT, group: pb.ObjectKind.GROUP, connector: pb.ObjectKind.CONNECTOR, ellipse: pb.ObjectKind.ELLIPSE, line: pb.ObjectKind.LINE };
function kindFromPb(k: pb.ObjectKind): EngineKind {
  switch (k) {
    case pb.ObjectKind.RECT: return "rect";
    case pb.ObjectKind.TEXT: return "text";
    case pb.ObjectKind.GROUP: return "group";
    case pb.ObjectKind.CONNECTOR: return "connector";
    case pb.ObjectKind.ELLIPSE: return "ellipse";
    case pb.ObjectKind.LINE: return "line";
    default: throw new Error("bad object kind");
  }
}

export function opToPb(op: Op): pb.Op {
  const k = op.kind;
  let kind: pb.Op["kind"];
  switch (k.op) {
    case "create": kind = { case: "create", value: create(pb.CreateSchema, { kind: KIND_TO_PB[k.kind], props: entriesToPb(k.props) }) }; break;
    case "set_props": kind = { case: "setProps", value: create(pb.SetPropsSchema, { object: idToPb(k.object), entries: entriesToPb(k.entries) }) }; break;
    case "delete": kind = { case: "delete", value: create(pb.DeleteSchema, { object: idToPb(k.object) }) }; break;
    case "restore": kind = { case: "restore", value: create(pb.RestoreSchema, { object: idToPb(k.object) }) }; break;
  }
  return create(pb.OpSchema, { id: idToPb(op.id), hlc: hlcToPb(op.hlc), kind });
}

export function opFromPb(p: pb.Op): Op {
  const k = p.kind;
  let kind: OpKind;
  switch (k.case) {
    case "create": kind = { op: "create", kind: kindFromPb(k.value.kind), props: entriesFromPb(k.value.props) }; break;
    case "setProps": kind = { op: "set_props", object: idFromPb(k.value.object, "object"), entries: entriesFromPb(k.value.entries) }; break;
    case "delete": kind = { op: "delete", object: idFromPb(k.value.object, "object") }; break;
    case "restore": kind = { op: "restore", object: idFromPb(k.value.object, "object") }; break;
    default: throw new Error("missing op kind");
  }
  return { id: idFromPb(p.id, "op.id"), hlc: hlcFromPb(p.hlc), kind };
}

export function encodeOp(op: Op): Uint8Array {
  return toBinary(pb.OpSchema, opToPb(op));
}
export function decodeOp(bytes: Uint8Array): Op {
  return opFromPb(fromBinary(pb.OpSchema, bytes));
}

// ----- protocol types -----

function userToPb(u: UserInfo): pb.UserInfo {
  return create(pb.UserInfoSchema, { name: u.name, color: u.color >>> 0 });
}
function userFromPb(p: pb.UserInfo | undefined): UserInfo {
  return p ? { name: p.name, color: p.color >>> 0 } : { name: "", color: 0 };
}
function presenceToPb(p: PresenceState): pb.PresenceState {
  return create(pb.PresenceStateSchema, {
    hasCursor: p.cursor !== null, cursorX: p.cursor?.[0] ?? 0, cursorY: p.cursor?.[1] ?? 0, selection: p.selection.map(idToPb),
  });
}
function presenceFromPb(p: pb.PresenceState | undefined): PresenceState {
  if (!p) return { cursor: null, selection: [] };
  return { cursor: p.hasCursor ? [p.cursorX, p.cursorY] : null, selection: p.selection.map((id) => idFromPb(id, "selection")) };
}
function entryToPb(e: PresenceEntry): pb.PresenceEntry {
  return create(pb.PresenceEntrySchema, { replica: e.replica, user: userToPb(e.user), state: presenceToPb(e.state) });
}
function entryFromPb(p: pb.PresenceEntry): PresenceEntry {
  return { replica: BigInt.asUintN(64, p.replica), user: userFromPb(p.user), state: presenceFromPb(p.state) };
}

export function encodeClient(m: ClientMsg): Uint8Array {
  let msg: pb.ClientMsg["msg"];
  switch (m.type) {
    case "hello":
      msg = { case: "hello", value: create(pb.HelloSchema, {
        version: m.hello.version, doc: m.hello.doc, replica: m.hello.replica, lastSeq: m.hello.lastSeq,
        wantSnapshot: m.hello.wantSnapshot, user: userToPb(m.hello.user),
      }) };
      break;
    case "submit": msg = { case: "submit", value: create(pb.SubmitSchema, { ops: m.ops.map(opToPb) }) }; break;
    case "presence": msg = { case: "presence", value: presenceToPb(m.state) }; break;
    case "pong": msg = { case: "pong", value: create(pb.PongSchema, { nonce: m.nonce }) }; break;
  }
  return toBinary(pb.ClientMsgSchema, create(pb.ClientMsgSchema, { msg }));
}

export function decodeClient(bytes: Uint8Array): ClientMsg {
  const p = fromBinary(pb.ClientMsgSchema, bytes);
  const m = p.msg;
  switch (m.case) {
    case "hello": return { type: "hello", hello: {
      version: m.value.version, doc: m.value.doc, replica: BigInt.asUintN(64, m.value.replica), lastSeq: m.value.lastSeq,
      wantSnapshot: m.value.wantSnapshot, user: userFromPb(m.value.user),
    } };
    case "submit": return { type: "submit", ops: m.value.ops.map(opFromPb) };
    case "presence": return { type: "presence", state: presenceFromPb(m.value) };
    case "pong": return { type: "pong", nonce: m.value.nonce };
    default: throw new Error("missing client message");
  }
}

const BYE_TO_PB: Record<ByeReason, pb.ByeReason> = {
  version_mismatch: pb.ByeReason.VERSION_MISMATCH, unknown_doc: pb.ByeReason.UNKNOWN_DOC, superseded: pb.ByeReason.SUPERSEDED,
  clock_skew: pb.ByeReason.CLOCK_SKEW, slow_consumer: pb.ByeReason.SLOW_CONSUMER, restart: pb.ByeReason.RESTART, protocol_error: pb.ByeReason.PROTOCOL_ERROR,
};
const BYE_FROM_PB = Object.fromEntries(Object.entries(BYE_TO_PB).map(([k, v]) => [v, k])) as Record<number, ByeReason>;
const NACK_TO_PB: Record<NackReason, pb.NackReason> = { clock_skew: pb.NackReason.CLOCK_SKEW, malformed: pb.NackReason.MALFORMED, rejected: pb.NackReason.REJECTED };
const NACK_FROM_PB = Object.fromEntries(Object.entries(NACK_TO_PB).map(([k, v]) => [v, k])) as Record<number, NackReason>;

function catchUpToPb(c: CatchUp): pb.Welcome["catchUp"] {
  return c.kind === "ops"
    ? { case: "ops", value: create(pb.CatchUpOpsSchema, { ops: c.ops.map((e) => create(pb.CommittedOpSchema, { seq: e.seq, op: opToPb(e.op) })) }) }
    : { case: "snapshot", value: create(pb.SnapshotSchema, { bytes: c.bytes, seq: c.seq }) };
}

export function encodeServer(m: ServerMsg): Uint8Array {
  let msg: pb.ServerMsg["msg"];
  switch (m.type) {
    case "welcome":
      msg = { case: "welcome", value: create(pb.WelcomeSchema, {
        session: m.session, serverTimeMs: m.serverTimeMs, durableHeadSeq: m.durableHeadSeq, catchUp: catchUpToPb(m.catchUp), presence: m.presence.map(entryToPb),
      }) };
      break;
    case "bye": msg = { case: "bye", value: create(pb.ByeSchema, { reason: BYE_TO_PB[m.reason] }) }; break;
    case "commit": msg = { case: "commit", value: create(pb.CommitSchema, { seq: m.seq, op: opToPb(m.op) }) }; break;
    case "ack": msg = { case: "ack", value: create(pb.AckSchema, m.seq === null ? { opId: idToPb(m.opId) } : { opId: idToPb(m.opId), seq: m.seq }) }; break;
    case "nack": msg = { case: "nack", value: create(pb.NackSchema, { opId: idToPb(m.opId), reason: NACK_TO_PB[m.reason] }) }; break;
    case "presence_update": msg = { case: "presenceUpdate", value: entryToPb(m.entry) }; break;
    case "presence_leave": msg = { case: "presenceLeave", value: create(pb.PresenceLeaveSchema, { replica: m.replica }) }; break;
    case "ping": msg = { case: "ping", value: create(pb.PingSchema, { nonce: m.nonce }) }; break;
  }
  return toBinary(pb.ServerMsgSchema, create(pb.ServerMsgSchema, { msg }));
}

export function decodeServer(bytes: Uint8Array): ServerMsg {
  const m = fromBinary(pb.ServerMsgSchema, bytes).msg;
  switch (m.case) {
    case "welcome": {
      const w = m.value;
      let catchUp: CatchUp;
      if (w.catchUp.case === "ops") {
        catchUp = { kind: "ops", ops: w.catchUp.value.ops.map((c) => { if (!c.op) throw new Error("missing op"); return { seq: c.seq, op: opFromPb(c.op) }; }) };
      } else if (w.catchUp.case === "snapshot") {
        catchUp = { kind: "snapshot", bytes: w.catchUp.value.bytes, seq: w.catchUp.value.seq };
      } else {
        throw new Error("missing catch_up");
      }
      return { type: "welcome", session: w.session, serverTimeMs: w.serverTimeMs, durableHeadSeq: w.durableHeadSeq, catchUp, presence: w.presence.map(entryFromPb) };
    }
    case "bye": {
      const r = BYE_FROM_PB[m.value.reason];
      if (!r) throw new Error("bad bye reason");
      return { type: "bye", reason: r };
    }
    case "commit": {
      if (!m.value.op) throw new Error("missing op");
      return { type: "commit", seq: m.value.seq, op: opFromPb(m.value.op) };
    }
    case "ack": return { type: "ack", opId: idFromPb(m.value.opId, "op_id"), seq: m.value.seq ?? null };
    case "nack": {
      const r = NACK_FROM_PB[m.value.reason];
      if (!r) throw new Error("bad nack reason");
      return { type: "nack", opId: idFromPb(m.value.opId, "op_id"), reason: r };
    }
    case "presenceUpdate": return { type: "presence_update", entry: entryFromPb(m.value) };
    case "presenceLeave": return { type: "presence_leave", replica: BigInt.asUintN(64, m.value.replica) };
    case "ping": return { type: "ping", nonce: m.value.nonce };
    default: throw new Error("missing server message");
  }
}

/** The `Codec` used by the WebSocket transport. */
export const wireCodec = { encodeClient, decodeServer };

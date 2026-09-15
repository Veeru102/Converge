/**
 * JSON form of ops shared with the Rust fixtures and the sim traces:
 * u64/i64 are decimal strings so that BigInt round-trips exactly.
 */
import type { Hlc } from "./hlc.js";
import type { OpId } from "./ids.js";
import type { Entries, Op, OpKind } from "./op.js";
import type { ObjectKind, Value } from "./value.js";

export interface IdJson {
  replica: string;
  counter: string;
}
export interface HlcJson {
  wall: string;
  logical: number;
}
export type ValueJson =
  | { t: "null" }
  | { t: "bool"; v: boolean }
  | { t: "i64"; v: string }
  | { t: "f64"; v: number | "NaN" | "Infinity" | "-Infinity" }
  | { t: "str"; v: string }
  | { t: "color"; v: number }
  | { t: "frac"; v: string }
  | { t: "ref"; v: IdJson };
export type EntriesJson = Array<[string, ValueJson]>;
export type OpKindJson =
  | { op: "create"; kind: ObjectKind; props: EntriesJson }
  | { op: "set_props"; object: IdJson; entries: EntriesJson }
  | { op: "delete"; object: IdJson }
  | { op: "restore"; object: IdJson };
export interface OpJson {
  id: IdJson;
  hlc: HlcJson;
  kind: OpKindJson;
}

export function idToJson(id: OpId): IdJson {
  return { replica: id.replica.toString(), counter: id.counter.toString() };
}
export function idFromJson(j: IdJson): OpId {
  return { replica: BigInt(j.replica), counter: BigInt(j.counter) };
}
export function hlcToJson(h: Hlc): HlcJson {
  return { wall: h.wall.toString(), logical: h.logical };
}
export function hlcFromJson(j: HlcJson): Hlc {
  return { wall: BigInt(j.wall), logical: j.logical };
}
export function valueToJson(v: Value): ValueJson {
  switch (v.t) {
    case "i64":
      return { t: "i64", v: v.v.toString() };
    case "ref":
      return { t: "ref", v: idToJson(v.v) };
    case "f64":
      if (Number.isFinite(v.v)) return { t: "f64", v: v.v };
      return { t: "f64", v: Number.isNaN(v.v) ? "NaN" : v.v > 0 ? "Infinity" : "-Infinity" };
    default:
      return v as ValueJson;
  }
}
export function valueFromJson(j: ValueJson): Value {
  switch (j.t) {
    case "i64":
      return { t: "i64", v: BigInt(j.v) };
    case "ref":
      return { t: "ref", v: idFromJson(j.v) };
    case "f64":
      if (typeof j.v === "string")
        return { t: "f64", v: j.v === "NaN" ? NaN : j.v === "Infinity" ? Infinity : -Infinity };
      return { t: "f64", v: j.v };
    default:
      return j;
  }
}
function entriesToJson(e: Entries): EntriesJson {
  return e.map(([k, v]) => [k, valueToJson(v)]);
}
function entriesFromJson(e: EntriesJson): Entries {
  return e.map(([k, v]) => [k, valueFromJson(v)] as const);
}
export function opToJson(op: Op): OpJson {
  const k = op.kind;
  let kind: OpKindJson;
  switch (k.op) {
    case "create":
      kind = { op: "create", kind: k.kind, props: entriesToJson(k.props) };
      break;
    case "set_props":
      kind = { op: "set_props", object: idToJson(k.object), entries: entriesToJson(k.entries) };
      break;
    case "delete":
      kind = { op: "delete", object: idToJson(k.object) };
      break;
    case "restore":
      kind = { op: "restore", object: idToJson(k.object) };
      break;
  }
  return { id: idToJson(op.id), hlc: hlcToJson(op.hlc), kind };
}
export function opFromJson(j: OpJson): Op {
  const k = j.kind;
  let kind: OpKind;
  switch (k.op) {
    case "create":
      kind = { op: "create", kind: k.kind, props: entriesFromJson(k.props) };
      break;
    case "set_props":
      kind = { op: "set_props", object: idFromJson(k.object), entries: entriesFromJson(k.entries) };
      break;
    case "delete":
      kind = { op: "delete", object: idFromJson(k.object) };
      break;
    case "restore":
      kind = { op: "restore", object: idFromJson(k.object) };
      break;
  }
  return { id: idFromJson(j.id), hlc: hlcFromJson(j.hlc), kind };
}

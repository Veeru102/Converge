/**
 * Canonical bytes, hash and snapshot format — byte-identical to the Rust
 * `codec` module. Little-endian, fixed-width integers.
 */
import { blake3 } from "@noble/hashes/blake3";
import { Document, ObjectState, type Register } from "./document.js";
import type { Hlc, Stamp } from "./hlc.js";
import { opId, type OpId } from "./ids.js";
import { KIND_TAG, TAG_KIND, VALUE_TAG, type Value } from "./value.js";

const SNAPSHOT_MAGIC = [0x43, 0x56, 0x47, 0x53]; // "CVGS"
const SNAPSHOT_VERSION = 1;
const utf8 = new TextEncoder();
const utf8d = new TextDecoder("utf-8", { fatal: true });

class Writer {
  private buf = new Uint8Array(1024);
  private view = new DataView(this.buf.buffer);
  private pos = 0;

  private ensure(n: number): void {
    if (this.pos + n <= this.buf.length) return;
    let cap = this.buf.length * 2;
    while (cap < this.pos + n) cap *= 2;
    const nb = new Uint8Array(cap);
    nb.set(this.buf.subarray(0, this.pos));
    this.buf = nb;
    this.view = new DataView(nb.buffer);
  }
  u8(v: number): void {
    this.ensure(1);
    this.view.setUint8(this.pos, v);
    this.pos += 1;
  }
  u16(v: number): void {
    this.ensure(2);
    this.view.setUint16(this.pos, v, true);
    this.pos += 2;
  }
  u32(v: number): void {
    this.ensure(4);
    this.view.setUint32(this.pos, v >>> 0, true);
    this.pos += 4;
  }
  u64(v: bigint): void {
    this.ensure(8);
    this.view.setBigUint64(this.pos, BigInt.asUintN(64, v), true);
    this.pos += 8;
  }
  i64(v: bigint): void {
    this.ensure(8);
    this.view.setBigInt64(this.pos, v, true);
    this.pos += 8;
  }
  f64(v: number): void {
    this.ensure(8);
    this.view.setFloat64(this.pos, v, true);
    this.pos += 8;
  }
  raw(b: Uint8Array): void {
    this.ensure(b.length);
    this.buf.set(b, this.pos);
    this.pos += b.length;
  }
  bytes(b: Uint8Array): void {
    this.u32(b.length);
    this.raw(b);
  }
  stamp(s: Stamp): void {
    this.u64(s.hlc.wall);
    this.u16(s.hlc.logical);
    this.u64(s.replica);
  }
  value(v: Value): void {
    this.u8(VALUE_TAG[v.t]);
    switch (v.t) {
      case "null":
        break;
      case "bool":
        this.u8(v.v ? 1 : 0);
        break;
      case "i64":
        this.i64(v.v);
        break;
      case "f64":
        this.f64(v.v);
        break;
      case "str":
      case "frac":
        this.bytes(utf8.encode(v.v));
        break;
      case "color":
        this.u32(v.v);
        break;
      case "ref":
        this.u64(v.v.replica);
        this.u64(v.v.counter);
        break;
    }
  }
  finish(): Uint8Array {
    return this.buf.slice(0, this.pos);
  }
}

class Reader {
  private view: DataView;
  pos = 0;
  constructor(
    private readonly buf: Uint8Array,
    start: number,
  ) {
    this.view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    this.pos = start;
  }
  private need(n: number): void {
    if (this.pos + n > this.buf.length) throw new Error("unexpected end of snapshot");
  }
  u8(): number {
    this.need(1);
    return this.view.getUint8(this.pos++);
  }
  u16(): number {
    this.need(2);
    const v = this.view.getUint16(this.pos, true);
    this.pos += 2;
    return v;
  }
  u32(): number {
    this.need(4);
    const v = this.view.getUint32(this.pos, true);
    this.pos += 4;
    return v;
  }
  u64(): bigint {
    this.need(8);
    const v = this.view.getBigUint64(this.pos, true);
    this.pos += 8;
    return v;
  }
  i64(): bigint {
    this.need(8);
    const v = this.view.getBigInt64(this.pos, true);
    this.pos += 8;
    return v;
  }
  f64(): number {
    this.need(8);
    const v = this.view.getFloat64(this.pos, true);
    this.pos += 8;
    return v;
  }
  string(): string {
    const n = this.u32();
    this.need(n);
    const s = utf8d.decode(this.buf.subarray(this.pos, this.pos + n));
    this.pos += n;
    return s;
  }
  stamp(): Stamp {
    const wall = this.u64();
    const logical = this.u16();
    const replica = this.u64();
    return { hlc: { wall, logical } satisfies Hlc, replica };
  }
  value(): Value {
    const tag = this.u8();
    switch (tag) {
      case 0:
        return { t: "null" };
      case 1:
        return { t: "bool", v: this.u8() !== 0 };
      case 2:
        return { t: "i64", v: this.i64() };
      case 3:
        return { t: "f64", v: this.f64() };
      case 4:
        return { t: "str", v: this.string() };
      case 5:
        return { t: "color", v: this.u32() };
      case 6:
        return { t: "frac", v: this.string() };
      case 7: {
        const r = this.u64();
        const c = this.u64();
        return { t: "ref", v: opId(r, c) };
      }
      default:
        throw new Error(`bad value tag ${tag}`);
    }
  }
}

export function canonicalBytes(doc: Document): Uint8Array {
  const w = new Writer();
  const entries = doc.entries();
  w.u64(BigInt(entries.length));
  for (const [id, obj] of entries) {
    w.u64(id.replica);
    w.u64(id.counter);
    w.u8(obj.kind === null ? 0 : KIND_TAG[obj.kind]);
    w.u8(obj.deleted.value ? 1 : 0);
    w.stamp(obj.deleted.at);
    const keys = [...obj.props.keys()].sort(); // ASCII keys: code-unit order == byte order
    w.u32(keys.length);
    for (const k of keys) {
      const reg = obj.props.get(k)!;
      w.bytes(utf8.encode(k));
      w.value(reg.value);
      w.stamp(reg.at);
    }
  }
  return w.finish();
}

export function hash(doc: Document): Uint8Array {
  return blake3(canonicalBytes(doc));
}

export function hex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

export function unhex(s: string): Uint8Array {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function hashHex(doc: Document): string {
  return hex(hash(doc));
}

export function encodeSnapshot(doc: Document): Uint8Array {
  const body = canonicalBytes(doc);
  const out = new Uint8Array(5 + body.length);
  out.set(SNAPSHOT_MAGIC, 0);
  out[4] = SNAPSHOT_VERSION;
  out.set(body, 5);
  return out;
}

export function decodeSnapshot(bytes: Uint8Array): Document {
  if (
    bytes.length < 5 ||
    SNAPSHOT_MAGIC.some((b, i) => bytes[i] !== b) ||
    bytes[4] !== SNAPSHOT_VERSION
  ) {
    throw new Error("bad snapshot header");
  }
  const r = new Reader(bytes, 5);
  const doc = new Document();
  const n = r.u64();
  for (let i = 0n; i < n; i++) {
    const replica = r.u64();
    const counter = r.u64();
    const kindTag = r.u8();
    const state = new ObjectState();
    if (kindTag !== 0) {
      const kind = TAG_KIND[kindTag];
      if (!kind) throw new Error(`bad kind tag ${kindTag}`);
      state.kind = kind;
    }
    const deletedV = r.u8() !== 0;
    const deletedAt = r.stamp();
    state.deleted = { value: deletedV, at: deletedAt };
    const np = r.u32();
    for (let p = 0; p < np; p++) {
      const key = r.string();
      const value = r.value();
      const at = r.stamp();
      const reg: Register<Value> = { value, at };
      state.props.set(key, reg);
    }
    doc.insertRaw({ replica, counter } satisfies OpId, state);
  }
  if (r.pos !== bytes.length) throw new Error("trailing bytes in snapshot");
  return doc;
}

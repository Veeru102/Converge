import type { Hlc, Stamp } from "./hlc.js";
import type { ObjectId, OpId } from "./ids.js";
import type { ObjectKind, Value } from "./value.js";
import { validate as validateFrac } from "./fracindex.js";

export const MAX_PROPS_PER_OP = 64;
export const MAX_STRING_BYTES = 32 * 1024;
export const MAX_KEY_BYTES = 64;

export type Entries = ReadonlyArray<readonly [string, Value]>;

export type OpKind =
  | { readonly op: "create"; readonly kind: ObjectKind; readonly props: Entries }
  | { readonly op: "set_props"; readonly object: ObjectId; readonly entries: Entries }
  | { readonly op: "delete"; readonly object: ObjectId }
  | { readonly op: "restore"; readonly object: ObjectId };

export interface Op {
  readonly id: OpId;
  readonly hlc: Hlc;
  readonly kind: OpKind;
}

export function opObject(op: Op): ObjectId {
  return op.kind.op === "create" ? op.id : op.kind.object;
}

export function opStamp(op: Op): Stamp {
  return { hlc: op.hlc, replica: op.id.replica };
}

export function opEntries(op: Op): Entries {
  switch (op.kind.op) {
    case "create":
      return op.kind.props;
    case "set_props":
      return op.kind.entries;
    default:
      return [];
  }
}

const KEY_RE = /^[A-Za-z0-9_.-]{1,64}$/;

export function isValidKey(key: string): boolean {
  return KEY_RE.test(key);
}

const utf8 = new TextEncoder();

/** Shape validation; mirrors `Op::validate` in Rust. Returns an error message or null. */
export function validateOp(op: Op): string | null {
  const entries = opEntries(op);
  if (entries.length > MAX_PROPS_PER_OP) return `too many props (${entries.length})`;
  const seen = new Set<string>();
  for (const [key, value] of entries) {
    if (!isValidKey(key)) return `bad key ${JSON.stringify(key)}`;
    if (seen.has(key)) return `duplicate key ${key}`;
    seen.add(key);
    switch (value.t) {
      case "f64":
        if (!Number.isFinite(value.v)) return `non-finite float for ${key}`;
        break;
      case "str":
        if (utf8.encode(value.v).length > MAX_STRING_BYTES) return `string too long for ${key}`;
        break;
      case "frac":
        if (!validateFrac(value.v)) return `bad fractional index for ${key}`;
        break;
      default:
        break;
    }
  }
  return null;
}

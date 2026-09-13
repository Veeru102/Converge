import type { ObjectId } from "./ids.js";

export type ObjectKind = "rect" | "text" | "group" | "connector";

export const KIND_TAG: Record<ObjectKind, number> = { rect: 1, text: 2, group: 3, connector: 4 };
export const TAG_KIND: Record<number, ObjectKind> = { 1: "rect", 2: "text", 3: "group", 4: "connector" };

export type Value =
  | { readonly t: "null" }
  | { readonly t: "bool"; readonly v: boolean }
  | { readonly t: "i64"; readonly v: bigint }
  | { readonly t: "f64"; readonly v: number }
  | { readonly t: "str"; readonly v: string }
  | { readonly t: "color"; readonly v: number }
  | { readonly t: "frac"; readonly v: string }
  | { readonly t: "ref"; readonly v: ObjectId };

export const VALUE_TAG: Record<Value["t"], number> = {
  null: 0, bool: 1, i64: 2, f64: 3, str: 4, color: 5, frac: 6, ref: 7,
};

export const V = {
  null: (): Value => ({ t: "null" }),
  bool: (v: boolean): Value => ({ t: "bool", v }),
  i64: (v: bigint): Value => ({ t: "i64", v }),
  f64: (v: number): Value => ({ t: "f64", v }),
  str: (v: string): Value => ({ t: "str", v }),
  color: (v: number): Value => ({ t: "color", v }),
  frac: (v: string): Value => ({ t: "frac", v }),
  ref: (v: ObjectId): Value => ({ t: "ref", v }),
};

export function valueAsNumber(v: Value | undefined): number | undefined {
  if (!v) return undefined;
  if (v.t === "f64") return v.v;
  if (v.t === "i64") return Number(v.v);
  return undefined;
}

/** Packed 0xRRGGBB (or 0xAARRGGBB) colour; `undefined` for any other kind. */
export function valueAsColor(v: Value | undefined): number | undefined {
  return v?.t === "color" ? v.v : undefined;
}

export function valueAsBool(v: Value | undefined): boolean | undefined {
  return v?.t === "bool" ? v.v : undefined;
}

export function valueAsString(v: Value | undefined): string | undefined {
  if (!v) return undefined;
  if (v.t === "str" || v.t === "frac") return v.v;
  return undefined;
}

export function valueEquals(a: Value, b: Value): boolean {
  if (a.t !== b.t) return false;
  switch (a.t) {
    case "null":
      return true;
    case "ref":
      return a.v.replica === (b as typeof a).v.replica && a.v.counter === (b as typeof a).v.counter;
    case "f64":
      return Object.is(a.v, (b as typeof a).v);
    default:
      return a.v === (b as { v: unknown }).v;
  }
}

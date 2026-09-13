/** Reading the prop vocabulary (ARCHITECTURE.md §3.1) off engine objects. */
import { valueAsBool, valueAsColor, valueAsNumber, valueAsString, type ObjectId, type ObjectState, type Value } from "@converge/engine";

export interface Box { x: number; y: number; w: number; h: number }

export const num = (o: ObjectState, k: string, d = 0): number => valueAsNumber(o.get(k)) ?? d;
export const str = (o: ObjectState, k: string, d = ""): string => valueAsString(o.get(k)) ?? d;
export const color = (o: ObjectState, k: string): number | undefined => valueAsColor(o.get(k));
export const bool = (o: ObjectState, k: string, d = false): boolean => valueAsBool(o.get(k)) ?? d;

export const f64 = (v: number): Value => ({ t: "f64", v });
export const colorV = (v: number): Value => ({ t: "color", v: v >>> 0 });
export const strV = (v: string): Value => ({ t: "str", v });
export const boolV = (v: boolean): Value => ({ t: "bool", v });
export const fracV = (v: string): Value => ({ t: "frac", v });

export function hexOf(c: number | undefined, fallback: string): string {
  return c === undefined ? fallback : `#${(c & 0xffffff).toString(16).padStart(6, "0")}`;
}
export function parseHex(s: string): number | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(s.trim());
  return m ? parseInt(m[1]!, 16) : null;
}

export const TEXT_LINE = 1.25;

export function textLines(o: ObjectState): string[] {
  return str(o, "text").split("\n");
}

/** Approximate text extent when no explicit width is stored. */
export function textBox(o: ObjectState): Box {
  const size = num(o, "size", 18);
  const lines = textLines(o);
  const longest = Math.max(1, ...lines.map((l) => l.length));
  const w = num(o, "w", 0) || Math.max(24, longest * size * 0.58 + 8);
  return { x: num(o, "x"), y: num(o, "y"), w, h: Math.max(1, lines.length) * size * TEXT_LINE };
}

export function bounds(o: ObjectState): Box {
  switch (o.kind) {
    case "text":
      return textBox(o);
    case "line": {
      const x1 = num(o, "x1"), y1 = num(o, "y1"), x2 = num(o, "x2"), y2 = num(o, "y2");
      return { x: Math.min(x1, x2), y: Math.min(y1, y2), w: Math.abs(x2 - x1), h: Math.abs(y2 - y1) };
    }
    default:
      return { x: num(o, "x"), y: num(o, "y"), w: num(o, "w", 100), h: num(o, "h", 60) };
  }
}

export function union(boxes: Box[]): Box | null {
  if (boxes.length === 0) return null;
  let x1 = Infinity, y1 = Infinity, x2 = -Infinity, y2 = -Infinity;
  for (const b of boxes) {
    x1 = Math.min(x1, b.x); y1 = Math.min(y1, b.y); x2 = Math.max(x2, b.x + b.w); y2 = Math.max(y2, b.y + b.h);
  }
  return { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
}

export function intersects(a: Box, b: Box): boolean {
  return a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
}

export function normBox(x1: number, y1: number, x2: number, y2: number): Box {
  return { x: Math.min(x1, x2), y: Math.min(y1, y2), w: Math.abs(x2 - x1), h: Math.abs(y2 - y1) };
}

export type Entry = [ObjectId, ObjectState];

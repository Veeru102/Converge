/** Every canvas edit is an ordinary op through `Client.edit`; nothing here is special-cased by the engine. */
import {
  fracindex,
  idKey,
  type Document,
  type ObjectId,
  type ObjectKind,
  type ObjectState,
  type Value,
} from "@converge/engine";
import type { Client } from "@converge/sync";
import {
  boolV,
  bounds,
  colorV,
  f64,
  fracV,
  num,
  str,
  strV,
  type Box,
  type Entry,
} from "./model.js";

export type Tool = "select" | "rect" | "ellipse" | "line" | "text";

const DEFAULT_TEXT_SIZE = 18;

export interface Style {
  fill: number;
  stroke: number;
  strokeWidth: number;
  opacity: number;
}

function zAbove(doc: Document): string {
  const order = doc.renderOrder();
  const top = order.length ? str(order[order.length - 1]![1], "z") || null : null;
  return fracindex.between(top, null);
}

export function createShape(
  client: Client,
  kind: Exclude<Tool, "select">,
  box: Box,
  style: Style,
  textColor = 0x17171c,
): ObjectId {
  const doc = client.document;
  const z = fracV(zAbove(doc));
  let props: Array<[string, Value]>;
  switch (kind) {
    case "rect":
    case "ellipse":
      props = [
        ["x", f64(box.x)],
        ["y", f64(box.y)],
        ["w", f64(box.w)],
        ["h", f64(box.h)],
        ["fill", colorV(style.fill)],
        ["stroke", colorV(style.stroke)],
        ["stroke_width", f64(style.strokeWidth)],
        ["opacity", f64(style.opacity)],
        ["z", z],
      ];
      break;
    case "line":
      props = [
        ["x1", f64(box.x)],
        ["y1", f64(box.y)],
        ["x2", f64(box.x + box.w)],
        ["y2", f64(box.y + box.h)],
        ["stroke", colorV(style.stroke)],
        ["stroke_width", f64(Math.max(2, style.strokeWidth))],
        ["arrow", boolV(true)],
        ["opacity", f64(style.opacity)],
        ["z", z],
      ];
      break;
    case "text":
      props = [
        ["x", f64(box.x)],
        ["y", f64(box.y)],
        ["text", strV("")],
        ["size", f64(DEFAULT_TEXT_SIZE)],
        ["color", colorV(textColor)],
        ["opacity", f64(style.opacity)],
        ["z", z],
      ];
      break;
  }
  return client.edit({ op: "create", kind: kind as ObjectKind, props }).id;
}

export function setGeometry(client: Client, id: ObjectId, o: ObjectState, box: Box): void {
  if (o.kind === "line") {
    // Keep the line's direction; the box is the normalised extent.
    const flipX = num(o, "x1") > num(o, "x2"),
      flipY = num(o, "y1") > num(o, "y2");
    const x1 = flipX ? box.x + box.w : box.x,
      x2 = flipX ? box.x : box.x + box.w;
    const y1 = flipY ? box.y + box.h : box.y,
      y2 = flipY ? box.y : box.y + box.h;
    client.edit({
      op: "set_props",
      object: id,
      entries: [
        ["x1", f64(x1)],
        ["y1", f64(y1)],
        ["x2", f64(x2)],
        ["y2", f64(y2)],
      ],
    });
  } else if (o.kind === "text") {
    client.edit({
      op: "set_props",
      object: id,
      entries: [
        ["x", f64(box.x)],
        ["y", f64(box.y)],
        ["w", f64(box.w)],
      ],
    });
  } else {
    client.edit({
      op: "set_props",
      object: id,
      entries: [
        ["x", f64(box.x)],
        ["y", f64(box.y)],
        ["w", f64(box.w)],
        ["h", f64(box.h)],
      ],
    });
  }
}

export function setLineEnd(client: Client, id: ObjectId, which: 1 | 2, x: number, y: number): void {
  client.edit({
    op: "set_props",
    object: id,
    entries: [
      [`x${which}`, f64(x)],
      [`y${which}`, f64(y)],
    ],
  });
}

/** Move a set of objects by a delta from their positions in `origin`. */
export function moveTo(
  client: Client,
  items: Array<{ id: ObjectId; o: ObjectState; origin: Box }>,
  dx: number,
  dy: number,
): void {
  for (const { id, o, origin } of items) {
    if (o.kind === "line") {
      const ox1 = num(o, "x1"),
        oy1 = num(o, "y1"),
        ox2 = num(o, "x2"),
        oy2 = num(o, "y2");
      const sx = origin.x - Math.min(ox1, ox2),
        sy = origin.y - Math.min(oy1, oy2);
      client.edit({
        op: "set_props",
        object: id,
        entries: [
          ["x1", f64(ox1 + sx + dx)],
          ["y1", f64(oy1 + sy + dy)],
          ["x2", f64(ox2 + sx + dx)],
          ["y2", f64(oy2 + sy + dy)],
        ],
      });
    } else {
      client.edit({
        op: "set_props",
        object: id,
        entries: [
          ["x", f64(origin.x + dx)],
          ["y", f64(origin.y + dy)],
        ],
      });
    }
  }
}

export function nudge(client: Client, entries: Entry[], dx: number, dy: number): void {
  moveTo(
    client,
    entries.map(([id, o]) => ({ id, o, origin: bounds(o) })),
    dx,
    dy,
  );
}

export function setProps(client: Client, ids: ObjectId[], entries: Array<[string, Value]>): void {
  for (const id of ids) client.edit({ op: "set_props", object: id, entries });
}

export function setText(client: Client, id: ObjectId, text: string): void {
  client.edit({ op: "set_props", object: id, entries: [["text", strV(text)]] });
}

export function deleteMany(client: Client, ids: ObjectId[]): void {
  for (const id of ids) client.edit({ op: "delete", object: id });
}

export function duplicate(client: Client, entries: Entry[], offset = 16): ObjectId[] {
  const doc = client.document;
  let z = zAbove(doc);
  const out: ObjectId[] = [];
  for (const [, o] of entries) {
    if (!o.kind) continue;
    const props: Array<[string, Value]> = [];
    for (const [k, reg] of o.props) {
      let v = reg.value;
      if (v.t === "f64" && (k === "x" || k === "x1" || k === "x2")) v = f64(v.v + offset);
      if (v.t === "f64" && (k === "y" || k === "y1" || k === "y2")) v = f64(v.v + offset);
      if (k === "z") continue;
      props.push([k, v]);
    }
    props.push(["z", fracV(z)]);
    z = fracindex.between(z, null);
    out.push(client.edit({ op: "create", kind: o.kind, props }).id);
  }
  return out;
}

export type ZMove = "front" | "back" | "forward" | "backward";

/** Re-key `ids` in the render order using fractional indices between neighbours. */
export function reorder(client: Client, ids: ObjectId[], move: ZMove): void {
  const order = client.document.renderOrder();
  const keys = new Set(ids.map(idKey));
  const zs = order.map(([, o]) => str(o, "z") || null);
  const idxs = order.map((_, i) => i).filter((i) => keys.has(idKey(order[i]![0])));
  if (idxs.length === 0) return;
  const assign = (i: number, lo: string | null, hi: string | null) => {
    const z = fracindex.between(lo, hi);
    client.edit({ op: "set_props", object: order[i]![0], entries: [["z", fracV(z)]] });
    return z;
  };
  switch (move) {
    case "front": {
      let lo = zs[zs.length - 1]!;
      for (const i of idxs) lo = assign(i, lo, null);
      break;
    }
    case "back": {
      let hi = zs.find((z) => z !== null) ?? null;
      for (const i of [...idxs].reverse()) hi = assign(i, null, hi);
      break;
    }
    case "forward": {
      // Move each selected item just past the next unselected neighbour.
      for (const i of [...idxs].reverse()) {
        let j = i + 1;
        while (j < order.length && keys.has(idKey(order[j]![0]))) j++;
        if (j >= order.length) continue;
        const after = j + 1 < order.length ? zs[j + 1]! : null;
        assign(i, zs[j]!, after);
      }
      break;
    }
    case "backward": {
      for (const i of idxs) {
        let j = i - 1;
        while (j >= 0 && keys.has(idKey(order[j]![0]))) j--;
        if (j < 0) continue;
        const before = j - 1 >= 0 ? zs[j - 1]! : null;
        assign(i, before, zs[j]!);
      }
      break;
    }
  }
}

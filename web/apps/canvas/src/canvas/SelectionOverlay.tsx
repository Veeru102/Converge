import type { ObjectState } from "@converge/engine";
import { bounds, num, union, type Box } from "../state/model.js";

export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

function handlePoint(b: Box, h: Handle): [number, number] {
  const cx = b.x + b.w / 2,
    cy = b.y + b.h / 2;
  switch (h) {
    case "nw":
      return [b.x, b.y];
    case "n":
      return [cx, b.y];
    case "ne":
      return [b.x + b.w, b.y];
    case "e":
      return [b.x + b.w, cy];
    case "se":
      return [b.x + b.w, b.y + b.h];
    case "s":
      return [cx, b.y + b.h];
    case "sw":
      return [b.x, b.y + b.h];
    case "w":
      return [b.x, cy];
  }
}

const CURSORS: Record<Handle, string> = {
  nw: "nwse-resize",
  se: "nwse-resize",
  ne: "nesw-resize",
  sw: "nesw-resize",
  n: "ns-resize",
  s: "ns-resize",
  e: "ew-resize",
  w: "ew-resize",
};

interface Props {
  selected: ObjectState[];
  scale: number;
  interactive: boolean;
  onHandleDown: (e: React.PointerEvent, h: Handle) => void;
  onLineEndDown: (e: React.PointerEvent, which: 1 | 2) => void;
}

/** Accent outline, resize handles for a single object, union box for many. */
export function SelectionOverlay({
  selected,
  scale,
  interactive,
  onHandleDown,
  onLineEndDown,
}: Props) {
  if (selected.length === 0) return null;
  const s = 1 / scale;
  const hs = 8 * s;
  const stroke = "var(--accent)";
  if (selected.length === 1) {
    const o = selected[0]!;
    if (o.kind === "line") {
      const pts: Array<[1 | 2, number, number]> = [
        [1, num(o, "x1"), num(o, "y1")],
        [2, num(o, "x2"), num(o, "y2")],
      ];
      return (
        <g data-testid="selection" pointerEvents={interactive ? "auto" : "none"}>
          {pts.map(([which, x, y]) => (
            <circle
              key={which}
              cx={x}
              cy={y}
              r={hs * 0.7}
              fill="#fff"
              stroke={stroke}
              strokeWidth={1.5 * s}
              style={{ cursor: "crosshair" }}
              onPointerDown={(e) => onLineEndDown(e, which)}
            />
          ))}
        </g>
      );
    }
    const b = bounds(o);
    const handles: Handle[] = o.kind === "text" ? ["e", "w"] : HANDLES;
    return (
      <g data-testid="selection" pointerEvents={interactive ? "auto" : "none"}>
        <rect
          x={b.x}
          y={b.y}
          width={b.w}
          height={b.h}
          fill="none"
          stroke={stroke}
          strokeWidth={1.5 * s}
          pointerEvents="none"
        />
        {handles.map((h) => {
          const [x, y] = handlePoint(b, h);
          return (
            <rect
              key={h}
              x={x - hs / 2}
              y={y - hs / 2}
              width={hs}
              height={hs}
              fill="#fff"
              stroke={stroke}
              strokeWidth={1.5 * s}
              style={{ cursor: CURSORS[h] }}
              onPointerDown={(e) => onHandleDown(e, h)}
              data-handle={h}
            />
          );
        })}
      </g>
    );
  }
  const all = union(selected.map(bounds))!;
  return (
    <g data-testid="selection" pointerEvents="none">
      {selected.map((o, i) => {
        const b = bounds(o);
        return (
          <rect
            key={i}
            x={b.x}
            y={b.y}
            width={b.w}
            height={b.h}
            fill="none"
            stroke={stroke}
            strokeWidth={1 * s}
            opacity={0.6}
          />
        );
      })}
      <rect
        x={all.x}
        y={all.y}
        width={all.w}
        height={all.h}
        fill="none"
        stroke={stroke}
        strokeWidth={1.5 * s}
        strokeDasharray={`${4 * s} ${3 * s}`}
      />
    </g>
  );
}

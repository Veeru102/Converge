import { memo } from "react";
import type { ObjectId, ObjectState } from "@converge/engine";
import { bool, color, hexOf, num, textBox, textLines, TEXT_LINE } from "../state/model.js";

interface Props {
  id: ObjectId;
  o: ObjectState;
  interactive: boolean;
  editing: boolean;
  onPointerDown: (e: React.PointerEvent, id: ObjectId) => void;
  onDoubleClick: (e: React.MouseEvent, id: ObjectId) => void;
}

function arrowHead(x1: number, y1: number, x2: number, y2: number, size: number): string {
  const a = Math.atan2(y2 - y1, x2 - x1);
  const p = (ang: number, r: number) => `${x2 - Math.cos(ang) * r},${y2 - Math.sin(ang) * r}`;
  return `${x2},${y2} ${p(a - 0.45, size)} ${p(a + 0.45, size)}`;
}

export const ShapeView = memo(function ShapeView({
  id,
  o,
  interactive,
  editing,
  onPointerDown,
  onDoubleClick,
}: Props) {
  const opacity = num(o, "opacity", 1);
  const common = {
    onPointerDown: (e: React.PointerEvent) => onPointerDown(e, id),
    onDoubleClick: (e: React.MouseEvent) => onDoubleClick(e, id),
    style: { pointerEvents: interactive ? ("auto" as const) : ("none" as const), cursor: "move" },
    "data-testid": "shape",
    "data-kind": o.kind ?? "",
    opacity,
  };
  switch (o.kind) {
    case "ellipse": {
      const x = num(o, "x"),
        y = num(o, "y"),
        w = num(o, "w", 100),
        h = num(o, "h", 60);
      return (
        <g {...common}>
          <ellipse
            cx={x + w / 2}
            cy={y + h / 2}
            rx={w / 2}
            ry={h / 2}
            fill={hexOf(color(o, "fill"), "#c7d2fe")}
            stroke={hexOf(color(o, "stroke"), "none")}
            strokeWidth={num(o, "stroke_width", 0)}
          />
        </g>
      );
    }
    case "line": {
      const x1 = num(o, "x1"),
        y1 = num(o, "y1"),
        x2 = num(o, "x2"),
        y2 = num(o, "y2");
      const sw = num(o, "stroke_width", 2);
      const stroke = hexOf(color(o, "stroke"), "#2b2b33");
      return (
        <g {...common}>
          <line
            x1={x1}
            y1={y1}
            x2={x2}
            y2={y2}
            stroke="transparent"
            strokeWidth={Math.max(12, sw + 8)}
          />
          <line
            x1={x1}
            y1={y1}
            x2={x2}
            y2={y2}
            stroke={stroke}
            strokeWidth={sw}
            strokeLinecap="round"
          />
          {bool(o, "arrow") && (
            <polygon points={arrowHead(x1, y1, x2, y2, 6 + sw * 2)} fill={stroke} />
          )}
        </g>
      );
    }
    case "text": {
      const box = textBox(o);
      const size = num(o, "size", 18);
      const lines = textLines(o);
      const fill = hexOf(color(o, "color"), "#17171c");
      return (
        <g {...common}>
          <rect x={box.x} y={box.y} width={box.w} height={box.h} fill="transparent" />
          {!editing && (
            <text
              x={box.x}
              y={box.y}
              fontSize={size}
              fill={fill}
              fontFamily="Inter, system-ui, sans-serif"
              style={{ whiteSpace: "pre" }}
            >
              {lines.map((l, i) => (
                <tspan key={i} x={box.x} y={box.y + size * 0.8 + i * size * TEXT_LINE}>
                  {l || " "}
                </tspan>
              ))}
            </text>
          )}
        </g>
      );
    }
    default: {
      const x = num(o, "x"),
        y = num(o, "y"),
        w = num(o, "w", 100),
        h = num(o, "h", 60);
      return (
        <g {...common}>
          <rect
            x={x}
            y={y}
            width={w}
            height={h}
            rx={3}
            fill={hexOf(color(o, "fill"), "#c7d2fe")}
            stroke={hexOf(color(o, "stroke"), "none")}
            strokeWidth={num(o, "stroke_width", 0)}
          />
        </g>
      );
    }
  }
});

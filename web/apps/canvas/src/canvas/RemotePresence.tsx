import { useEffect, useRef } from "react";
import { idKey, type Document } from "@converge/engine";
import type { PresenceEntry } from "@converge/protocol";
import { bounds, hexOf } from "../state/model.js";

/** Remote selections in each user's colour with a name tag. */
export function RemoteSelections({
  presence,
  doc,
  scale,
}: {
  presence: PresenceEntry[];
  doc: Document;
  scale: number;
}) {
  const s = 1 / scale;
  return (
    <g pointerEvents="none">
      {presence.flatMap((p) =>
        p.state.selection.map((id) => {
          const o = doc.get(id);
          if (!o || !o.visible()) return null;
          const b = bounds(o);
          const c = hexOf(p.user.color, "#888");
          return (
            <g key={`${p.replica}-${idKey(id)}`}>
              <rect
                x={b.x}
                y={b.y}
                width={b.w}
                height={b.h}
                fill="none"
                stroke={c}
                strokeWidth={1.5 * s}
              />
              <g transform={`translate(${b.x} ${b.y - 18 * s}) scale(${s})`}>
                <rect width={p.user.name.length * 6.6 + 10} height={16} rx={3} fill={c} />
                <text
                  x={5}
                  y={11.5}
                  fontSize={11}
                  fill="#fff"
                  fontFamily="Inter, system-ui, sans-serif"
                  fontWeight={500}
                >
                  {p.user.name}
                </text>
              </g>
            </g>
          );
        }),
      )}
    </g>
  );
}

/** Remote cursors, interpolated towards their latest position at ~60 fps outside React. */
export function RemoteCursors({ presence, scale }: { presence: PresenceEntry[]; scale: number }) {
  const nodes = useRef(new Map<string, SVGGElement>());
  const targets = useRef(new Map<string, { x: number; y: number }>());
  const shown = useRef(new Map<string, { x: number; y: number }>());

  for (const p of presence) {
    if (p.state.cursor)
      targets.current.set(p.replica.toString(), { x: p.state.cursor[0], y: p.state.cursor[1] });
  }

  useEffect(() => {
    let raf = 0;
    const step = () => {
      for (const [k, t] of targets.current) {
        const cur = shown.current.get(k) ?? t;
        const nx = cur.x + (t.x - cur.x) * 0.35,
          ny = cur.y + (t.y - cur.y) * 0.35;
        shown.current.set(k, { x: nx, y: ny });
        nodes.current
          .get(k)
          ?.setAttribute("transform", `translate(${nx} ${ny}) scale(${1 / scale})`);
      }
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [scale]);

  return (
    <g pointerEvents="none">
      {presence.map((p) => {
        if (!p.state.cursor) return null;
        const k = p.replica.toString();
        const c = hexOf(p.user.color, "#888");
        const start = shown.current.get(k) ?? { x: p.state.cursor[0], y: p.state.cursor[1] };
        return (
          <g
            key={k}
            data-testid="remote-cursor"
            transform={`translate(${start.x} ${start.y}) scale(${1 / scale})`}
            ref={(el) => {
              if (el) nodes.current.set(k, el);
              else nodes.current.delete(k);
            }}
          >
            <path
              d="M0 0 L0 16 L4.5 12.5 L8 19 L10.5 18 L7 11.5 L12.5 11.5 Z"
              fill={c}
              stroke="#fff"
              strokeWidth={1}
            />
            <rect x={13} y={12} width={p.user.name.length * 6.6 + 10} height={16} rx={3} fill={c} />
            <text
              x={18}
              y={23.5}
              fontSize={11}
              fill="#fff"
              fontFamily="Inter, system-ui, sans-serif"
              fontWeight={500}
            >
              {p.user.name}
            </text>
          </g>
        );
      })}
    </g>
  );
}

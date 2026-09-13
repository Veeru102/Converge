import { useEffect, useMemo, useRef, useState } from "react";
import { fracindex, idEquals, idKey, valueAsNumber, valueAsString, type Document, type ObjectId, type ObjectState, type Op } from "@converge/engine";
import type { PresenceEntry } from "@converge/protocol";
import type { Client, ClientStatus } from "@converge/sync";
import { startSession, type Session } from "./session.js";
import { DebugPanel } from "./DebugPanel.js";

const colorHex = (c: number | undefined, fallback: string) => (c === undefined ? fallback : `#${(c & 0xffffff).toString(16).padStart(6, "0")}`);

export function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    startSession().then(setSession, (e) => setError(String(e)));
  }, []);
  if (error) return <div style={{ padding: 24 }}>Failed to start: {error}</div>;
  if (!session) return <div style={{ padding: 24 }}>Loading…</div>;
  return <Canvas session={session} />;
}

function useClientState(client: Client) {
  const [, force] = useState(0);
  const [status, setStatus] = useState<ClientStatus>(() => client.status());
  const [presence, setPresence] = useState<PresenceEntry[]>([]);
  useEffect(() => {
    const a = client.onChange(() => force((n) => n + 1));
    const b = client.onStatus(setStatus);
    const c = client.onPresence(setPresence);
    return () => {
      a();
      b();
      c();
    };
  }, [client]);
  return { status, presence };
}

function Canvas({ session }: { session: Session }) {
  const { client } = session;
  const { status, presence } = useClientState(client);
  const [selected, setSelected] = useState<ObjectId | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const drag = useRef<{ id: ObjectId; dx: number; dy: number; last: number } | null>(null);
  const doc: Document = client.document;
  const objects = doc.renderOrder();
  const topZ = useMemo(() => {
    let z: string | null = null;
    for (const [, o] of objects) {
      const v = valueAsString(o.get("z"));
      if (v && (z === null || v > z)) z = v;
    }
    return z;
  }, [objects]);

  const nextZ = () => fracindex.between(topZ, null);

  const addRect = () => {
    const op = client.edit({
      op: "create", kind: "rect",
      props: [["x", { t: "f64", v: 40 + Math.random() * 400 }], ["y", { t: "f64", v: 40 + Math.random() * 300 }], ["w", { t: "f64", v: 120 }], ["h", { t: "f64", v: 80 }], ["fill", { t: "color", v: session.color }], ["z", { t: "frac", v: nextZ() }]],
    });
    setSelected(op.id);
  };
  const addText = () => {
    const op = client.edit({
      op: "create", kind: "text",
      props: [["x", { t: "f64", v: 40 + Math.random() * 400 }], ["y", { t: "f64", v: 40 + Math.random() * 300 }], ["text", { t: "str", v: "Text" }], ["size", { t: "f64", v: 18 }], ["color", { t: "color", v: 0x222222 }], ["z", { t: "frac", v: nextZ() }]],
    });
    setSelected(op.id);
  };
  const del = () => {
    if (selected) client.edit({ op: "delete", object: selected });
    setSelected(null);
  };
  const front = () => {
    if (selected) client.edit({ op: "set_props", object: selected, entries: [["z", { t: "frac", v: nextZ() }]] });
  };
  const recolor = () => {
    if (selected) client.edit({ op: "set_props", object: selected, entries: [["fill", { t: "color", v: Math.floor(Math.random() * 0xffffff) }]] });
  };
  const editText = () => {
    if (!selected) return;
    const cur = valueAsString(doc.get(selected)?.get("text")) ?? "";
    const next = prompt("Text", cur);
    if (next !== null) client.edit({ op: "set_props", object: selected, entries: [["text", { t: "str", v: next }]] });
  };

  const svgPoint = (e: React.PointerEvent) => {
    const r = svgRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const onPointerDown = (e: React.PointerEvent, id: ObjectId, o: ObjectState) => {
    e.stopPropagation();
    setSelected(id);
    const p = svgPoint(e);
    drag.current = { id, dx: p.x - (valueAsNumber(o.get("x")) ?? 0), dy: p.y - (valueAsNumber(o.get("y")) ?? 0), last: 0 };
    (e.target as Element).setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const p = svgPoint(e);
    client.sendPresence({ cursor: [p.x, p.y], selection: selected ? [selected] : [] });
    const d = drag.current;
    if (!d) return;
    const now = performance.now();
    if (now - d.last < 33) return; // ~30 Hz while dragging
    d.last = now;
    client.edit({ op: "set_props", object: d.id, entries: [["x", { t: "f64", v: p.x - d.dx }], ["y", { t: "f64", v: p.y - d.dy }]] });
  };
  const onPointerUp = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    const p = svgPoint(e);
    client.edit({ op: "set_props", object: d.id, entries: [["x", { t: "f64", v: p.x - d.dx }], ["y", { t: "f64", v: p.y - d.dy }]] });
  };

  const remoteSelections = new Map<string, PresenceEntry[]>();
  for (const p of presence) for (const id of p.state.selection) remoteSelections.set(idKey(id), [...(remoteSelections.get(idKey(id)) ?? []), p]);

  return (
    <div style={{ display: "grid", gridTemplateRows: "auto 1fr", height: "100%" }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", padding: 8, borderBottom: "1px solid #ddd", background: "#fff" }}>
        <strong>Converge</strong>
        <span style={{ color: "#888" }}>{session.docId.slice(0, 8)}</span>
        <button onClick={addRect} data-testid="add-rect">+ Rect</button>
        <button onClick={addText} data-testid="add-text">+ Text</button>
        <button onClick={del} disabled={!selected} data-testid="delete">Delete</button>
        <button onClick={front} disabled={!selected}>To front</button>
        <button onClick={recolor} disabled={!selected} data-testid="recolor">Recolor</button>
        <button onClick={editText} disabled={!selected || doc.get(selected)?.kind !== "text"}>Edit text</button>
        <span style={{ flex: 1 }} />
        <StatusPill status={status} offline={client.isOffline()} />
        <span style={{ color: colorHex(session.color, "#000") }}>● {session.userName}</span>
        {presence.map((p) => (
          <span key={p.replica.toString()} style={{ color: colorHex(p.user.color, "#000") }}>● {p.user.name}</span>
        ))}
      </div>
      <div style={{ position: "relative", overflow: "hidden" }}>
        <svg
          ref={svgRef}
          data-testid="canvas"
          style={{ width: "100%", height: "100%", touchAction: "none", background: "white" }}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerDown={() => setSelected(null)}
        >
          {objects.map(([id, o]) => (
            <Shape key={idKey(id)} id={id} o={o} selected={selected !== null && idEquals(selected, id)} remote={remoteSelections.get(idKey(id)) ?? []} onPointerDown={onPointerDown} />
          ))}
          {presence.map((p) =>
            p.state.cursor ? (
              <g key={`c${p.replica}`} transform={`translate(${p.state.cursor[0]},${p.state.cursor[1]})`} pointerEvents="none">
                <path d="M0 0 L0 14 L4 11 L7 17 L9 16 L6 10 L11 10 Z" fill={colorHex(p.user.color, "#000")} />
                <text x={12} y={20} fontSize={11} fill={colorHex(p.user.color, "#000")}>{p.user.name}</text>
              </g>
            ) : null,
          )}
        </svg>
        <DebugPanel client={client} status={status} />
      </div>
    </div>
  );
}

function Shape({ id, o, selected, remote, onPointerDown }: { id: ObjectId; o: ObjectState; selected: boolean; remote: PresenceEntry[]; onPointerDown: (e: React.PointerEvent, id: ObjectId, o: ObjectState) => void }) {
  const x = valueAsNumber(o.get("x")) ?? 0;
  const y = valueAsNumber(o.get("y")) ?? 0;
  const outline = selected ? "#1a73e8" : remote[0] ? colorHex(remote[0].user.color, "#888") : "none";
  if (o.kind === "text") {
    const text = valueAsString(o.get("text")) ?? "";
    const size = valueAsNumber(o.get("size")) ?? 16;
    return (
      <g onPointerDown={(e) => onPointerDown(e, id, o)} style={{ cursor: "move" }} data-testid="shape" data-kind="text">
        <rect x={x - 4} y={y - size} width={Math.max(30, text.length * size * 0.6) + 8} height={size * 1.4} fill="transparent" stroke={outline} strokeDasharray={selected ? undefined : "4 2"} />
        <text x={x} y={y} fontSize={size} fill={colorHex(valueAsNumber(o.get("color")), "#222")}>{text}</text>
      </g>
    );
  }
  const w = valueAsNumber(o.get("w")) ?? 100;
  const h = valueAsNumber(o.get("h")) ?? 60;
  return (
    <g onPointerDown={(e) => onPointerDown(e, id, o)} style={{ cursor: "move" }} data-testid="shape" data-kind="rect">
      <rect x={x} y={y} width={w} height={h} rx={4} fill={colorHex(valueAsNumber(o.get("fill")), "#ccc")} stroke={outline} strokeWidth={2} strokeDasharray={selected ? undefined : "4 2"} />
    </g>
  );
}

function StatusPill({ status, offline }: { status: ClientStatus; offline: boolean }) {
  const label = offline ? "offline" : status.state === "live" ? "live" : status.state === "hello_sent" ? "connecting" : "disconnected";
  const bg = offline ? "#999" : status.state === "live" ? "#2e7d32" : "#e65100";
  return (
    <span data-testid="status" data-state={label} style={{ background: bg, color: "#fff", borderRadius: 12, padding: "2px 10px" }}>
      {label} · seq {status.lastSeq.toString()} · pending {status.pending}{status.unacked ? ` (${status.unacked} unacked)` : ""}{status.unsaved ? ` · ${status.unsaved} unsaved` : ""}
    </span>
  );
}

export type { Op };

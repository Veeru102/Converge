import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { idKey, type ObjectId, type ObjectState } from "@converge/engine";
import type { PresenceEntry } from "@converge/protocol";
import type { Client } from "@converge/sync";
import { bounds, intersects, normBox, str, type Box, type Entry } from "../state/model.js";
import {
  createShape,
  deleteMany,
  moveTo,
  setGeometry,
  setLineEnd,
  setText,
  type Style,
  type Tool,
} from "../state/actions.js";
import { toWorld, zoomAt, type Viewport } from "../state/viewport.js";
import { usePresenceSender } from "../hooks/usePresenceSender.js";
import { ShapeView } from "./ShapeView.js";
import { SelectionOverlay, type Handle } from "./SelectionOverlay.js";
import { RemoteCursors, RemoteSelections } from "./RemotePresence.js";
import { TextEditor } from "./TextEditor.js";

type Mode =
  | { t: "idle" }
  | { t: "pan"; sx: number; sy: number; vx: number; vy: number }
  | { t: "marquee"; x0: number; y0: number; additive: boolean }
  | {
      t: "drag";
      items: Array<{ id: ObjectId; o: ObjectState; origin: Box }>;
      x0: number;
      y0: number;
      moved: boolean;
      last: number;
      sent: string;
    }
  | {
      t: "resize";
      id: ObjectId;
      o: ObjectState;
      handle: Handle;
      origin: Box;
      x0: number;
      y0: number;
      last: number;
      sent: string;
    }
  | { t: "lineend"; id: ObjectId; which: 1 | 2; last: number; sent: string }
  | {
      t: "draw";
      kind: Exclude<Tool, "select">;
      id: ObjectId | null;
      x0: number;
      y0: number;
      last: number;
      sent: string;
    };

const RATE_MS = 33;

interface CanvasProps {
  client: Client;
  objects: Entry[];
  presence: PresenceEntry[];
  tool: Tool;
  onToolUsed: () => void;
  selection: Set<string>;
  setSelection: (s: Set<string>) => void;
  viewport: Viewport;
  setViewport: (v: Viewport | ((p: Viewport) => Viewport)) => void;
  style: Style;
  textColor: number;
  editingId: ObjectId | null;
  setEditingId: (id: ObjectId | null) => void;
  spaceHeld: boolean;
}

export function CanvasView(p: CanvasProps) {
  const {
    client,
    objects,
    presence,
    tool,
    selection,
    setSelection,
    viewport,
    setViewport,
    editingId,
    setEditingId,
    spaceHeld,
  } = p;
  const svgRef = useRef<SVGSVGElement>(null);
  const mode = useRef<Mode>({ t: "idle" });
  const [marquee, setMarquee] = useState<Box | null>(null);
  const [panning, setPanning] = useState(false);
  const sendPresence = usePresenceSender(client);
  const cursorRef = useRef<[number, number] | null>(null);
  const doc = client.document;

  const byKey = useMemo(
    () => new Map(objects.map(([id, o]) => [idKey(id), [id, o] as Entry])),
    [objects],
  );
  const selectedEntries = useMemo(
    () => [...selection].map((k) => byKey.get(k)).filter((e): e is Entry => !!e),
    [selection, byKey],
  );
  const selectedIds = useMemo(() => selectedEntries.map(([id]) => id), [selectedEntries]);

  const world = useCallback(
    (e: { clientX: number; clientY: number }) => {
      const r = svgRef.current!.getBoundingClientRect();
      return toWorld(viewport, e.clientX - r.left, e.clientY - r.top);
    },
    [viewport],
  );

  const announce = useCallback(
    (sel: ObjectId[] = selectedIds) => sendPresence({ cursor: cursorRef.current, selection: sel }),
    [sendPresence, selectedIds],
  );
  // Re-announce only when the selection itself changes, not on every cursor move.
  useEffect(() => {
    announce();
  }, [selection]);

  // Wheel must be non-passive to stop the page from scrolling/zooming.
  useEffect(() => {
    const el = svgRef.current!;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const r = el.getBoundingClientRect();
      if (e.ctrlKey || e.metaKey) {
        const factor = Math.exp(-e.deltaY * 0.01);
        setViewport((v) => zoomAt(v, factor, e.clientX - r.left, e.clientY - r.top));
      } else {
        setViewport((v) => ({ ...v, x: v.x - e.deltaX, y: v.y - e.deltaY }));
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [setViewport]);

  const startPan = (e: React.PointerEvent) => {
    mode.current = { t: "pan", sx: e.clientX, sy: e.clientY, vx: viewport.x, vy: viewport.y };
    setPanning(true);
  };

  const onBackgroundDown = (e: React.PointerEvent) => {
    if (e.button === 1 || spaceHeld || (e.button === 0 && e.altKey && tool === "select")) {
      startPan(e);
      svgRef.current!.setPointerCapture(e.pointerId);
      return;
    }
    if (e.button !== 0) return;
    svgRef.current!.setPointerCapture(e.pointerId);
    const w = world(e);
    if (editingId) setEditingId(null);
    if (tool === "select") {
      const additive = e.shiftKey;
      if (!additive) setSelection(new Set());
      mode.current = { t: "marquee", x0: w.x, y0: w.y, additive };
      return;
    }
    mode.current = { t: "draw", kind: tool, id: null, x0: w.x, y0: w.y, last: 0, sent: "" };
  };

  const onShapeDown = (e: React.PointerEvent, id: ObjectId) => {
    if (tool !== "select" || e.button !== 0 || spaceHeld) return;
    e.stopPropagation();
    svgRef.current!.setPointerCapture(e.pointerId);
    const key = idKey(id);
    let next: Set<string>;
    if (e.shiftKey) {
      next = new Set(selection);
      if (next.has(key)) next.delete(key);
      else next.add(key);
    } else {
      next = selection.has(key) ? selection : new Set([key]);
    }
    if (next !== selection) setSelection(next);
    if (editingId && idKey(editingId) !== key) setEditingId(null);
    const w = world(e);
    const items = [...next]
      .map((k) => byKey.get(k))
      .filter((x): x is Entry => !!x)
      .map(([iid, o]) => ({ id: iid, o, origin: bounds(o) }));
    mode.current = { t: "drag", items, x0: w.x, y0: w.y, moved: false, last: 0, sent: "" };
  };

  const onHandleDown = (e: React.PointerEvent, handle: Handle) => {
    if (selectedEntries.length !== 1) return;
    e.stopPropagation();
    svgRef.current!.setPointerCapture(e.pointerId);
    const [id, o] = selectedEntries[0]!;
    const w = world(e);
    mode.current = {
      t: "resize",
      id,
      o,
      handle,
      origin: bounds(o),
      x0: w.x,
      y0: w.y,
      last: 0,
      sent: "",
    };
  };

  const onLineEndDown = (e: React.PointerEvent, which: 1 | 2) => {
    if (selectedEntries.length !== 1) return;
    e.stopPropagation();
    svgRef.current!.setPointerCapture(e.pointerId);
    mode.current = { t: "lineend", id: selectedEntries[0]![0], which, last: 0, sent: "" };
  };

  const onDoubleClick = (_e: React.MouseEvent, id: ObjectId) => {
    if (tool !== "select") return;
    const o = doc.get(id);
    if (o?.kind === "text") {
      setSelection(new Set([idKey(id)]));
      setEditingId(id);
    }
  };

  /** Rate-limit geometry ops during gestures; the final call always goes out. */
  const due = (m: { last: number; sent: string }, key: string, final: boolean): boolean => {
    const now = performance.now();
    if (m.sent === key) return false;
    if (!final && now - m.last < RATE_MS) return false;
    m.last = now;
    m.sent = key;
    return true;
  };

  const resized = (
    m: Extract<Mode, { t: "resize" }>,
    wx: number,
    wy: number,
    keepAspect: boolean,
  ): Box => {
    const { origin: b, handle } = m;
    const dx = wx - m.x0,
      dy = wy - m.y0;
    let x = b.x,
      y = b.y,
      w = b.w,
      h = b.h;
    if (handle.includes("e")) w = b.w + dx;
    if (handle.includes("s")) h = b.h + dy;
    if (handle.includes("w")) {
      x = b.x + dx;
      w = b.w - dx;
    }
    if (handle.includes("n")) {
      y = b.y + dy;
      h = b.h - dy;
    }
    if (keepAspect && handle.length === 2 && b.w > 0 && b.h > 0) {
      const k = Math.max(Math.abs(w) / b.w, Math.abs(h) / b.h);
      const nw = b.w * k * Math.sign(w || 1),
        nh = b.h * k * Math.sign(h || 1);
      if (handle.includes("w")) x = b.x + b.w - nw;
      if (handle.includes("n")) y = b.y + b.h - nh;
      w = nw;
      h = nh;
    }
    const out = normBox(x, y, x + w, y + h);
    out.w = Math.max(4, out.w);
    out.h = m.o.kind === "text" ? b.h : Math.max(4, out.h);
    return out;
  };

  const step = (e: React.PointerEvent, final: boolean) => {
    const m = mode.current;
    switch (m.t) {
      case "pan":
        setViewport({ ...viewport, x: m.vx + (e.clientX - m.sx), y: m.vy + (e.clientY - m.sy) });
        break;
      case "marquee": {
        const w = world(e);
        setMarquee(normBox(m.x0, m.y0, w.x, w.y));
        break;
      }
      case "drag": {
        const w = world(e);
        const dx = w.x - m.x0,
          dy = w.y - m.y0;
        if (!m.moved && Math.hypot(dx, dy) < 2 / viewport.scale) return;
        m.moved = true;
        if (due(m, `${dx},${dy}`, final)) moveTo(client, m.items, dx, dy);
        break;
      }
      case "resize": {
        const w = world(e);
        const box = resized(m, w.x, w.y, e.shiftKey);
        if (due(m, JSON.stringify(box), final)) setGeometry(client, m.id, m.o, box);
        break;
      }
      case "lineend": {
        const w = world(e);
        if (due(m, `${w.x},${w.y}`, final)) setLineEnd(client, m.id, m.which, w.x, w.y);
        break;
      }
      case "draw": {
        if (m.kind === "text") return; // text is placed on release
        const w = world(e);
        const box =
          m.kind === "line"
            ? { x: m.x0, y: m.y0, w: w.x - m.x0, h: w.y - m.y0 }
            : normBox(m.x0, m.y0, w.x, w.y);
        if (m.id === null) {
          if (Math.hypot(w.x - m.x0, w.y - m.y0) < 3 / viewport.scale) return;
          m.id = createShape(client, m.kind, box, p.style, p.textColor);
          m.sent = JSON.stringify(box);
          setSelection(new Set([idKey(m.id)]));
          return;
        }
        if (due(m, JSON.stringify(box), final)) {
          const o = doc.get(m.id);
          if (o) {
            if (m.kind === "line") setLineEnd(client, m.id, 2, w.x, w.y);
            else setGeometry(client, m.id, o, box);
          }
        }
        break;
      }
    }
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const w = world(e);
    cursorRef.current = [w.x, w.y];
    announce();
    if (mode.current.t !== "idle") step(e, false);
  };

  const onPointerUp = (e: React.PointerEvent) => {
    const m = mode.current;
    if (m.t === "idle") return;
    step(e, true);
    switch (m.t) {
      case "pan":
        setPanning(false);
        break;
      case "marquee": {
        const w = world(e);
        const box = normBox(m.x0, m.y0, w.x, w.y);
        if (box.w > 1 || box.h > 1) {
          const hit = objects
            .filter(([, o]) => intersects(box, bounds(o)))
            .map(([id]) => idKey(id));
          setSelection(m.additive ? new Set([...selection, ...hit]) : new Set(hit));
        }
        setMarquee(null);
        break;
      }
      case "draw": {
        if (m.kind === "text") {
          // Created on release so the browser's mousedown focus handling cannot blur the editor.
          const id = createShape(
            client,
            "text",
            { x: m.x0, y: m.y0 - 11, w: 0, h: 0 },
            p.style,
            p.textColor,
          );
          setSelection(new Set([idKey(id)]));
          setEditingId(id);
        } else if (m.id === null) {
          const size =
            m.kind === "line"
              ? { x: m.x0, y: m.y0, w: 140, h: 0 }
              : { x: m.x0 - 60, y: m.y0 - 40, w: 120, h: 80 };
          const id = createShape(client, m.kind, size, p.style, p.textColor);
          setSelection(new Set([idKey(id)]));
        }
        p.onToolUsed();
        break;
      }
    }
    mode.current = { t: "idle" };
  };

  const onPointerLeave = () => {
    cursorRef.current = null;
    announce();
  };

  // Inline text editing: live updates at ~8 Hz, final on close; empty text objects are removed.
  const textThrottle = useRef<{
    timer: ReturnType<typeof setTimeout> | null;
    pending: string | null;
    last: number;
  }>({ timer: null, pending: null, last: 0 });
  const onTextInput = (text: string) => {
    if (!editingId) return;
    const t = textThrottle.current;
    const now = performance.now();
    const id = editingId;
    if (now - t.last >= 125) {
      t.last = now;
      setText(client, id, text);
      return;
    }
    t.pending = text;
    t.timer ??= setTimeout(
      () => {
        t.timer = null;
        if (t.pending !== null) {
          t.last = performance.now();
          setText(client, id, t.pending);
          t.pending = null;
        }
      },
      125 - (now - t.last),
    );
  };
  const onTextDone = () => {
    if (!editingId) return;
    const t = textThrottle.current;
    if (t.timer) {
      clearTimeout(t.timer);
      t.timer = null;
    }
    const id = editingId;
    const text = t.pending ?? str(doc.get(id)!, "text");
    t.pending = null;
    if (text.trim() === "") deleteMany(client, [id]);
    else if (text !== str(doc.get(id)!, "text")) setText(client, id, text);
    setEditingId(null);
  };

  const editing = editingId ? doc.get(editingId) : undefined;
  const scale = viewport.scale;
  const gridSize = 24 * scale;
  const cls = [
    "canvas",
    panning ? "panning" : spaceHeld ? "space" : tool === "select" ? "tool-select" : "tool-draw",
  ].join(" ");

  return (
    <svg
      ref={svgRef}
      className={cls}
      data-testid="canvas"
      onPointerDown={onBackgroundDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onPointerLeave={onPointerLeave}
    >
      <defs>
        <pattern
          id="grid"
          width={gridSize}
          height={gridSize}
          patternUnits="userSpaceOnUse"
          x={viewport.x % gridSize}
          y={viewport.y % gridSize}
        >
          <circle cx={1} cy={1} r={Math.min(1.2, 0.8 * scale + 0.4)} fill="var(--grid)" />
        </pattern>
      </defs>
      <rect width="100%" height="100%" fill="url(#grid)" />
      <g transform={`translate(${viewport.x} ${viewport.y}) scale(${scale})`}>
        {objects.map(([id, o]) => (
          <ShapeView
            key={idKey(id)}
            id={id}
            o={o}
            interactive={tool === "select" && !spaceHeld}
            editing={editingId !== null && idKey(id) === idKey(editingId)}
            onPointerDown={onShapeDown}
            onDoubleClick={onDoubleClick}
          />
        ))}
        <RemoteSelections presence={presence} doc={doc} scale={scale} />
        {!editing && (
          <SelectionOverlay
            selected={selectedEntries.map(([, o]) => o)}
            scale={scale}
            interactive={tool === "select" && !spaceHeld}
            onHandleDown={onHandleDown}
            onLineEndDown={onLineEndDown}
          />
        )}
        {marquee && (
          <rect
            x={marquee.x}
            y={marquee.y}
            width={marquee.w}
            height={marquee.h}
            fill="var(--accent-soft)"
            stroke="var(--accent)"
            strokeWidth={1 / scale}
            data-testid="marquee"
          />
        )}
        {editing && editingId && (
          <TextEditor
            key={idKey(editingId)}
            o={editing}
            onInput={onTextInput}
            onDone={onTextDone}
          />
        )}
        <RemoteCursors presence={presence} scale={scale} />
      </g>
    </svg>
  );
}

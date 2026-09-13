import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { idKey, type ObjectId } from "@converge/engine";
import { useClient } from "../hooks/useClient.js";
import { useSyncStatus } from "../hooks/useSyncStatus.js";
import { useKeyboard } from "../hooks/useKeyboard.js";
import { useLocalStorage } from "../hooks/useLocalStorage.js";
import { bounds, union, type Entry } from "../state/model.js";
import { deleteMany, duplicate, nudge, reorder, type Style, type Tool } from "../state/actions.js";
import { fitTo, zoomAt, type Viewport } from "../state/viewport.js";
import type { Session } from "../session.js";
import { CanvasView } from "../canvas/CanvasView.js";
import { Toolbar } from "./Toolbar.js";
import { StatusPill } from "./StatusPill.js";
import { PresenceAvatars } from "./Presence.js";
import { PropertiesPanel } from "./PropertiesPanel.js";
import { NetworkLab } from "./NetworkLab.js";
import { ShortcutSheet } from "./ShortcutSheet.js";
import { Icons } from "./icons.js";

export function Shell({ session, bench }: { session: Session; bench?: (client: Session["client"]) => React.ReactNode }) {
  const { client, docId } = session;
  const { version, status, presence, objects } = useClient(client);
  const [tool, setTool] = useState<Tool>("select");
  const [selection, setSelection] = useState<Set<string>>(() => new Set());
  const [editingId, setEditingId] = useState<ObjectId | null>(null);
  const [viewport, setViewport] = useState<Viewport>({ x: 0, y: 0, scale: 1 });
  const [labOpen, setLabOpen] = useLocalStorage("converge:lab", false);
  const [propsOpen, setPropsOpen] = useLocalStorage("converge:props", true);
  const [sheet, setSheet] = useState(false);
  const [spaceHeld, setSpaceHeld] = useState(false);
  const stageRef = useRef<HTMLDivElement>(null);
  const view = useSyncStatus(client, docId, status, version, true);

  const style: Style = useMemo(() => ({ fill: 0xc7d2fe, stroke: 0x4f6ef7, strokeWidth: 0, opacity: 1 }), []);
  const others = useMemo(() => presence.filter((p) => p.replica !== status.replica), [presence, status.replica]);

  // Selection ∩ visible objects (remote deletes drop out automatically).
  const byKey = useMemo(() => new Map(objects.map(([id, o]) => [idKey(id), [id, o] as Entry])), [objects]);
  const selected = useMemo(() => [...selection].map((k) => byKey.get(k)).filter((e): e is Entry => !!e), [selection, byKey]);
  const selectedIds = useMemo(() => selected.map(([id]) => id), [selected]);

  const stageSize = () => {
    const r = stageRef.current?.getBoundingClientRect();
    return { w: r?.width ?? 1200, h: r?.height ?? 800 };
  };
  const zoomBy = useCallback((f: number) => setViewport((v) => { const { w, h } = stageSize(); return zoomAt(v, f, w / 2, h / 2); }), []);
  const fit = useCallback(() => {
    const { w, h } = stageSize();
    setViewport(fitTo(union(objects.map(([, o]) => bounds(o))), w, h));
  }, [objects]);

  const doDuplicate = useCallback(() => {
    if (!selected.length) return;
    setSelection(new Set(duplicate(client, selected).map(idKey)));
  }, [client, selected]);
  const doDelete = useCallback(() => {
    if (!selectedIds.length) return;
    deleteMany(client, selectedIds);
    setSelection(new Set());
  }, [client, selectedIds]);

  useEffect(() => {
    const down = (e: KeyboardEvent) => { if (e.code === "Space" && !(e.target as HTMLElement).matches("input,textarea")) { setSpaceHeld(true); e.preventDefault(); } };
    const up = (e: KeyboardEvent) => { if (e.code === "Space") setSpaceHeld(false); };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => { window.removeEventListener("keydown", down); window.removeEventListener("keyup", up); };
  }, []);

  const keys = useMemo(
    () => ({
      v: () => setTool("select"), r: () => setTool("rect"), o: () => setTool("ellipse"), l: () => setTool("line"), t: () => setTool("text"),
      escape: () => { setSelection(new Set()); setTool("select"); setSheet(false); },
      delete: doDelete, backspace: doDelete,
      "mod+d": doDuplicate,
      "mod+a": () => setSelection(new Set(objects.map(([id]) => idKey(id)))),
      "mod+]": () => reorder(client, selectedIds, "forward"), "mod+[": () => reorder(client, selectedIds, "backward"),
      "mod+shift+]": () => reorder(client, selectedIds, "front"), "mod+shift+[": () => reorder(client, selectedIds, "back"),
      "mod+shift+}": () => reorder(client, selectedIds, "front"), "mod+shift+{": () => reorder(client, selectedIds, "back"),
      arrowleft: () => nudge(client, selected, -1, 0), arrowright: () => nudge(client, selected, 1, 0), arrowup: () => nudge(client, selected, 0, -1), arrowdown: () => nudge(client, selected, 0, 1),
      "shift+arrowleft": () => nudge(client, selected, -10, 0), "shift+arrowright": () => nudge(client, selected, 10, 0), "shift+arrowup": () => nudge(client, selected, 0, -10), "shift+arrowdown": () => nudge(client, selected, 0, 10),
      "mod+0": () => setViewport({ x: 0, y: 0, scale: 1 }), "mod+1": fit, "mod+=": () => zoomBy(1.2), "mod+-": () => zoomBy(1 / 1.2),
      "mod+.": () => setLabOpen((v) => !v),
      "?": () => setSheet((v) => !v), "shift+?": () => setSheet((v) => !v),
      enter: () => { if (selected.length === 1 && selected[0]![1].kind === "text") setEditingId(selected[0]![0]); },
    }),
    [client, objects, selected, selectedIds, doDelete, doDuplicate, fit, zoomBy, setLabOpen],
  );
  useKeyboard(keys, editingId === null);

  const showProps = propsOpen && selected.length > 0 && !labOpen;

  return (
    <div className="shell">
      <div className="topbar">
        <div className="brand"><span className="logo"><Icons.logo /></span>Converge</div>
        <span className="doc" title={docId}>{docId.slice(0, 8)}</span>
        <span className="spacer" />
        <PresenceAvatars me={{ name: session.userName, color: session.color }} others={others} />
        <StatusPill view={view} onClick={() => setLabOpen((v) => !v)} />
        <button className={`icon-btn${labOpen ? " active" : ""}`} title="Network Lab (⌘.)" aria-label="Network Lab" data-testid="lab-toggle" onClick={() => setLabOpen((v) => !v)}><Icons.lab /></button>
        <button className="icon-btn" title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts" onClick={() => setSheet(true)}><Icons.keyboard /></button>
      </div>
      <div className="stage" ref={stageRef}>
        <CanvasView
          client={client} objects={objects} presence={others} tool={tool} onToolUsed={() => setTool("select")}
          selection={selection} setSelection={setSelection} viewport={viewport} setViewport={setViewport}
          style={style} textColor={0x17171c} editingId={editingId} setEditingId={setEditingId} spaceHeld={spaceHeld}
        />
        {objects.length === 0 && tool === "select" && (
          <div className="hint" data-testid="empty-hint">
            <h2>Blank canvas</h2>
            <div>Press <kbd>R</kbd> for a rectangle, <kbd>T</kbd> for text, or share this URL to collaborate.</div>
          </div>
        )}
        <Toolbar tool={tool} setTool={setTool} hasSelection={selected.length > 0} onDuplicate={doDuplicate} onDelete={doDelete} />
        <div className="zoomctl">
          <button className="icon-btn" aria-label="Zoom out" onClick={() => zoomBy(1 / 1.2)}><Icons.minus /></button>
          <button className="pct btn ghost sm" title="Reset zoom (⌘0)" onClick={() => setViewport({ x: 0, y: 0, scale: 1 })}>{Math.round(viewport.scale * 100)}%</button>
          <button className="icon-btn" aria-label="Zoom in" onClick={() => zoomBy(1.2)}><Icons.plus /></button>
          <button className="icon-btn" aria-label="Zoom to fit" title="Zoom to fit (⌘1)" onClick={fit}><Icons.fit /></button>
        </div>
        {showProps && <PropertiesPanel client={client} selected={selected} onClose={() => setPropsOpen(false)} />}
        {!propsOpen && selected.length > 0 && !labOpen && (
          <button className="btn" style={{ position: "absolute", right: 12, top: 12 }} onClick={() => setPropsOpen(true)}>Properties</button>
        )}
        {labOpen && <NetworkLab client={client} view={view} onClose={() => setLabOpen(false)} bench={bench?.(client)} />}
        {sheet && <ShortcutSheet onClose={() => setSheet(false)} />}
      </div>
    </div>
  );
}

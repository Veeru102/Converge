const ROWS: Array<[string, string]> = [
  ["Select / Rectangle / Ellipse / Arrow / Text", "V R O L T"],
  ["Add to selection", "⇧ click"],
  ["Select all / clear", "⌘A / Esc"],
  ["Duplicate", "⌘D"],
  ["Delete", "⌫"],
  ["Nudge (×10)", "← → ↑ ↓ (⇧)"],
  ["Bring forward / send backward", "⌘] / ⌘["],
  ["Bring to front / send to back", "⌘⇧] / ⌘⇧["],
  ["Edit text", "double-click / Enter"],
  ["Zoom", "⌘ scroll · pinch"],
  ["Pan", "scroll · space drag · middle drag"],
  ["Reset zoom / fit", "⌘0 / ⌘1"],
  ["Network Lab", "⌘."],
  ["This sheet", "?"],
];

export function ShortcutSheet({ onClose }: { onClose: () => void }) {
  return (
    <div className="sheet" onClick={onClose} role="dialog" aria-label="Keyboard shortcuts">
      <div className="card" onClick={(e) => e.stopPropagation()}>
        <h3>Keyboard shortcuts</h3>
        <div className="grid">
          {ROWS.map(([what, keys]) => (
            <div key={what} style={{ display: "contents" }}>
              <span className="muted">{what}</span>
              <span>
                {keys
                  .split(" ")
                  .map((k, i) =>
                    k === "/" || k === "·" ? <span key={i}> {k} </span> : <kbd key={i}>{k}</kbd>,
                  )}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

import { useEffect, useState } from "react";
import type { Client } from "@converge/sync";
import type { ObjectId } from "@converge/engine";
import {
  boolV,
  bool,
  color,
  colorV,
  f64,
  hexOf,
  num,
  parseHex,
  type Entry,
} from "../state/model.js";
import { reorder, setProps, type ZMove } from "../state/actions.js";
import { Icons } from "./icons.js";

const SWATCHES = [
  0xc7d2fe, 0x93c5fd, 0x86efac, 0xfde68a, 0xfdba74, 0xfca5a5, 0xf9a8d4, 0xd8b4fe, 0xe5e7eb,
  0x17171c, 0xffffff,
];

function ColorField({
  label,
  value,
  onChange,
  testid,
}: {
  label: string;
  value: number | undefined;
  onChange: (c: number) => void;
  testid: string;
}) {
  const [text, setText] = useState(hexOf(value, ""));
  useEffect(() => setText(hexOf(value, "")), [value]);
  return (
    <section>
      <h4>{label}</h4>
      <div className="swatches">
        {SWATCHES.map((c) => (
          <button
            key={c}
            className={`swatch${value === c ? " active" : ""}`}
            style={{ background: hexOf(c, "#fff") }}
            aria-label={hexOf(c, "")}
            onClick={() => onChange(c)}
          />
        ))}
      </div>
      <input
        className="input mono"
        data-testid={testid}
        value={text}
        placeholder="#rrggbb"
        onChange={(e) => {
          setText(e.target.value);
          const c = parseHex(e.target.value);
          if (c !== null) onChange(c);
        }}
      />
    </section>
  );
}

export function PropertiesPanel({
  client,
  selected,
  onClose,
}: {
  client: Client;
  selected: Entry[];
  onClose: () => void;
}) {
  const ids: ObjectId[] = selected.map(([id]) => id);
  const first = selected[0]?.[1];
  if (!first) return null;
  const apply = (key: string, v: ReturnType<typeof f64>) => setProps(client, ids, [[key, v]]);
  const z = (m: ZMove) => reorder(client, ids, m);
  const filled = selected.filter(([, o]) => o.kind === "rect" || o.kind === "ellipse");
  const texts = selected.filter(([, o]) => o.kind === "text");
  const lines = selected.filter(([, o]) => o.kind === "line");

  return (
    <div className="panel" data-testid="properties">
      <header>
        <span>
          {selected.length === 1
            ? ((
                { rect: "Rectangle", ellipse: "Ellipse", line: "Arrow", text: "Text" } as Record<
                  string,
                  string
                >
              )[first.kind ?? ""] ?? "Object")
            : `${selected.length} objects`}
        </span>
        <span className="spacer" />
        <button className="icon-btn" aria-label="Close" onClick={onClose}>
          <Icons.close />
        </button>
      </header>
      <div className="body">
        <section>
          <h4>Arrange</h4>
          <div className="row">
            <button
              className="btn sm"
              title="Bring to front (⌘⇧])"
              data-testid="z-front"
              onClick={() => z("front")}
            >
              <Icons.front /> Front
            </button>
            <button className="btn sm" title="Bring forward (⌘])" onClick={() => z("forward")}>
              <Icons.up />
            </button>
            <button className="btn sm" title="Send backward (⌘[)" onClick={() => z("backward")}>
              <Icons.down />
            </button>
            <button
              className="btn sm"
              title="Send to back (⌘⇧[)"
              data-testid="z-back"
              onClick={() => z("back")}
            >
              <Icons.back /> Back
            </button>
          </div>
        </section>
        {filled.length > 0 && (
          <ColorField
            label="Fill"
            value={color(filled[0]![1], "fill")}
            onChange={(c) =>
              setProps(
                client,
                filled.map(([id]) => id),
                [["fill", colorV(c)]],
              )
            }
            testid="fill-hex"
          />
        )}
        {(filled.length > 0 || lines.length > 0) && (
          <>
            <ColorField
              label="Stroke"
              value={color((filled[0] ?? lines[0])![1], "stroke")}
              onChange={(c) =>
                setProps(
                  client,
                  [...filled, ...lines].map(([id]) => id),
                  [["stroke", colorV(c)]],
                )
              }
              testid="stroke-hex"
            />
            <section>
              <div className="row">
                <label>Stroke width</label>
                <input
                  className="input num"
                  type="number"
                  min={0}
                  max={40}
                  step={1}
                  value={num((filled[0] ?? lines[0])![1], "stroke_width", 0)}
                  onChange={(e) =>
                    setProps(
                      client,
                      [...filled, ...lines].map(([id]) => id),
                      [["stroke_width", f64(Math.max(0, +e.target.value))]],
                    )
                  }
                />
              </div>
            </section>
          </>
        )}
        {lines.length > 0 && (
          <section>
            <label className="switch">
              <input
                type="checkbox"
                checked={bool(lines[0]![1], "arrow")}
                onChange={(e) =>
                  setProps(
                    client,
                    lines.map(([id]) => id),
                    [["arrow", boolV(e.target.checked)]],
                  )
                }
              />
              Arrow head
            </label>
          </section>
        )}
        {texts.length > 0 && (
          <>
            <ColorField
              label="Text colour"
              value={color(texts[0]![1], "color")}
              onChange={(c) =>
                setProps(
                  client,
                  texts.map(([id]) => id),
                  [["color", colorV(c)]],
                )
              }
              testid="text-hex"
            />
            <section>
              <div className="row">
                <label>Font size</label>
                <input
                  className="input num"
                  type="number"
                  min={8}
                  max={200}
                  value={num(texts[0]![1], "size", 18)}
                  onChange={(e) =>
                    setProps(
                      client,
                      texts.map(([id]) => id),
                      [["size", f64(Math.max(8, +e.target.value))]],
                    )
                  }
                />
              </div>
            </section>
          </>
        )}
        <section>
          <div className="row">
            <label>Opacity</label>
            <span className="mono muted">{Math.round(num(first, "opacity", 1) * 100)}%</span>
          </div>
          <input
            type="range"
            min={0}
            max={100}
            value={Math.round(num(first, "opacity", 1) * 100)}
            onChange={(e) => apply("opacity", f64(+e.target.value / 100))}
            data-testid="opacity"
          />
        </section>
        {selected.length === 1 && first.kind !== "line" && (
          <section>
            <h4>Position</h4>
            <div className="kv">
              <dt>x</dt>
              <dd>{num(first, "x").toFixed(0)}</dd>
              <dt>y</dt>
              <dd>{num(first, "y").toFixed(0)}</dd>
              {first.kind !== "text" && (
                <>
                  <dt>w</dt>
                  <dd>{num(first, "w").toFixed(0)}</dd>
                  <dt>h</dt>
                  <dd>{num(first, "h").toFixed(0)}</dd>
                </>
              )}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}

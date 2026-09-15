import { useEffect, useRef } from "react";
import type { ObjectState } from "@converge/engine";
import { hexOf, color, num, str, textBox, TEXT_LINE } from "../state/model.js";

interface Props {
  o: ObjectState;
  onInput: (text: string) => void;
  onDone: () => void;
}

/** Inline editor over a text object; commits live (throttled by the caller) and closes on blur/Esc. */
export function TextEditor({ o, onInput, onDone }: Props) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const box = textBox(o);
  const size = num(o, "size", 18);
  const initial = useRef(str(o, "text"));
  useEffect(() => {
    const focus = () => {
      const el = ref.current;
      if (!el || document.activeElement === el) return;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    };
    focus();
    const raf = requestAnimationFrame(focus);
    return () => cancelAnimationFrame(raf);
  }, []);
  const lines = Math.max(1, (ref.current?.value ?? initial.current).split("\n").length);
  const w = Math.max(box.w, 60);
  const h = Math.max(lines * size * TEXT_LINE, size * TEXT_LINE);
  return (
    <foreignObject x={box.x} y={box.y} width={w + 4} height={h + 4} style={{ overflow: "visible" }}>
      <textarea
        ref={ref}
        className="text-editor"
        data-testid="text-editor"
        defaultValue={initial.current}
        rows={lines}
        spellCheck={false}
        style={{
          fontSize: size,
          color: hexOf(color(o, "color"), "#17171c"),
          width: w + 4,
          height: h + 4,
        }}
        onInput={(e) => onInput((e.target as HTMLTextAreaElement).value)}
        onBlur={onDone}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape" || (e.key === "Enter" && !e.shiftKey)) {
            e.preventDefault();
            onDone();
          }
        }}
        onPointerDown={(e) => e.stopPropagation()}
      />
    </foreignObject>
  );
}

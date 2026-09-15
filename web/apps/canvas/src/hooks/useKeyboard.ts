import { useEffect } from "react";

export interface KeyHandlers {
  [combo: string]: (e: KeyboardEvent) => void;
}

const isEditable = (t: EventTarget | null) => {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
};

/** Combos look like "mod+d", "shift+arrowup", "delete", "?" — `mod` is ⌘ on macOS, Ctrl elsewhere. */
export function useKeyboard(handlers: KeyHandlers, enabled = true): void {
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (isEditable(e.target)) return;
      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push("mod");
      if (e.shiftKey) parts.push("shift");
      if (e.altKey) parts.push("alt");
      const key = e.key.toLowerCase();
      const combo = [...parts, key].join("+");
      const h = handlers[combo] ?? (key === "?" ? handlers["?"] : undefined);
      if (h) {
        e.preventDefault();
        h(e);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handlers, enabled]);
}

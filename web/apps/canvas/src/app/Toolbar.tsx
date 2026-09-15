import { Icons } from "./icons.js";
import type { Tool } from "../state/actions.js";

interface Props {
  tool: Tool;
  setTool: (t: Tool) => void;
  hasSelection: boolean;
  onDuplicate: () => void;
  onDelete: () => void;
}

const TOOLS: Array<{ id: Tool; icon: keyof typeof Icons; label: string; key: string }> = [
  { id: "select", icon: "cursor", label: "Select", key: "V" },
  { id: "rect", icon: "rect", label: "Rectangle", key: "R" },
  { id: "ellipse", icon: "ellipse", label: "Ellipse", key: "O" },
  { id: "line", icon: "line", label: "Arrow", key: "L" },
  { id: "text", icon: "text", label: "Text", key: "T" },
];

export function Toolbar({ tool, setTool, hasSelection, onDuplicate, onDelete }: Props) {
  return (
    <div className="toolbar" role="toolbar" aria-label="Tools">
      {TOOLS.map((t) => {
        const Icon = Icons[t.icon];
        return (
          <button
            key={t.id}
            className={`icon-btn${tool === t.id ? " active" : ""}`}
            title={`${t.label} (${t.key})`}
            aria-label={t.label}
            aria-pressed={tool === t.id}
            data-testid={`tool-${t.id}`}
            onClick={() => setTool(t.id)}
          >
            <Icon />
            <span className="kbd">{t.key}</span>
          </button>
        );
      })}
      <div className="sep" />
      <button
        className="icon-btn"
        title="Duplicate (⌘D)"
        aria-label="Duplicate"
        disabled={!hasSelection}
        data-testid="duplicate"
        onClick={onDuplicate}
      >
        <Icons.copy />
      </button>
      <button
        className="icon-btn"
        title="Delete (⌫)"
        aria-label="Delete"
        disabled={!hasSelection}
        data-testid="delete"
        onClick={onDelete}
      >
        <Icons.trash />
      </button>
    </div>
  );
}

import type { PresenceEntry } from "@converge/protocol";
import { hexOf } from "../state/model.js";

export function PresenceAvatars({ me, others }: { me: { name: string; color: number }; others: PresenceEntry[] }) {
  const initial = (n: string) => (n.trim()[0] ?? "?").toUpperCase();
  return (
    <div className="row" data-testid="presence">
      <div className="avatars">
        <div className="avatar me" style={{ background: hexOf(me.color, "#888") }}>
          {initial(me.name)}
          <span className="tip">{me.name} (you)</span>
        </div>
        {others.map((p) => (
          <div key={p.replica.toString()} className="avatar" style={{ background: hexOf(p.user.color, "#888") }} data-testid="avatar">
            {initial(p.user.name)}
            <span className="tip">{p.user.name}</span>
          </div>
        ))}
      </div>
      <span className="online-count" data-testid="online-count">{others.length + 1} online</span>
    </div>
  );
}

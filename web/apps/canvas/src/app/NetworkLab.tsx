import { useEffect, useRef, useState } from "react";
import type { Client } from "@converge/sync";
import type { SyncView } from "../hooks/useSyncStatus.js";
import { Icons } from "./icons.js";
import { BenchPanel } from "./BenchPanel.js";

export interface ChaosConfig {
  enabled: boolean;
  latency_ms: [number, number];
  drop_p: number;
  dup_p: number;
  disconnect_every: number;
}

const CLEAN: ChaosConfig = {
  enabled: false,
  latency_ms: [0, 0],
  drop_p: 0,
  dup_p: 0,
  disconnect_every: 0,
};
export const PRESETS: Array<{ name: string; cfg: ChaosConfig }> = [
  { name: "Clean", cfg: CLEAN },
  {
    name: "Flaky Wi-Fi",
    cfg: { enabled: true, latency_ms: [20, 150], drop_p: 0.05, dup_p: 0.02, disconnect_every: 0 },
  },
  {
    name: "Bad mobile",
    cfg: { enabled: true, latency_ms: [200, 900], drop_p: 0.15, dup_p: 0.05, disconnect_every: 60 },
  },
  {
    name: "Hostile",
    cfg: { enabled: true, latency_ms: [50, 400], drop_p: 0.3, dup_p: 0.3, disconnect_every: 15 },
  },
];

async function putChaos(cfg: ChaosConfig): Promise<void> {
  await fetch("/admin/chaos", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(cfg),
  });
}

function Sparkline({ values, max }: { values: number[]; max: number }) {
  const w = 280,
    h = 36;
  const m = Math.max(1, max);
  const pts = values
    .map(
      (v, i) =>
        `${(i / Math.max(1, values.length - 1)) * w},${h - (Math.min(v, m) / m) * (h - 2) - 1}`,
    )
    .join(" ");
  return (
    <svg className="sparkline" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden>
      <polyline
        points={pts}
        fill="none"
        stroke="var(--accent)"
        strokeWidth={1.5}
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

interface Props {
  client: Client;
  view: SyncView;
  onClose: () => void;
}

/** Everything needed to break the network on purpose and watch the document heal. */
export function NetworkLab({ client, view, onClose }: Props) {
  const [chaos, setChaos] = useState<ChaosConfig>(CLEAN);
  const [history, setHistory] = useState<number[]>(() => Array(60).fill(0));
  const latest = useRef(view.status);
  latest.current = view.status;

  useEffect(() => {
    fetch("/admin/chaos")
      .then((r) => r.json())
      .then((c: Partial<ChaosConfig>) => setChaos({ ...CLEAN, ...c }))
      .catch(() => undefined);
    const i = setInterval(
      () => setHistory((h) => [...h.slice(1), latest.current.pending + latest.current.unsaved]),
      1000,
    );
    return () => clearInterval(i);
  }, []);

  const apply = (next: ChaosConfig) => {
    setChaos(next);
    void putChaos(next);
  };
  const isPreset = (p: ChaosConfig) => JSON.stringify(p) === JSON.stringify(chaos);
  const s = view.status;
  const serverMatch = view.server
    ? view.server.hash === view.localHash
      ? "match"
      : "differs"
    : "unknown";

  return (
    <div className="panel" data-testid="lab">
      <header>
        <Icons.lab />
        <span>Network Lab</span>
        <span className="spacer" />
        <button className="icon-btn" aria-label="Close" onClick={onClose}>
          <Icons.close />
        </button>
      </header>
      <div className="body">
        <section>
          <h4>Convergence</h4>
          <div className="row between">
            <span
              className={`badge ${view.converged ? "ok" : view.synced ? "neutral" : "warn"}`}
              data-testid="converged-badge"
            >
              {view.converged ? (
                <>
                  <Icons.check /> Converged
                </>
              ) : view.synced ? (
                "Synced · awaiting server check"
              ) : view.kind === "live" ? (
                "Syncing"
              ) : (
                view.kind
              )}
            </span>
            <span className="faint mono">{s.replica.toString(16).slice(0, 8)}</span>
          </div>
          <dl className="kv">
            <dt>Local seq</dt>
            <dd data-testid="local-seq">{s.lastSeq.toString()}</dd>
            <dt>Server durable seq</dt>
            <dd data-testid="server-seq">
              {view.server ? view.server.durableSeq.toString() : "—"}
            </dd>
            <dt>Server sessions</dt>
            <dd>{view.server?.sessions ?? "—"}</dd>
            <dt>Hash</dt>
            <dd data-testid="hash-match">{serverMatch}</dd>
          </dl>
          <div className="hash" title="local state hash" data-testid="local-hash">
            {view.localHash}
          </div>
          {view.server && view.server.hash !== view.localHash && (
            <div
              className="hash"
              style={{ color: "var(--warn)" }}
              title="server hash"
              data-testid="server-hash"
            >
              {view.server.hash}
            </div>
          )}
        </section>

        <section>
          <h4>Connection</h4>
          <div className="row between">
            <label className="switch">
              <input
                type="checkbox"
                data-testid="offline-toggle"
                checked={s.offline}
                onChange={(e) => client.setOffline(e.target.checked)}
              />
              Simulate offline
            </label>
            <button className="btn sm" onClick={() => client.reconnectNow()} disabled={s.offline}>
              <Icons.refresh /> Reconnect
            </button>
          </div>
          <div className="stats">
            <div className="stat">
              <span className="v" data-testid="stat-pending">
                {s.pending}
              </span>
              <span className="l">pending ops</span>
            </div>
            <div className="stat">
              <span className="v" data-testid="stat-unacked">
                {s.unacked}
              </span>
              <span className="l">unacked</span>
            </div>
            <div className="stat">
              <span className="v">{s.unsaved}</span>
              <span className="l">unsaved</span>
            </div>
            <div className="stat">
              <span className="v">{view.kind}</span>
              <span className="l">
                state
                {view.retryInMs !== null ? ` · retry ${Math.ceil(view.retryInMs / 1000)}s` : ""}
              </span>
            </div>
          </div>
          <Sparkline values={history} max={Math.max(10, ...history)} />
          <span className="faint">queued ops, last 60 s</span>
        </section>

        <section>
          <h4>Server chaos</h4>
          <div className="presets">
            {PRESETS.map((p) => (
              <button
                key={p.name}
                className={`btn sm${isPreset(p.cfg) ? " primary" : ""}`}
                data-testid={`preset-${p.name.toLowerCase().replace(/\s+/g, "-")}`}
                onClick={() => apply(p.cfg)}
              >
                {p.name}
              </button>
            ))}
          </div>
          <label className="switch">
            <input
              type="checkbox"
              data-testid="chaos-enabled"
              checked={chaos.enabled}
              onChange={(e) => apply({ ...chaos, enabled: e.target.checked })}
            />
            Fault injection enabled
          </label>
          <div className="row">
            <label>Latency</label>
            <span className="mono muted">
              {chaos.latency_ms[0]}–{chaos.latency_ms[1]} ms
            </span>
          </div>
          <input
            type="range"
            min={0}
            max={1000}
            step={10}
            value={chaos.latency_ms[1]}
            onChange={(e) =>
              apply({
                ...chaos,
                latency_ms: [Math.min(chaos.latency_ms[0], +e.target.value), +e.target.value],
              })
            }
          />
          <div className="row">
            <label>Drop</label>
            <span className="mono muted">{Math.round(chaos.drop_p * 100)}%</span>
          </div>
          <input
            type="range"
            min={0}
            max={80}
            value={Math.round(chaos.drop_p * 100)}
            onChange={(e) => apply({ ...chaos, drop_p: +e.target.value / 100 })}
          />
          <div className="row">
            <label>Duplicate</label>
            <span className="mono muted">{Math.round(chaos.dup_p * 100)}%</span>
          </div>
          <input
            type="range"
            min={0}
            max={80}
            value={Math.round(chaos.dup_p * 100)}
            onChange={(e) => apply({ ...chaos, dup_p: +e.target.value / 100 })}
          />
          <div className="row">
            <label>Disconnect every N messages</label>
            <input
              className="input num"
              type="number"
              min={0}
              value={chaos.disconnect_every}
              onChange={(e) => apply({ ...chaos, disconnect_every: Math.max(0, +e.target.value) })}
            />
          </div>
          <span className="faint">
            Applied server-side to every session, seeded and deterministic per session.
          </span>
        </section>

        <BenchPanel client={client} />
      </div>
    </div>
  );
}

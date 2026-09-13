import { useEffect, useState } from "react";
import type { Client, ClientStatus } from "@converge/sync";

interface ChaosConfig {
  enabled: boolean;
  latency_ms: [number, number];
  drop_p: number;
  dup_p: number;
  disconnect_every: number;
}

const DEFAULT: ChaosConfig = { enabled: false, latency_ms: [0, 0], drop_p: 0, dup_p: 0, disconnect_every: 0 };

/** Sync internals and server-side chaos controls. */
export function DebugPanel({ client, status }: { client: Client; status: ClientStatus }) {
  const [open, setOpen] = useState(false);
  const [chaos, setChaos] = useState<ChaosConfig>(DEFAULT);
  const [serverHash, setServerHash] = useState<string>("");
  const [offline, setOffline] = useState(client.isOffline());
  useEffect(() => {
    if (!open) return;
    fetch("/admin/chaos").then((r) => r.json()).then((c: ChaosConfig) => setChaos({ ...DEFAULT, ...c })).catch(() => undefined);
  }, [open]);
  const apply = async (next: ChaosConfig) => {
    setChaos(next);
    await fetch("/admin/chaos", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(next) });
  };
  const checkServer = async () => {
    const r = await fetch(`/docs/${encodeURIComponent((window as unknown as { __converge: { docId: string } }).__converge.docId)}/hash`);
    const j = (await r.json()) as { hash: string; durable_seq: number };
    setServerHash(`${j.hash.slice(0, 12)} @ ${j.durable_seq}`);
  };
  const hash = client.hashHex();
  return (
    <div style={{ position: "absolute", right: 8, bottom: 8, background: "#fff", border: "1px solid #ddd", borderRadius: 8, padding: 8, width: open ? 320 : "auto", fontFamily: "ui-monospace, monospace", fontSize: 12 }}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
        <span data-testid="hash" title={hash}>hash {hash.slice(0, 12)}</span>
        <button onClick={() => setOpen(!open)}>{open ? "hide" : "debug"}</button>
      </div>
      {open && (
        <div style={{ display: "grid", gap: 6, marginTop: 8 }}>
          <div>replica {status.replica.toString(16)}</div>
          <div>seq {status.lastSeq.toString()} · pending {status.pending} · unacked {status.unacked} · unsaved {status.unsaved}</div>
          <label><input type="checkbox" checked={offline} onChange={(e) => { setOffline(e.target.checked); client.setOffline(e.target.checked); }} /> simulate offline</label>
          <button onClick={() => void client.flush()}>flush snapshot</button>
          <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <button onClick={() => void checkServer()}>server hash</button>
            <span data-testid="server-hash">{serverHash}</span>
          </div>
          <hr />
          <strong>Server chaos</strong>
          <label><input type="checkbox" checked={chaos.enabled} onChange={(e) => void apply({ ...chaos, enabled: e.target.checked })} /> enabled</label>
          <label>latency {chaos.latency_ms[0]}–{chaos.latency_ms[1]} ms
            <input type="range" min={0} max={1000} value={chaos.latency_ms[1]} onChange={(e) => void apply({ ...chaos, latency_ms: [Math.min(chaos.latency_ms[0], +e.target.value), +e.target.value] })} />
          </label>
          <label>drop {Math.round(chaos.drop_p * 100)}%
            <input type="range" min={0} max={90} value={chaos.drop_p * 100} onChange={(e) => void apply({ ...chaos, drop_p: +e.target.value / 100 })} />
          </label>
          <label>duplicate {Math.round(chaos.dup_p * 100)}%
            <input type="range" min={0} max={90} value={chaos.dup_p * 100} onChange={(e) => void apply({ ...chaos, dup_p: +e.target.value / 100 })} />
          </label>
          <label>disconnect every
            <input type="number" min={0} value={chaos.disconnect_every} onChange={(e) => void apply({ ...chaos, disconnect_every: +e.target.value })} style={{ width: 60, marginLeft: 6 }} />
          </label>
        </div>
      )}
    </div>
  );
}

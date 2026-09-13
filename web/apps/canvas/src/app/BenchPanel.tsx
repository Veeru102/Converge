import { useEffect, useRef, useState } from "react";
import { BenchStats, type BenchSnapshot, type Client } from "@converge/sync";
import { f64, fracV } from "../state/model.js";

const ms = (v: number | null | undefined) => (v === null || v === undefined ? "—" : v < 10 ? v.toFixed(1) : Math.round(v).toString());

/** Live numbers from `Client.onEvent`, plus two scripted runs that exercise the queue. */
export function BenchPanel({ client }: { client: Client }) {
  const stats = useRef(new BenchStats());
  const [snap, setSnap] = useState<BenchSnapshot | null>(null);
  const [running, setRunning] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    const off = client.onEvent((e) => stats.current.feed(e));
    const i = setInterval(() => setSnap(stats.current.stats(performance.now())), 500);
    return () => {
      off();
      clearInterval(i);
    };
  }, [client]);

  const waitFor = (pred: () => boolean, timeoutMs: number) =>
    new Promise<boolean>((resolve) => {
      const t0 = performance.now();
      const i = setInterval(() => {
        if (pred()) { clearInterval(i); resolve(true); }
        else if (performance.now() - t0 > timeoutMs) { clearInterval(i); resolve(false); }
      }, 20);
    });

  /** N property writes on a scratch object; measures how fast they all commit. */
  const burst = async (n: number, offlineFirst: boolean) => {
    setRunning(offlineFirst ? "offline burst" : "burst");
    setResult(null);
    const before = stats.current.stats(performance.now()).commitLatency.n;
    const scratch = client.edit({ op: "create", kind: "rect", props: [["x", f64(-10_000)], ["y", f64(-10_000)], ["w", f64(1)], ["h", f64(1)], ["z", fracV("A")]] }).id;
    if (offlineFirst) client.setOffline(true);
    const t0 = performance.now();
    for (let i = 0; i < n; i++) client.edit({ op: "set_props", object: scratch, entries: [["x", f64(-10_000 + i)]] });
    if (offlineFirst) {
      await waitFor(() => client.status().unsaved === 0, 10_000);
      client.setOffline(false);
    }
    const ok = await waitFor(() => client.status().unacked === 0 && client.status().unsaved === 0 && client.status().state === "live", 60_000);
    const total = performance.now() - t0;
    client.edit({ op: "delete", object: scratch });
    const after = stats.current.stats(performance.now());
    const newSamples = after.commitLatency.n - before;
    setResult(ok ? `${n} ops ${offlineFirst ? "queued offline and " : ""}committed in ${ms(total)} ms · ${(n / (total / 1000)).toFixed(0)} ops/s · ${newSamples} latency samples` : "timed out");
    setRunning(null);
  };

  const s = snap;
  return (
    <section data-testid="bench">
      <h4>Benchmarks</h4>
      <div className="stats">
        <div className="stat"><span className="v" data-testid="bench-latency">{ms(s?.commitLatency.p50)}<span className="faint"> / {ms(s?.commitLatency.p95)}</span></span><span className="l">commit latency p50 / p95 (ms)</span></div>
        <div className="stat"><span className="v">{s ? s.opsInPerSec.toFixed(1) : "—"}</span><span className="l">ops/s committed (5 s window)</span></div>
        <div className="stat"><span className="v">{ms(s?.reconnect.last)}</span><span className="l">last reconnect (ms)</span></div>
        <div className="stat"><span className="v">{ms(s?.drain.last)}</span><span className="l">last queue drain (ms)</span></div>
      </div>
      <div className="row">
        <button className="btn sm" data-testid="bench-burst" disabled={running !== null} onClick={() => void burst(200, false)}>Run burst (200 ops)</button>
        <button className="btn sm" data-testid="bench-offline-burst" disabled={running !== null} onClick={() => void burst(100, true)}>Offline burst + reconnect</button>
      </div>
      {(running || result) && <span className="faint" data-testid="bench-result">{running ? `running ${running}…` : result}</span>}
    </section>
  );
}

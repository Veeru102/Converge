/**
 * Headless benchmark: N clients on one document, a mixed workload, optional server chaos.
 *   pnpm bench --url http://127.0.0.1:8080 --clients 10 --ops 200 [--chaos hostile] [--json out.json]
 */
import { mkdirSync, writeFileSync } from "node:fs";
import {
  BenchStats,
  Client,
  MemoryStore,
  WebSocketTransport,
  type BenchSnapshot,
} from "@converge/sync";
import { wireCodec } from "@converge/protocol";

const args = new Map<string, string>();
for (let i = 2; i < process.argv.length; i += 2)
  args.set(process.argv[i]!.replace(/^--/, ""), process.argv[i + 1] ?? "");
const url = args.get("url") ?? "http://127.0.0.1:8080";
const clients = Number(args.get("clients") ?? 10);
const opsPerClient = Number(args.get("ops") ?? 200);
const chaosName = args.get("chaos") ?? "clean";
const jsonOut = args.get("json");
const wsUrl = url.replace(/^http/, "ws") + "/ws";

const PRESETS: Record<string, object> = {
  clean: { enabled: false },
  flaky: { enabled: true, latency_ms: [20, 150], drop_p: 0.05, dup_p: 0.02, disconnect_every: 0 },
  hostile: { enabled: true, latency_ms: [50, 400], drop_p: 0.3, dup_p: 0.3, disconnect_every: 15 },
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const fmt = (v: number) => (v < 10 ? v.toFixed(1) : Math.round(v).toString());

async function main() {
  const preset = PRESETS[chaosName];
  if (!preset) throw new Error(`unknown chaos preset ${chaosName}`);
  await fetch(`${url}/admin/chaos`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(preset),
  });
  const doc = ((await (await fetch(`${url}/docs`, { method: "POST" })).json()) as { id: string })
    .id;

  const runs: Array<{ client: Client; stats: BenchStats }> = [];
  const t0 = performance.now();
  for (let i = 0; i < clients; i++) {
    const client = new Client({
      docId: doc,
      user: { name: `bench-${i}`, color: 0x4f6ef7 },
      store: new MemoryStore(),
      transport: new WebSocketTransport(wsUrl, wireCodec),
      reconnectBackoffMs: [100, 2000],
    });
    const stats = new BenchStats(2000);
    client.onEvent((e) => stats.feed(e));
    await client.start();
    runs.push({ client, stats });
  }
  for (const r of runs) while (r.client.status().state !== "live") await sleep(20);
  const connectMs = performance.now() - t0;

  // Workload: each client creates a few objects then hammers set_props on them (the drag pattern).
  const t1 = performance.now();
  const objects = runs.map((r) =>
    Array.from(
      { length: 3 },
      (_, k) =>
        r.client.edit({
          op: "create",
          kind: "rect",
          props: [
            ["x", { t: "f64", v: k * 10 }],
            ["y", { t: "f64", v: 0 }],
            ["w", { t: "f64", v: 10 }],
            ["h", { t: "f64", v: 10 }],
            ["z", { t: "frac", v: `a${k}` }],
          ],
        }).id,
    ),
  );
  const perTick = 5;
  for (let n = 0; n < opsPerClient; n += perTick) {
    runs.forEach((r, i) => {
      for (let k = 0; k < perTick && n + k < opsPerClient; k++) {
        r.client.edit({
          op: "set_props",
          object: objects[i]![(n + k) % 3]!,
          entries: [["x", { t: "f64", v: n + k }]],
        });
      }
    });
    await sleep(33); // ~30 Hz like a drag
  }
  const submitMs = performance.now() - t1;

  // Drain.
  const deadline = performance.now() + 120_000;
  while (
    performance.now() < deadline &&
    runs.some(
      (r) =>
        r.client.status().unacked > 0 ||
        r.client.status().pending > 0 ||
        r.client.status().state !== "live",
    )
  )
    await sleep(50);
  const drainMs = performance.now() - t1;
  await fetch(`${url}/admin/chaos`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ enabled: false }),
  });

  // Convergence check against the server.
  await sleep(200);
  const srv = (await (await fetch(`${url}/docs/${doc}/hash`)).json()) as {
    hash: string;
    durable_seq: number;
    sessions: number;
  };
  const converged = runs.every(
    (r) => r.client.hashHex() === srv.hash && r.client.status().lastSeq === BigInt(srv.durable_seq),
  );

  const all: number[] = [];
  const perClient: BenchSnapshot[] = runs.map((r) => {
    all.push(...r.stats.latencies);
    return r.stats.stats(performance.now());
  });
  all.sort((a, b) => a - b);
  const q = (p: number) => (all.length ? all[Math.max(0, Math.ceil(p * all.length) - 1)]! : 0);
  const totalOps = clients * (opsPerClient + 3);
  const reconnects = perClient.reduce((s, c) => s + c.reconnect.n, 0);
  const reconnectAll = runs.flatMap((r) =>
    Array.from({ length: r.stats.stats(0).reconnect.n }, () => r.stats.stats(0).reconnect.p50),
  );

  const summary = {
    url,
    clients,
    opsPerClient,
    chaos: chaosName,
    doc,
    connectMs: Math.round(connectMs),
    submitMs: Math.round(submitMs),
    drainMs: Math.round(drainMs),
    totalOps,
    opsPerSec: Math.round(totalOps / (drainMs / 1000)),
    latencyMs: {
      p50: q(0.5),
      p95: q(0.95),
      p99: q(0.99),
      max: all[all.length - 1] ?? 0,
      n: all.length,
    },
    reconnects,
    reconnectP50Ms: reconnectAll.length
      ? reconnectAll.sort((a, b) => a - b)[Math.floor(reconnectAll.length / 2)]!
      : null,
    serverSessions: srv.sessions,
    serverSeq: srv.durable_seq,
    converged,
  };

  console.log(`\nConverge bench — ${clients} clients × ${opsPerClient} ops, chaos=${chaosName}`);
  console.log(`  connect all      ${fmt(summary.connectMs)} ms`);
  console.log(
    `  submit phase     ${fmt(summary.submitMs)} ms   drain to converged ${fmt(summary.drainMs)} ms`,
  );
  console.log(`  throughput       ${summary.opsPerSec} ops/s committed (${totalOps} ops)`);
  console.log(
    `  commit latency   p50 ${fmt(summary.latencyMs.p50)} · p95 ${fmt(summary.latencyMs.p95)} · p99 ${fmt(summary.latencyMs.p99)} · max ${fmt(summary.latencyMs.max)} ms  (n=${summary.latencyMs.n})`,
  );
  console.log(
    `  reconnects       ${reconnects}${summary.reconnectP50Ms !== null ? ` · p50 ${fmt(summary.reconnectP50Ms)} ms` : ""}`,
  );
  console.log(
    `  server           ${srv.sessions} sessions · durable seq ${srv.durable_seq} · ${converged ? "all clients converged ✓" : "NOT CONVERGED ✗"}\n`,
  );

  if (jsonOut) {
    mkdirSync(jsonOut.replace(/\/[^/]*$/, ""), { recursive: true });
    writeFileSync(jsonOut, JSON.stringify({ ...summary, perClient }, null, 2));
    console.log(`  wrote ${jsonOut}`);
  }
  for (const r of runs) await r.client.stop();
  process.exit(converged ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});

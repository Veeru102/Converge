import { useEffect, useRef, useState } from "react";
import type { Client, ClientStatus } from "@converge/sync";

export type SyncKind = "connecting" | "live" | "reconnecting" | "offline";

export interface ServerView {
  hash: string;
  durableSeq: bigint;
  sessions: number;
  at: number;
}

export interface SyncView {
  kind: SyncKind;
  status: ClientStatus;
  retryInMs: number | null;
  server: ServerView | null;
  /** Live, nothing queued, and the server's durable hash equals ours. */
  converged: boolean;
  /** Live and nothing queued (server not yet consulted). */
  synced: boolean;
  localHash: string;
}

export async function fetchServerView(docId: string): Promise<ServerView | null> {
  try {
    const r = await fetch(`/docs/${encodeURIComponent(docId)}/hash`);
    if (!r.ok) return null;
    const j = (await r.json()) as { hash: string; durable_seq: number; sessions: number };
    return { hash: j.hash, durableSeq: BigInt(j.durable_seq), sessions: j.sessions, at: Date.now() };
  } catch {
    return null;
  }
}

/** Derives the user-facing sync state and, when `probe` is on, checks the server hash. */
export function useSyncStatus(client: Client, docId: string, status: ClientStatus, version: number, probe: boolean): SyncView {
  const [server, setServer] = useState<ServerView | null>(null);
  const [tick, setTick] = useState(0);
  const inflight = useRef(false);
  const quiet = status.state === "live" && status.pending === 0 && status.unsaved === 0;

  useEffect(() => {
    if (!probe) return;
    let cancelled = false;
    const run = async () => {
      if (inflight.current) return;
      inflight.current = true;
      const v = await fetchServerView(docId);
      inflight.current = false;
      if (!cancelled && v) setServer(v);
    };
    const t = setTimeout(run, quiet ? 150 : 600);
    const i = setInterval(run, 2000);
    return () => {
      cancelled = true;
      clearTimeout(t);
      clearInterval(i);
    };
  }, [probe, docId, quiet, version, status.lastSeq]);

  // Countdown re-render while backing off.
  useEffect(() => {
    if (status.retryAt === null) return;
    const i = setInterval(() => setTick((n) => n + 1), 250);
    return () => clearInterval(i);
  }, [status.retryAt]);
  void tick;

  const kind: SyncKind = status.offline ? "offline" : status.state === "live" ? "live" : status.retryAt !== null ? "reconnecting" : "connecting";
  const localHash = client.hashHex();
  const converged = quiet && server !== null && server.hash === localHash && server.durableSeq === status.lastSeq;
  return {
    kind, status, server, converged, synced: quiet, localHash,
    retryInMs: status.retryAt === null ? null : Math.max(0, status.retryAt - Date.now()),
  };
}

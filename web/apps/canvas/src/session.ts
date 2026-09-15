/** Creates the Client for the document named in the URL and exposes test hooks. */
import { Client, IndexedDbStore, WebSocketTransport } from "@converge/sync";
import { wireCodec } from "@converge/protocol";

export interface Session {
  client: Client;
  docId: string;
  userName: string;
  color: number;
}

const PALETTE = [0xe6194b, 0x3cb44b, 0x4363d8, 0xf58231, 0x911eb4, 0x42d4f4, 0xf032e6, 0x9a6324];

export async function startSession(): Promise<Session> {
  const params = new URLSearchParams(location.search);
  let docId = params.get("doc");
  if (!docId) {
    const res = await fetch("/docs", { method: "POST" });
    docId = ((await res.json()) as { id: string }).id;
    params.set("doc", docId);
    history.replaceState(null, "", `?${params}`);
  }
  const userName = params.get("name") ?? `user-${Math.random().toString(36).slice(2, 6)}`;
  const color = PALETTE[Math.abs(hashCode(userName)) % PALETTE.length]!;
  const store = await IndexedDbStore.open(docId);
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const transport = new WebSocketTransport(`${proto}://${location.host}/ws`, wireCodec);
  const debug = params.has("debug");
  const client = new Client({
    docId,
    user: { name: userName, color },
    store,
    transport,
    snapshotIdleMs: 1_000,
    snapshotEveryOps: 100,
    ...(debug ? { log: (l: string) => console.log(l) } : {}),
  });
  if (params.has("offline")) client.setOffline(true); // start without connecting (tests, demos)
  await client.start();
  // Test hooks (Playwright reads these).
  (window as unknown as { __converge: unknown }).__converge = {
    client,
    docId,
    hash: () => client.hashHex(),
    status: () => {
      const s = client.status();
      return { ...s, replica: s.replica.toString(), lastSeq: s.lastSeq.toString() };
    },
    objectCount: () => client.document.renderOrder().length,
    setOffline: (v: boolean) => client.setOffline(v),
    flush: () => client.flush(),
  };
  return { client, docId, userName, color };
}

function hashCode(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (Math.imul(31, h) + s.charCodeAt(i)) | 0;
  return h;
}

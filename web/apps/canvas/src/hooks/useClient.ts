import { useEffect, useMemo, useState } from "react";
import type { Client, ClientStatus } from "@converge/sync";
import type { PresenceEntry } from "@converge/protocol";

/** Subscribes to a Client; `version` bumps on every document change. */
export function useClient(client: Client) {
  const [version, setVersion] = useState(0);
  const [status, setStatus] = useState<ClientStatus>(() => client.status());
  const [presence, setPresence] = useState<PresenceEntry[]>(() => client.presence());
  useEffect(() => {
    const offs = [
      client.onChange(() => setVersion((n) => n + 1)),
      client.onStatus(setStatus),
      client.onPresence(setPresence),
    ];
    return () => offs.forEach((f) => f());
  }, [client]);
  // `version` is the dependency that matters: the document is mutated in place.
  const objects = useMemo(() => client.document.renderOrder(), [client, version]);
  return { version, status, presence, objects };
}

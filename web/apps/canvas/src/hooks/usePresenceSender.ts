import { useMemo } from "react";
import type { Client } from "@converge/sync";
import type { PresenceState } from "@converge/protocol";

/** Presence at ≤ 20 Hz with a trailing send; the last state always goes out. */
export function usePresenceSender(client: Client, hz = 20) {
  return useMemo(() => {
    const interval = 1000 / hz;
    let last = 0;
    let next: PresenceState | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    return (state: PresenceState) => {
      const now = performance.now();
      if (now - last >= interval) {
        last = now;
        client.sendPresence(state);
        return;
      }
      next = state;
      timer ??= setTimeout(
        () => {
          timer = null;
          if (next) {
            last = performance.now();
            client.sendPresence(next);
            next = null;
          }
        },
        interval - (now - last),
      );
    };
  }, [client, hz]);
}

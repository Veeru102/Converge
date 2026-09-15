import { useEffect, useState } from "react";
import { startSession, type Session } from "./session.js";
import { Shell } from "./app/Shell.js";

let sessionPromise: Promise<Session> | null = null;

export function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    sessionPromise ??= startSession();
    sessionPromise.then(setSession, (e) => setError(String(e)));
  }, []);
  if (error) return <div style={{ padding: 24 }}>Failed to start: {error}</div>;
  if (!session)
    return (
      <div style={{ padding: 24 }} className="muted">
        Loading…
      </div>
    );
  return <Shell session={session} />;
}

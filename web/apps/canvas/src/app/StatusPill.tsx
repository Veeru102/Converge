import type { SyncView } from "../hooks/useSyncStatus.js";

/** One glance: are we live, is anything queued, does the server agree with us. */
export function StatusPill({ view, onClick }: { view: SyncView; onClick: () => void }) {
  const { kind, status } = view;
  let cls = "neutral",
    label = "",
    sub = "";
  const queued = status.pending + status.unsaved;
  if (kind === "offline") {
    cls = "danger";
    label = "Offline";
    sub = queued ? `${queued} queued` : "";
  } else if (kind === "reconnecting") {
    cls = "warn";
    label = "Reconnecting";
    sub = view.retryInMs !== null ? `in ${Math.ceil(view.retryInMs / 1000)}s` : "";
  } else if (kind === "connecting") {
    cls = "warn";
    label = "Connecting";
  } else if (queued > 0) {
    cls = "warn";
    label = "Syncing";
    sub = `${queued}`;
  } else if (view.converged) {
    cls = "ok";
    label = "Converged";
    sub = `seq ${status.lastSeq}`;
  } else {
    cls = "ok";
    label = "Synced";
    sub = `seq ${status.lastSeq}`;
  }
  return (
    <button
      className={`pill ${cls}`}
      onClick={onClick}
      data-testid="status"
      data-state={label.toLowerCase()}
      title="Open Network Lab"
    >
      <span className="dot" />
      {label}
      {sub && <span className="sub">{sub}</span>}
    </button>
  );
}

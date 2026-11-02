import {
  formatBytes,
  formatEta,
  formatSpeed,
  type TransferStats,
} from "../lib/transferProgress";

function formatElapsed(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}:${r.toString().padStart(2, "0")}`;
}

export function TransferProgressBar({
  stats,
  label = "Syncing to host",
  doneLabel = "Synced to host",
  doneMeta = "sent",
  done = false,
  error,
  elapsedSec,
}: {
  stats: TransferStats;
  label?: string;
  doneLabel?: string;
  /** Word after byte total when done — "sent" or "received". */
  doneMeta?: string;
  done?: boolean;
  error?: string | null;
  /** Optional elapsed timer shown in the meta row. */
  elapsedSec?: number | null;
}) {
  const pctLabel = done
    ? "100%"
    : `${stats.pct < 10 ? stats.pct.toFixed(1) : Math.round(stats.pct)}%`;
  const width = done ? 100 : Math.max(stats.pct, stats.bytes > 0 ? 1.5 : 0);

  return (
    <div
      className={`rs-xfer${done ? " is-done" : ""}${error ? " is-error" : ""}`}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(done ? 100 : stats.pct)}
      aria-label={label}
    >
      <div className="rs-xfer-top">
        <span className="rs-xfer-label">
          {error ? "Sync paused" : done ? doneLabel : label}
        </span>
        <span className="rs-xfer-pct">{error ? "—" : pctLabel}</span>
      </div>
      <div className="rs-xfer-track">
        <span className="rs-xfer-fill" style={{ width: `${width}%` }} />
      </div>
      <div className="rs-xfer-meta">
        {error ? (
          <span className="rs-xfer-error">{error}</span>
        ) : done ? (
          <>
            <span>
              {formatBytes(stats.total)} {doneMeta}
            </span>
            {elapsedSec != null && (
              <span>{formatElapsed(elapsedSec)} elapsed</span>
            )}
          </>
        ) : (
          <>
            <span>
              {formatBytes(stats.bytes)} / {formatBytes(stats.total)}
            </span>
            <span>{formatSpeed(stats.bytesPerSec)}</span>
            <span>{formatEta(stats.etaSec)}</span>
            {elapsedSec != null && (
              <span>{formatElapsed(elapsedSec)} elapsed</span>
            )}
          </>
        )}
      </div>
    </div>
  );
}

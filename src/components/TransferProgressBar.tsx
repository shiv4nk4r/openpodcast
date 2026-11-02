import {
  formatBytes,
  formatEta,
  formatSpeed,
  type TransferStats,
} from "../lib/transferProgress";

export function TransferProgressBar({
  stats,
  label = "Syncing to host",
  done = false,
  error,
}: {
  stats: TransferStats;
  label?: string;
  done?: boolean;
  error?: string | null;
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
          {error ? "Sync paused" : done ? "Synced to host" : label}
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
          <span>
            {formatBytes(stats.total)} sent
          </span>
        ) : (
          <>
            <span>
              {formatBytes(stats.bytes)} / {formatBytes(stats.total)}
            </span>
            <span>{formatSpeed(stats.bytesPerSec)}</span>
            <span>{formatEta(stats.etaSec)}</span>
          </>
        )}
      </div>
    </div>
  );
}

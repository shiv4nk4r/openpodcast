/** Shared helpers for sync/upload progress UI (%, speed, ETA). */

export type TransferSample = {
  bytes: number;
  total: number;
  at: number;
};

export type TransferStats = {
  bytes: number;
  total: number;
  pct: number;
  /** Smoothed bytes/sec */
  bytesPerSec: number;
  /** Seconds remaining; null while unknown */
  etaSec: number | null;
};

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0 B";
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  if (n < 1024 * 1024 * 1024) {
    return `${(n / (1024 * 1024)).toFixed(n < 10 * 1024 * 1024 ? 1 : 0)} MB`;
  }
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatSpeed(bytesPerSec: number): string {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec < 1) return "—";
  return `${formatBytes(bytesPerSec)}/s`;
}

export function formatEta(sec: number | null): string {
  if (sec == null || !Number.isFinite(sec) || sec < 0) return "Calculating…";
  if (sec < 1) return "Almost done";
  if (sec < 60) return `~${Math.ceil(sec)}s left`;
  const m = Math.floor(sec / 60);
  const s = Math.ceil(sec % 60);
  if (m < 60) return s > 0 ? `~${m}m ${s}s left` : `~${m}m left`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return rm > 0 ? `~${h}h ${rm}m left` : `~${h}h left`;
}

/**
 * Rolling speed from recent samples. Uses EMA so short stalls don't spike ETA.
 */
export function computeTransferStats(
  samples: TransferSample[],
  now = Date.now(),
): TransferStats {
  const latest = samples[samples.length - 1];
  if (!latest) {
    return { bytes: 0, total: 1, pct: 0, bytesPerSec: 0, etaSec: null };
  }
  const total = Math.max(1, latest.total);
  const bytes = Math.min(latest.bytes, total);
  const pct = Math.min(100, Math.max(0, (bytes / total) * 100));

  // Prefer a ~2–4s window for instantaneous rate.
  let bytesPerSec = 0;
  for (let i = samples.length - 1; i > 0; i--) {
    const a = samples[i - 1];
    const b = samples[i];
    const dt = (b.at - a.at) / 1000;
    const db = b.bytes - a.bytes;
    if (dt >= 0.05 && db >= 0) {
      const inst = db / dt;
      bytesPerSec = bytesPerSec === 0 ? inst : bytesPerSec * 0.65 + inst * 0.35;
    }
  }

  // Fallback: average since first sample if EMA never engaged.
  if (bytesPerSec < 1 && samples.length >= 2) {
    const first = samples[0];
    const dt = (latest.at - first.at) / 1000;
    const db = latest.bytes - first.bytes;
    if (dt > 0.25 && db > 0) bytesPerSec = db / dt;
  }

  // Ignore tiny rates until we have a real window.
  const ageSec = (now - (samples[0]?.at ?? now)) / 1000;
  const etaSec =
    bytesPerSec >= 8 * 1024 && ageSec >= 0.6 && bytes < total
      ? (total - bytes) / bytesPerSec
      : bytes >= total
        ? 0
        : null;

  return { bytes, total, pct, bytesPerSec, etaSec };
}

/** Keep a bounded sample history for speed estimation. */
export function pushSample(
  prev: TransferSample[],
  bytes: number,
  total: number,
  at = Date.now(),
  max = 24,
): TransferSample[] {
  const next = [...prev, { bytes, total, at }];
  // Drop samples older than 8s, but keep at least 2.
  const cutoff = at - 8000;
  const trimmed = next.filter((s, i) => s.at >= cutoff || i >= next.length - 2);
  return trimmed.length > max ? trimmed.slice(trimmed.length - max) : trimmed;
}

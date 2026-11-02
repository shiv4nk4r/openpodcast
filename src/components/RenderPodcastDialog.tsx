import { IconMicrophone2, IconMovie } from "@tabler/icons-react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { Segmented } from "./Select";
import {
  FRAME_RATE_OPTIONS,
  RESOLUTION_OPTIONS,
} from "../lib/studioQuality";
import { formatEta } from "../lib/transferProgress";
import type { RenderLayout } from "../types";

const LAYOUT_OPTIONS: {
  id: RenderLayout;
  label: string;
  description: string;
}[] = [
  {
    id: "dynamic",
    label: "Dynamic",
    description: "Active speaker grows; others shrink beside them",
  },
  {
    id: "equal",
    label: "Equal grid",
    description: "Same-size tiles for everyone present",
  },
  {
    id: "normal",
    label: "Normal",
    description: "Host large up top; guests along the bottom",
  },
];

export type RenderMode = "video" | "audio";

type RenderProgressPayload = {
  pct: number;
  out_time_secs: number;
  duration_secs: number;
  eta_secs: number | null;
  phase: string;
};

function formatClock(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}:${r.toString().padStart(2, "0")}`;
}

export function RenderPodcastDialog({
  open,
  layout,
  resolution,
  fps,
  busy = false,
  busyMode = null,
  status = null,
  hasPresence = false,
  onLayoutChange,
  onResolutionChange,
  onFpsChange,
  onClose,
  onConfirm,
}: {
  open: boolean;
  layout: RenderLayout;
  resolution: string;
  fps: number;
  busy?: boolean;
  busyMode?: RenderMode | null;
  status?: string | null;
  hasPresence?: boolean;
  onLayoutChange: (layout: RenderLayout) => void;
  onResolutionChange: (resolution: string) => void;
  onFpsChange: (fps: number) => void;
  onClose: () => void;
  onConfirm: (mode: RenderMode) => void | Promise<void>;
}) {
  const [progress, setProgress] = useState<RenderProgressPayload | null>(null);

  useEffect(() => {
    if (!open || !busy) {
      setProgress(null);
      return;
    }
    setProgress({
      pct: 0,
      out_time_secs: 0,
      duration_secs: 0,
      eta_secs: null,
      phase: "encoding",
    });
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    void listen<RenderProgressPayload>("render-progress", (ev) => {
      if (!cancelled) setProgress(ev.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [open, busy]);

  if (!open) return null;

  const showProgress = busy;
  const done = !busy && !!status;
  const pct = progress?.pct ?? 0;
  const pctLabel =
    pct < 10 && pct > 0 ? pct.toFixed(1) : `${Math.round(pct)}%`;
  const barWidth = Math.max(pct, progress && progress.out_time_secs > 0 ? 1.5 : 0);
  const modeLabel =
    busyMode === "audio" ? "audio podcast" : "video podcast";
  const resLabel =
    RESOLUTION_OPTIONS.find((o) => o.value === resolution)?.label ?? resolution;

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onClick={() => {
        if (!busy) onClose();
      }}
    >
      <div
        className="modal render-podcast-modal"
        role="dialog"
        aria-modal
        aria-labelledby="render-podcast-title"
        aria-describedby="render-podcast-desc"
        onClick={(e) => e.stopPropagation()}
      >
        {showProgress ? (
          <>
            <h3 id="render-podcast-title">
              Rendering {busyMode === "audio" ? "audio" : "video"}…
            </h3>
            <p id="render-podcast-desc" className="render-podcast-copy">
              Encoding your {modeLabel}
              {busyMode === "video" ? ` at ${resLabel} / ${fps}fps` : ""}. Keep
              this window open until it finishes.
            </p>

            <div
              className="rs-xfer render-podcast-progress"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(pct)}
              aria-label="Render progress"
            >
              <div className="rs-xfer-top">
                <span className="rs-xfer-label">
                  {progress?.phase === "done" ? "Finishing…" : "Encoding"}
                </span>
                <span className="rs-xfer-pct">{pctLabel}</span>
              </div>
              <div className="rs-xfer-track">
                <span
                  className="rs-xfer-fill"
                  style={{ width: `${barWidth}%` }}
                />
              </div>
              <div className="rs-xfer-meta">
                <span>
                  {formatClock(progress?.out_time_secs ?? 0)}
                  {progress && progress.duration_secs > 0
                    ? ` / ${formatClock(progress.duration_secs)}`
                    : ""}
                </span>
                <span>{formatEta(progress?.eta_secs ?? null)}</span>
              </div>
            </div>
          </>
        ) : (
          <>
            <h3 id="render-podcast-title">
              {done ? "Render complete" : "Final render settings"}
            </h3>
            {!done && (
              <p id="render-podcast-desc" className="render-podcast-copy">
                Pick layout and output quality for the stage export, then choose
                video or audio-only.
                {hasPresence
                  ? " Join and leave times are baked into the export automatically."
                  : ""}
              </p>
            )}

            {!done && (
              <>
                <p className="render-podcast-section-label">Video layout</p>
                <div className="playback-layout-options render-podcast-layouts">
                  {LAYOUT_OPTIONS.map((opt) => (
                    <button
                      key={opt.id}
                      type="button"
                      className={`playback-layout-option${
                        layout === opt.id ? " is-active" : ""
                      }`}
                      disabled={busy}
                      onClick={() => onLayoutChange(opt.id)}
                    >
                      <strong>{opt.label}</strong>
                      <span>{opt.description}</span>
                    </button>
                  ))}
                </div>

                <div className="render-podcast-quality">
                  <Segmented
                    label="Resolution"
                    value={resolution}
                    onChange={onResolutionChange}
                    options={RESOLUTION_OPTIONS.map((o) => ({
                      value: o.value,
                      label: o.label,
                    }))}
                  />
                  <Segmented
                    label="Frame rate"
                    value={String(fps)}
                    onChange={(v) => onFpsChange(Number(v))}
                    options={FRAME_RATE_OPTIONS.map((o) => ({
                      value: o.value,
                      label: o.label.replace(" fps", ""),
                    }))}
                  />
                </div>
              </>
            )}

            {status && <p className="playback-render-msg">{status}</p>}

            <div className="render-podcast-actions">
              <button
                type="button"
                className="btn btn-secondary"
                disabled={busy}
                onClick={onClose}
              >
                {done ? "Close" : "Cancel"}
              </button>
              {!done && (
                <>
                  <button
                    type="button"
                    className="btn btn-secondary"
                    disabled={busy}
                    onClick={() => void onConfirm("audio")}
                  >
                    <IconMicrophone2 size={16} stroke={1.75} />
                    Render audio
                  </button>
                  <button
                    type="button"
                    className="btn btn-primary"
                    disabled={busy}
                    onClick={() => void onConfirm("video")}
                  >
                    <IconMovie size={16} stroke={1.75} />
                    Render video
                  </button>
                </>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

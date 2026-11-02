import { IconMovie } from "@tabler/icons-react";
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

export function RenderPodcastDialog({
  open,
  layout,
  busy = false,
  status = null,
  hasPresence = false,
  onLayoutChange,
  onClose,
  onConfirm,
}: {
  open: boolean;
  layout: RenderLayout;
  busy?: boolean;
  status?: string | null;
  hasPresence?: boolean;
  onLayoutChange: (layout: RenderLayout) => void;
  onClose: () => void;
  onConfirm: () => void | Promise<void>;
}) {
  if (!open) return null;

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
        <h3 id="render-podcast-title">Final render settings</h3>
        <p id="render-podcast-desc" className="render-podcast-copy">
          Choose how the finished podcast should look. The stage behind this
          dialog previews your layout.
          {hasPresence
            ? " Join and leave times are baked into the export automatically."
            : ""}
        </p>

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

        {status && <p className="playback-render-msg">{status}</p>}

        <div className="render-podcast-actions">
          <button
            type="button"
            className="btn btn-secondary"
            disabled={busy}
            onClick={onClose}
          >
            {status && !busy ? "Close" : "Cancel"}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy}
            onClick={() => void onConfirm()}
          >
            <IconMovie size={16} stroke={1.75} />
            {busy ? "Rendering…" : "Start render"}
          </button>
        </div>
      </div>
    </div>
  );
}

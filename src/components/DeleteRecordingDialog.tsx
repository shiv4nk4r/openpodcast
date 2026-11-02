import { useEffect, useState } from "react";

type Step = 1 | 2;

export function DeleteRecordingDialog({
  open,
  recordingName,
  busy = false,
  onClose,
  onConfirm,
}: {
  open: boolean;
  recordingName: string;
  busy?: boolean;
  onClose: () => void;
  onConfirm: () => void | Promise<void>;
}) {
  const [step, setStep] = useState<Step>(1);

  useEffect(() => {
    if (open) setStep(1);
  }, [open]);

  if (!open) return null;

  const title =
    step === 1 ? "Delete this recording?" : "Delete permanently?";
  const body =
    step === 1
      ? `“${recordingName}” will be removed from your library. Media files on this machine will be deleted.`
      : "This cannot be undone. The recording and all local files will be gone forever.";

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onClick={() => {
        if (!busy) onClose();
      }}
    >
      <div
        className="modal delete-recording-modal"
        role="alertdialog"
        aria-modal
        aria-labelledby="delete-recording-title"
        aria-describedby="delete-recording-desc"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 id="delete-recording-title">{title}</h3>
        <p id="delete-recording-desc" className="delete-recording-copy">
          {body}
        </p>
        <div className="delete-recording-actions">
          <button
            type="button"
            className="btn btn-secondary"
            disabled={busy}
            onClick={onClose}
          >
            Cancel
          </button>
          {step === 1 ? (
            <button
              type="button"
              className="btn btn-danger"
              disabled={busy}
              onClick={() => setStep(2)}
            >
              Yes, delete
            </button>
          ) : (
            <button
              type="button"
              className="btn btn-danger"
              disabled={busy}
              onClick={() => void onConfirm()}
            >
              {busy ? "Deleting…" : "Delete permanently"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

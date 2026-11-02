import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  IconArrowLeft,
  IconCheck,
  IconMicrophone2,
  IconPlayerPlay,
  IconSettings,
  IconTrash,
  IconUpload,
  IconVideo,
  IconWaveSine,
} from "@tabler/icons-react";
import { api } from "../api";
import { DeleteRecordingDialog } from "../components/DeleteRecordingDialog";
import { TransferProgressBar } from "../components/TransferProgressBar";
import {
  computeTransferStats,
  pushSample,
  type TransferSample,
  type TransferStats,
} from "../lib/transferProgress";
import { useApp } from "../state";
import type { SessionRecord } from "../types";

type PendingTransfer = {
  id: string;
  host_session_id: string;
  participant_id: string;
  invite: string;
  local_session_id: string;
  file_names: string[];
  last_error: string | null;
};

type SyncProgress = {
  sessionId: string;
  bytes: number;
  total: number;
  fileName?: string;
  status: "sending" | "done" | "error";
  error?: string;
  samples: TransferSample[];
};

function formatDuration(secs: number) {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function statsFor(prog: SyncProgress | undefined, done: boolean): TransferStats {
  if (done) {
    const total = prog?.total || prog?.bytes || 1;
    return {
      bytes: total,
      total,
      pct: 100,
      bytesPerSec: 0,
      etaSec: 0,
    };
  }
  if (!prog) {
    return { bytes: 0, total: 1, pct: 0, bytesPerSec: 0, etaSec: null };
  }
  return computeTransferStats(prog.samples.length ? prog.samples : [
    { bytes: prog.bytes, total: prog.total, at: Date.now() },
  ]);
}

function SyncRing({
  frac,
  busy,
  done,
  title,
  onClick,
  disabled,
}: {
  frac: number;
  busy: boolean;
  done: boolean;
  title: string;
  onClick: () => void;
  disabled?: boolean;
}) {
  const size = 34;
  const stroke = 3;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const clamped = Number.isFinite(frac)
    ? Math.min(1, Math.max(0, frac))
    : 0;
  const dash = c * clamped;
  const gap = Math.max(0, c - dash);
  return (
    <button
      type="button"
      className={`rs-rec-sync${busy ? " is-busy" : ""}${done ? " is-done" : ""}`}
      title={title}
      aria-label={title}
      disabled={Boolean(disabled || (busy && !done))}
      onClick={onClick}
    >
      <svg
        className="rs-rec-sync-ring"
        width={size}
        height={size}
        viewBox={`0 0 ${size} ${size}`}
        aria-hidden
      >
        <circle
          className="rs-rec-sync-track"
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          strokeWidth={stroke}
        />
        {(busy || done) && (
          <circle
            className="rs-rec-sync-value"
            cx={size / 2}
            cy={size / 2}
            r={r}
            fill="none"
            strokeWidth={stroke}
            strokeDasharray={`${dash} ${gap}`}
            strokeLinecap="round"
            transform={`rotate(-90 ${size / 2} ${size / 2})`}
          />
        )}
      </svg>
      <span className="rs-rec-sync-icon" aria-hidden>
        {done ? (
          <IconCheck size={14} stroke={2.25} />
        ) : (
          <IconUpload size={14} stroke={1.75} />
        )}
      </span>
    </button>
  );
}

export default function Dashboard() {
  const { sessions: sessionsRaw, refreshSessions } = useApp();
  const sessions = Array.isArray(sessionsRaw) ? sessionsRaw : [];
  const navigate = useNavigate();
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [editingId, setEditingId] = useState<string | null>(null);
  const [nameDraft, setNameDraft] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<SessionRecord | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [pendingTransfers, setPendingTransfers] = useState<PendingTransfer[]>(
    [],
  );
  const [syncProgress, setSyncProgress] = useState<
    Record<string, SyncProgress>
  >({});
  const [syncedIds, setSyncedIds] = useState<Record<string, boolean>>({});
  const activeSyncSession = useRef<string | null>(null);
  const samplesRef = useRef<Record<string, TransferSample[]>>({});
  const nameInputRef = useRef<HTMLInputElement | null>(null);

  async function refreshPending() {
    try {
      const list = await api.listPendingTransfers();
      setPendingTransfers(Array.isArray(list) ? list : []);
    } catch {
      setPendingTransfers([]);
    }
  }

  useEffect(() => {
    void refreshSessions();
    void refreshPending();
  }, [refreshSessions]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        unlisten = await listen<{
          participant_id: string;
          file_name: string;
          status: string;
          bytes_received: number;
          total_bytes: number;
          overall_bytes?: number;
          overall_total?: number;
        }>("studio-transfer", (ev) => {
          const sessionId = activeSyncSession.current;
          if (!sessionId || !ev.payload) return;
          const p = ev.payload;
          const total = Math.max(
            1,
            p.overall_total || p.total_bytes || 1,
          );
          const bytes = Math.min(
            total,
            p.overall_bytes || p.bytes_received || 0,
          );
          const samples = pushSample(
            samplesRef.current[sessionId] ?? [],
            bytes,
            total,
          );
          samplesRef.current[sessionId] = samples;
          setSyncProgress((prev) => ({
            ...prev,
            [sessionId]: {
              sessionId,
              bytes,
              total,
              fileName: p.file_name,
              samples,
              status:
                p.status === "error"
                  ? "error"
                  : "sending",
            },
          }));
        });
      } catch {
        /* browser preview */
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const next: Record<string, string> = {};
      await Promise.all(
        sessions.map(async (s) => {
          try {
            const path = await api.getSessionThumbnail(s.id);
            if (path) next[s.id] = convertFileSrc(path);
          } catch {
            /* no thumb */
          }
        }),
      );
      if (!cancelled) setThumbs(next);
    })();
    return () => {
      cancelled = true;
    };
  }, [sessions]);

  useEffect(() => {
    if (!editingId) return;
    const el = nameInputRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, [editingId]);

  const pendingBySession = useMemo(() => {
    const map = new Map<string, PendingTransfer>();
    for (const item of pendingTransfers) {
      map.set(item.local_session_id, item);
    }
    return map;
  }, [pendingTransfers]);

  /** Pending rows that don't yet have a library card. */
  const orphanPending = useMemo(
    () =>
      pendingTransfers.filter(
        (p) => !sessions.some((s) => s.id === p.local_session_id),
      ),
    [pendingTransfers, sessions],
  );

  function beginRename(id: string, current: string) {
    if (renaming) return;
    setEditingId(id);
    setNameDraft(current);
  }

  async function commitRename() {
    if (!editingId || renaming) return;
    const id = editingId;
    const current = sessions.find((s) => s.id === id)?.name ?? "";
    const next = nameDraft.trim();
    setEditingId(null);
    if (!next || next === current) {
      setNameDraft(current);
      return;
    }
    setRenaming(true);
    try {
      await api.renameSession(id, next);
      await refreshSessions();
    } catch (e) {
      alert(String(e));
    } finally {
      setRenaming(false);
    }
  }

  function cancelRename() {
    setEditingId(null);
    setNameDraft("");
  }

  async function confirmDelete() {
    if (!pendingDelete || deleting) return;
    setDeleting(true);
    try {
      await api.deleteSession(pendingDelete.id);
      setPendingDelete(null);
      await refreshSessions();
      await refreshPending();
    } catch (e) {
      alert(String(e));
    } finally {
      setDeleting(false);
    }
  }

  async function syncToHost(item: PendingTransfer) {
    if (activeSyncSession.current) return;
    const sid = item.local_session_id;
    activeSyncSession.current = sid;
    samplesRef.current[sid] = [];
    setSyncProgress((prev) => ({
      ...prev,
      [sid]: {
        sessionId: sid,
        bytes: 0,
        total: 1,
        samples: [],
        status: "sending",
      },
    }));
    try {
      await api.returnLocalTracks(
        item.local_session_id,
        item.participant_id,
        item.invite,
        item.host_session_id,
      );
      setSyncedIds((prev) => ({ ...prev, [sid]: true }));
      setSyncProgress((prev) => {
        const cur = prev[sid];
        const total = Math.max(1, cur?.total ?? 1);
        return {
          ...prev,
          [sid]: {
            sessionId: sid,
            bytes: total,
            total,
            samples: cur?.samples ?? [],
            status: "done",
          },
        };
      });
      await refreshPending();
    } catch (e) {
      const msg = String(e).replace(/^Error:\s*/i, "");
      setSyncProgress((prev) => ({
        ...prev,
        [sid]: {
          sessionId: sid,
          bytes: prev[sid]?.bytes ?? 0,
          total: prev[sid]?.total ?? 1,
          samples: prev[sid]?.samples ?? [],
          status: "error",
          error: msg,
        },
      }));
      await refreshPending();
      alert(
        `${msg}\n\nHost must still have the studio open. Your files stay on this computer — tap Sync to host again to resume.`,
      );
    } finally {
      activeSyncSession.current = null;
    }
  }

  function syncUiForSession(sessionId: string) {
    const pending = pendingBySession.get(sessionId);
    const prog = syncProgress[sessionId];
    const done = Boolean(syncedIds[sessionId] && !pending) || prog?.status === "done";
    const busy = prog?.status === "sending";
    const failed = prog?.status === "error";
    const stats = statsFor(prog, done);
    const frac = done ? 1 : stats.pct / 100;
    const title = busy
      ? `Syncing — ${Math.round(stats.pct)}%`
      : done
        ? "Synced to host"
        : pending
          ? pending.last_error
            ? `Sync to host (resume) — ${pending.last_error}`
            : "Sync to host"
          : "Nothing queued to sync";
    return { pending, prog, done, busy, failed, frac, title, stats };
  }

  return (
    <div className="rs-lobby rs-recordings">
      <header className="rs-rec-top">
        <div className="rs-rec-brand">
          <span className="rs-rec-dot" />
          OpenPodcast
        </div>
        <div className="rs-rec-actions">
          <button
            type="button"
            className="rs-rec-back"
            onClick={() => navigate("/")}
          >
            <IconArrowLeft size={16} stroke={1.75} />
            Studio
          </button>
          <button
            type="button"
            className="rs-rec-back"
            onClick={() => navigate("/settings")}
          >
            <IconSettings size={16} stroke={1.75} />
            Settings
          </button>
        </div>
      </header>

      <div className="rs-rec-shell">
        <p className="rs-lobby-eyebrow">Your library</p>
        <h1 className="rs-lobby-title">Past recordings</h1>
        <p className="rs-rec-sub">
          Takes saved on this machine. Use{" "}
          <strong>Sync to host</strong> to send or resume full-quality guest
          video while their studio is open.
        </p>

        {orphanPending.length > 0 && (
          <section className="rs-pending-transfers">
            <h2>Ready to sync</h2>
            <p>
              These takes are saved locally but not in the grid yet — sync them
              to the host.
            </p>
            <ul className="rs-pending-list">
              {orphanPending.map((item) => {
                const ui = syncUiForSession(item.local_session_id);
                const fileCount = Array.isArray(item.file_names)
                  ? item.file_names.length
                  : 0;
                return (
                  <li key={item.id} className="rs-pending-item is-stack">
                    <div className="rs-pending-item-head">
                      <div>
                        <strong>
                          {fileCount} file
                          {fileCount === 1 ? "" : "s"}
                        </strong>
                        <span>
                          {ui.busy || ui.failed
                            ? ui.prog?.fileName || "Uploading…"
                            : item.last_error || "Waiting to sync"}
                        </span>
                      </div>
                      <div className="rs-pending-actions">
                        <SyncRing
                          frac={ui.frac}
                          busy={!!ui.busy}
                          done={!!ui.done}
                          title={ui.title}
                          disabled={!ui.pending && !ui.busy}
                          onClick={() => void syncToHost(item)}
                        />
                      </div>
                    </div>
                    {(ui.busy || ui.failed || ui.done) && (
                      <TransferProgressBar
                        stats={ui.stats}
                        done={ui.done}
                        error={ui.failed ? ui.prog?.error : null}
                        label="Syncing to host"
                      />
                    )}
                  </li>
                );
              })}
            </ul>
          </section>
        )}

        {sessions.length === 0 && pendingTransfers.length === 0 ? (
          <div className="rs-rec-empty">
            <IconWaveSine size={36} stroke={1.5} />
            <p>No recordings yet.</p>
            <button
              type="button"
              className="rs-join-btn rs-rec-cta"
              onClick={() => navigate("/")}
            >
              <IconMicrophone2 size={16} stroke={1.75} />
              Open studio
            </button>
          </div>
        ) : sessions.length > 0 ? (
          <div className="rs-rec-grid">
            {sessions.map((s) => {
              const ui = syncUiForSession(s.id);
              return (
                <div
                  key={s.id}
                  className={`rs-rec-card${ui.busy ? " is-syncing" : ""}`}
                >
                  <button
                    type="button"
                    className="rs-rec-thumb"
                    onClick={() => navigate(`/playback/${s.id}`)}
                    aria-label={`Open ${s.name}`}
                  >
                    {thumbs[s.id] ? (
                      <img src={thumbs[s.id]} alt="" />
                    ) : (
                      <div className="rs-rec-thumb-fallback">
                        {s.has_video ? (
                          <IconVideo size={28} stroke={1.5} />
                        ) : (
                          <IconWaveSine size={28} stroke={1.5} />
                        )}
                      </div>
                    )}
                    <span className="rs-rec-play" aria-hidden>
                      <IconPlayerPlay size={16} stroke={2} />
                    </span>
                    {ui.busy && (
                      <span className="rs-rec-sync-badge">
                        {ui.stats.pct < 10
                          ? ui.stats.pct.toFixed(1)
                          : Math.round(ui.stats.pct)}
                        %
                      </span>
                    )}
                  </button>
                  <div className="rs-rec-meta">
                    {editingId === s.id ? (
                      <input
                        ref={nameInputRef}
                        className="rs-rec-name-input"
                        value={nameDraft}
                        aria-label="Recording name"
                        disabled={renaming}
                        onChange={(e) => setNameDraft(e.target.value)}
                        onBlur={() => void commitRename()}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            e.currentTarget.blur();
                          } else if (e.key === "Escape") {
                            e.preventDefault();
                            cancelRename();
                          }
                        }}
                        onClick={(e) => e.stopPropagation()}
                      />
                    ) : (
                      <button
                        type="button"
                        className="rs-rec-name-btn"
                        title="Click to rename"
                        onClick={() => beginRename(s.id, s.name)}
                      >
                        {s.name}
                      </button>
                    )}
                    <div className="rs-rec-meta-row">
                      <span>
                        {formatDuration(s.duration_secs)} · {s.resolution} ·{" "}
                        {new Date(s.created_at).toLocaleDateString()}
                        {ui.done
                          ? " · Synced to host"
                          : ui.pending
                            ? " · Needs sync"
                            : ""}
                      </span>
                      <div className="rs-rec-meta-actions">
                        <SyncRing
                          frac={ui.frac}
                          busy={!!ui.busy}
                          done={!!ui.done}
                          title={ui.title}
                          disabled={!ui.pending && !ui.busy && !ui.done}
                          onClick={() => {
                            if (ui.pending) void syncToHost(ui.pending);
                          }}
                        />
                        <button
                          type="button"
                          className="rs-rec-delete"
                          title="Delete recording"
                          aria-label={`Delete ${s.name}`}
                          onClick={() => setPendingDelete(s)}
                        >
                          <IconTrash size={15} stroke={1.75} />
                        </button>
                      </div>
                    </div>
                    {(ui.busy || ui.failed) && (
                      <TransferProgressBar
                        stats={ui.stats}
                        done={false}
                        error={ui.failed ? ui.prog?.error : null}
                        label={
                          ui.prog?.fileName
                            ? `Sending ${ui.prog.fileName}`
                            : "Syncing to host"
                        }
                      />
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        ) : null}
      </div>

      <DeleteRecordingDialog
        open={!!pendingDelete}
        recordingName={pendingDelete?.name ?? "this recording"}
        busy={deleting}
        onClose={() => {
          if (!deleting) setPendingDelete(null);
        }}
        onConfirm={confirmDelete}
      />
    </div>
  );
}

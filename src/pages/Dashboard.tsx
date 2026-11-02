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
  file_sizes?: Record<string, number>;
  offsets?: Record<string, number>;
  attempts?: number;
  updated_at?: string | null;
  last_error: string | null;
};

function pendingResumeStats(item: PendingTransfer): TransferStats {
  const sizes = item.file_sizes ?? {};
  const offsets = item.offsets ?? {};
  const names =
    item.file_names.length > 0
      ? item.file_names
      : Array.from(new Set([...Object.keys(sizes), ...Object.keys(offsets)]));
  let total = 0;
  let bytes = 0;
  for (const name of names) {
    const size = sizes[name] ?? 0;
    const off = offsets[name] ?? 0;
    total += size;
    bytes += size > 0 ? Math.min(off, size) : off;
  }
  total = Math.max(1, total || bytes || 1);
  bytes = Math.min(total, bytes);
  const pct = Math.min(100, (bytes / total) * 100);
  return { bytes, total, pct, bytesPerSec: 0, etaSec: null };
}

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
    const seed = pendingResumeStats(item);
    samplesRef.current[sid] = [
      { bytes: seed.bytes, total: seed.total, at: Date.now() },
    ];
    setSyncProgress((prev) => ({
      ...prev,
      [sid]: {
        sessionId: sid,
        bytes: seed.bytes,
        total: seed.total,
        samples: samplesRef.current[sid],
        status: "sending",
        fileName:
          seed.bytes > 0
            ? `Resuming from ${Math.round(seed.pct)}%…`
            : undefined,
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
        const total = Math.max(1, cur?.total ?? seed.total);
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
          bytes: prev[sid]?.bytes ?? seed.bytes,
          total: prev[sid]?.total ?? seed.total,
          samples: prev[sid]?.samples ?? [],
          status: "error",
          error: msg,
        },
      }));
      await refreshPending();
    } finally {
      activeSyncSession.current = null;
    }
  }

  function syncUiForSession(sessionId: string) {
    const pending = pendingBySession.get(sessionId);
    const prog = syncProgress[sessionId];
    const done = Boolean(syncedIds[sessionId] && !pending) || prog?.status === "done";
    const busy = prog?.status === "sending";
    const failed = prog?.status === "error" || Boolean(pending?.last_error && !busy && !done);
    const resume = pending ? pendingResumeStats(pending) : null;
    const stats = done
      ? statsFor(prog, true)
      : busy || prog
        ? statsFor(prog, false)
        : resume
          ? resume
          : statsFor(undefined, false);
    const frac = done ? 1 : stats.pct / 100;
    const title = busy
      ? `Syncing — ${Math.round(stats.pct)}%`
      : done
        ? "Synced to host"
        : pending
          ? pending.last_error
            ? `Resume sync — ${pending.last_error}`
            : resume && resume.bytes > 0
              ? `Resume sync (${Math.round(resume.pct)}% already sent)`
              : "Sync to host"
          : "Nothing queued to sync";
    return { pending, prog, done, busy, failed, frac, title, stats, resume };
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
          Takes saved on this machine. Interrupted guest uploads stay queued —
          open <strong>Resume sync</strong> anytime while the host studio is up.
        </p>

        {pendingTransfers.length > 0 && (
          <section className="rs-pending-transfers">
            <h2>
              {pendingTransfers.some((p) => (p.offsets && Object.keys(p.offsets).length) || p.last_error)
                ? "Uploads to resume"
                : "Ready to sync"}
            </h2>
            <p>
              Your full-quality files stay on this computer. If the host drops
              mid-send, tap Resume — we continue from the last saved byte.
              {orphanPending.length > 0
                ? " Some takes below aren’t in the library grid yet."
                : ""}
            </p>
            <ul className="rs-pending-list">
              {pendingTransfers.map((item) => {
                const ui = syncUiForSession(item.local_session_id);
                const fileCount = Array.isArray(item.file_names)
                  ? item.file_names.length
                  : 0;
                const sessionName =
                  sessions.find((s) => s.id === item.local_session_id)?.name ??
                  "Guest take";
                const err = ui.prog?.error || item.last_error;
                return (
                  <li
                    key={item.id}
                    className={`rs-pending-item is-stack${ui.failed ? " is-failed" : ""}${ui.busy ? " is-busy" : ""}`}
                  >
                    <div className="rs-pending-item-head">
                      <div>
                        <strong>{sessionName}</strong>
                        <span>
                          {fileCount} file{fileCount === 1 ? "" : "s"}
                          {ui.resume && ui.resume.bytes > 0
                            ? ` · ${Math.round(ui.resume.pct)}% already on host`
                            : ""}
                          {item.attempts ? ` · ${item.attempts} attempt${item.attempts === 1 ? "" : "s"}` : ""}
                        </span>
                      </div>
                      <div className="rs-pending-actions">
                        <button
                          type="button"
                          className="btn btn-primary rs-pending-resume"
                          disabled={!!ui.busy}
                          onClick={() => void syncToHost(item)}
                        >
                          <IconUpload size={15} stroke={1.75} />
                          {ui.busy
                            ? "Sending…"
                            : ui.resume && ui.resume.bytes > 0
                              ? "Resume sync"
                              : "Sync to host"}
                        </button>
                      </div>
                    </div>
                    <TransferProgressBar
                      stats={ui.stats}
                      done={ui.done}
                      error={ui.busy ? null : err}
                      label={
                        ui.busy
                          ? ui.prog?.fileName || "Reconnecting & sending…"
                          : ui.resume && ui.resume.bytes > 0
                            ? "Paused — ready to resume"
                            : "Waiting to sync"
                      }
                    />
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

import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  IconArrowLeft,
  IconMicrophone2,
  IconPlayerPlay,
  IconSettings,
  IconTrash,
  IconVideo,
  IconWaveSine,
} from "@tabler/icons-react";
import { api } from "../api";
import { DeleteRecordingDialog } from "../components/DeleteRecordingDialog";
import { useApp } from "../state";
import type { SessionRecord } from "../types";

function formatDuration(secs: number) {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export default function Dashboard() {
  const { sessions, refreshSessions } = useApp();
  const navigate = useNavigate();
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [editingId, setEditingId] = useState<string | null>(null);
  const [nameDraft, setNameDraft] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<SessionRecord | null>(null);
  const [deleting, setDeleting] = useState(false);
  const nameInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    void refreshSessions();
  }, [refreshSessions]);

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
    } catch (e) {
      alert(String(e));
    } finally {
      setDeleting(false);
    }
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
          Takes saved on this machine. Click a thumbnail to open, or the title to
          rename.
        </p>

        {sessions.length === 0 ? (
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
        ) : (
          <div className="rs-rec-grid">
            {sessions.map((s) => (
              <div key={s.id} className="rs-rec-card">
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
                    </span>
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
              </div>
            ))}
          </div>
        )}
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

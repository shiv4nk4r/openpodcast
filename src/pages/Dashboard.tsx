import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  IconArrowLeft,
  IconFolderOpen,
  IconMicrophone2,
  IconPlayerPlay,
  IconSettings,
  IconVideo,
  IconWaveSine,
  IconX,
} from "@tabler/icons-react";
import { api } from "../api";
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
  const [preview, setPreview] = useState<SessionRecord | null>(null);
  const [previewVideo, setPreviewVideo] = useState<string | null>(null);
  const [previewAudio, setPreviewAudio] = useState<string | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);

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

  async function openPreview(s: SessionRecord) {
    setPreview(s);
    setPreviewBusy(true);
    setPreviewVideo(null);
    setPreviewAudio(null);
    try {
      const media = await api.getSessionMedia(s.id);
      const withVideo = media.find((m) => m.video_path);
      const withAudio = media.find((m) => m.audio_path);
      setPreviewVideo(
        withVideo?.video_path ? convertFileSrc(withVideo.video_path) : null,
      );
      setPreviewAudio(
        withAudio?.audio_path ? convertFileSrc(withAudio.audio_path) : null,
      );
    } catch {
      /* ignore */
    } finally {
      setPreviewBusy(false);
    }
  }

  async function openFolder(sessionId: string) {
    try {
      await api.revealSessionFolder(sessionId);
    } catch (e) {
      alert(String(e));
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
          Takes saved on this machine. Click one to preview or open its folder.
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
              <button
                key={s.id}
                type="button"
                className="rs-rec-card"
                onClick={() => void openPreview(s)}
              >
                <div className="rs-rec-thumb">
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
                </div>
                <div className="rs-rec-meta">
                  <strong>{s.name}</strong>
                  <span>
                    {formatDuration(s.duration_secs)} · {s.resolution} ·{" "}
                    {new Date(s.created_at).toLocaleDateString()}
                  </span>
                </div>
              </button>
            ))}
          </div>
        )}
      </div>

      {preview && (
        <div
          className="modal-backdrop"
          onClick={() => setPreview(null)}
          role="presentation"
        >
          <div
            className="modal recording-preview-modal"
            onClick={(e) => e.stopPropagation()}
            role="dialog"
            aria-modal
            aria-label="Recording preview"
          >
            <div className="recording-preview-head">
              <div>
                <h3 style={{ margin: 0 }}>{preview.name}</h3>
                <p className="recording-preview-sub">
                  {formatDuration(preview.duration_secs)} · {preview.resolution} ·{" "}
                  {new Date(preview.created_at).toLocaleString()}
                </p>
              </div>
              <button
                type="button"
                className="rs-icon-btn"
                onClick={() => setPreview(null)}
                aria-label="Close"
              >
                <IconX size={18} stroke={1.75} />
              </button>
            </div>

            <div className="recording-preview-stage">
              {previewBusy ? (
                <p className="recording-preview-empty">Loading…</p>
              ) : previewVideo ? (
                <video
                  key={previewVideo}
                  src={previewVideo}
                  controls
                  playsInline
                  className="recording-preview-video"
                />
              ) : previewAudio ? (
                <div className="recording-preview-audio">
                  {thumbs[preview.id] ? (
                    <img src={thumbs[preview.id]} alt="" />
                  ) : (
                    <IconWaveSine size={48} stroke={1.5} />
                  )}
                  <audio key={previewAudio} src={previewAudio} controls />
                </div>
              ) : (
                <p className="recording-preview-empty">
                  No playable media found for this session.
                </p>
              )}
            </div>

            <div className="recording-preview-actions">
              <button
                type="button"
                className="btn btn-secondary"
                onClick={() => void openFolder(preview.id)}
              >
                <IconFolderOpen size={16} stroke={1.75} />
                Open folder
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => setPreview(null)}
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

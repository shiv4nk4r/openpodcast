import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import {
  IconArrowLeft,
  IconFolderOpen,
  IconPlayerPause,
  IconPlayerPlay,
  IconTrash,
} from "@tabler/icons-react";
import { api } from "../api";
import { DeleteRecordingDialog } from "../components/DeleteRecordingDialog";
import { useApp } from "../state";
import { useTheme } from "../theme";
import type { Participant, SessionRecord } from "../types";

interface MediaTrack {
  participant_id: string;
  name: string;
  role: string;
  videoUrl: string | null;
  audioUrl: string | null;
}

function formatDuration(secs: number) {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

async function decodePeaks(
  url: string,
  buckets: number,
): Promise<Float32Array | null> {
  try {
    const res = await fetch(url);
    const buf = await res.arrayBuffer();
    const ctx = new AudioContext();
    const audio = await ctx.decodeAudioData(buf.slice(0));
    await ctx.close();
    const channel = audio.getChannelData(0);
    const peaks = new Float32Array(buckets);
    const block = Math.max(1, Math.floor(channel.length / buckets));
    for (let i = 0; i < buckets; i++) {
      const start = i * block;
      const end = Math.min(channel.length, start + block);
      let peak = 0;
      for (let j = start; j < end; j++) {
        peak = Math.max(peak, Math.abs(channel[j]));
      }
      peaks[i] = peak;
    }
    return peaks;
  } catch {
    return null;
  }
}

export default function Playback() {
  const { id } = useParams();
  const navigate = useNavigate();
  const { refreshSessions } = useApp();
  const { theme } = useTheme();
  const [session, setSession] = useState<SessionRecord | null>(null);
  const [tracks, setTracks] = useState<MediaTrack[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [t, setT] = useState(0);
  const [mediaDuration, setMediaDuration] = useState(0);
  const [peaksByTrack, setPeaksByTrack] = useState<
    Record<string, Float32Array>
  >({});
  const [editingName, setEditingName] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const nameInputRef = useRef<HTMLInputElement | null>(null);

  const videoRefs = useRef<Map<string, HTMLVideoElement>>(new Map());
  const audioRefs = useRef<Map<string, HTMLAudioElement>>(new Map());
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const duration = Math.max(
    session?.duration_secs || 0,
    mediaDuration || 0,
    1,
  );

  const participants: Participant[] = useMemo(() => {
    if (session?.participants?.length) return session.participants;
    return tracks.map((tr) => ({
      id: tr.participant_id,
      name: tr.name,
      role: tr.role || "Guest",
      resolution: session?.resolution ?? "1080p",
    }));
  }, [session, tracks]);

  useEffect(() => {
    if (!id) return;
    (async () => {
      try {
        const s = await api.getSession(id);
        setSession(s);
        const media = await api.getSessionMedia(id);
        const nameById = new Map(
          (s?.participants ?? []).map((p) => [p.id, p] as const),
        );
        const mapped: MediaTrack[] = media.map((m) => {
          const meta = nameById.get(m.participant_id);
          return {
            participant_id: m.participant_id,
            name: meta?.name ?? m.participant_id,
            role: meta?.role ?? "Guest",
            videoUrl: m.video_path ? convertFileSrc(m.video_path) : null,
            audioUrl: m.audio_path ? convertFileSrc(m.audio_path) : null,
          };
        });
        // Include participants who were in the lobby but have no local media file
        for (const p of s?.participants ?? []) {
          if (!mapped.some((m) => m.participant_id === p.id)) {
            mapped.push({
              participant_id: p.id,
              name: p.name,
              role: p.role,
              videoUrl: null,
              audioUrl: null,
            });
          }
        }
        setTracks(mapped);
        if (!mapped.some((m) => m.videoUrl || m.audioUrl)) {
          setLoadError("No playable media found for this recording.");
        }
      } catch (e) {
        setLoadError(String(e));
      }
    })();
  }, [id]);

  // Build real waveforms from WAV / audio tracks
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const next: Record<string, Float32Array> = {};
      for (const tr of tracks) {
        const url = tr.audioUrl ?? tr.videoUrl;
        if (!url) continue;
        const peaks = await decodePeaks(url, 900);
        if (peaks && !cancelled) next[tr.participant_id] = peaks;
      }
      if (!cancelled) setPeaksByTrack(next);
    })();
    return () => {
      cancelled = true;
    };
  }, [tracks]);

  function setVideoRef(pid: string, el: HTMLVideoElement | null) {
    if (el) videoRefs.current.set(pid, el);
    else videoRefs.current.delete(pid);
  }

  function setAudioRef(pid: string, el: HTMLAudioElement | null) {
    if (el) audioRefs.current.set(pid, el);
    else audioRefs.current.delete(pid);
  }

  function seekAll(time: number) {
    videoRefs.current.forEach((v) => {
      if (Number.isFinite(v.duration)) {
        v.currentTime = Math.min(time, v.duration || time);
      }
    });
    audioRefs.current.forEach((a) => {
      if (Number.isFinite(a.duration)) {
        a.currentTime = Math.min(time, a.duration || time);
      }
    });
  }

  async function playAll() {
    const tasks: Promise<void>[] = [];
    videoRefs.current.forEach((v) => {
      tasks.push(v.play().then(() => undefined).catch(() => undefined));
    });
    // Prefer separate high-quality WAV when present (video is muted in that case)
    audioRefs.current.forEach((a) => {
      tasks.push(a.play().then(() => undefined).catch(() => undefined));
    });
    await Promise.all(tasks);
  }

  function pauseAll() {
    videoRefs.current.forEach((v) => v.pause());
    audioRefs.current.forEach((a) => a.pause());
  }

  useEffect(() => {
    if (playing) void playAll();
    else pauseAll();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing]);

  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const clock =
        [...videoRefs.current.values()].find((v) => !v.paused && !v.ended) ??
        [...audioRefs.current.values()].find((a) => !a.paused && !a.ended);
      if (clock) {
        setT(clock.currentTime);
        if (clock.ended || clock.currentTime >= duration - 0.05) {
          setPlaying(false);
          setT(Math.min(clock.currentTime, duration));
          pauseAll();
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, duration]);

  useEffect(() => {
    if (playing) return;
    seekAll(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, playing]);

  // Draw real waveforms
  useEffect(() => {
    const c = canvasRef.current;
    if (!c) return;
    const ctx = c.getContext("2d");
    if (!ctx) return;
    const styles = getComputedStyle(document.documentElement);
    const bg = styles.getPropertyValue("--surface-2").trim() || "#f7f8fb";
    const label = styles.getPropertyValue("--ink-soft").trim() || "#3d4658";
    const muted = styles.getPropertyValue("--ink-muted").trim() || "#5b6578";
    const waveA = styles.getPropertyValue("--green").trim() || "#16a34a";
    const waveB = styles.getPropertyValue("--violet").trim() || "#6366f1";
    const dpr = window.devicePixelRatio || 1;
    const cssW = c.clientWidth || 1200;
    const cssH = c.clientHeight || 96;
    c.width = Math.floor(cssW * dpr);
    c.height = Math.floor(cssH * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = bg;
    ctx.fillRect(0, 0, cssW, cssH);

    const ids = Object.keys(peaksByTrack);
    const lanes = Math.max(ids.length, 1);
    const laneH = cssH / lanes;

    ids.forEach((pid, idx) => {
      const peaks = peaksByTrack[pid];
      const yMid = laneH * idx + laneH / 2;
      ctx.fillStyle = idx % 2 === 0 ? waveA : waveB;
      const n = peaks.length;
      for (let i = 0; i < n; i++) {
        const x = (i / n) * cssW;
        const amp = Math.max(1, peaks[i] * (laneH * 0.42));
        ctx.fillRect(x, yMid - amp, Math.max(1, cssW / n - 0.5), amp * 2);
      }
      const tr = tracks.find((t) => t.participant_id === pid);
      if (tr) {
        ctx.fillStyle = label;
        ctx.font = "600 11px Manrope, ui-sans-serif, system-ui";
        ctx.fillText(tr.name, 8, laneH * idx + 14);
      }
    });

    if (ids.length === 0) {
      ctx.fillStyle = muted;
      ctx.font = "13px Manrope, ui-sans-serif, system-ui";
      ctx.fillText("Waveform loads when audio is available…", 16, cssH / 2);
    }
  }, [peaksByTrack, tracks, t, duration, theme]);

  function onMediaMeta(el: HTMLMediaElement) {
    if (Number.isFinite(el.duration) && el.duration > 0) {
      setMediaDuration((d) => Math.max(d, el.duration));
    }
  }

  async function openFolder() {
    if (!id) return;
    try {
      await api.revealSessionFolder(id);
    } catch (e) {
      alert(String(e));
    }
  }

  function beginRename() {
    if (!session || renaming) return;
    setNameDraft(session.name);
    setEditingName(true);
  }

  useEffect(() => {
    if (!editingName) return;
    const el = nameInputRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, [editingName]);

  async function commitRename() {
    if (!id || !session || renaming) return;
    const next = nameDraft.trim();
    setEditingName(false);
    if (!next || next === session.name) {
      setNameDraft(session.name);
      return;
    }
    setRenaming(true);
    try {
      const updated = await api.renameSession(id, next);
      setSession(updated);
      await refreshSessions();
    } catch (e) {
      alert(String(e));
      setNameDraft(session.name);
    } finally {
      setRenaming(false);
    }
  }

  function cancelRename() {
    setEditingName(false);
    setNameDraft(session?.name ?? "");
  }

  async function confirmDelete() {
    if (!id || deleting) return;
    setDeleting(true);
    try {
      pauseAll();
      setPlaying(false);
      await api.deleteSession(id);
      await refreshSessions();
      navigate("/recordings", { replace: true });
    } catch (e) {
      alert(String(e));
      setDeleting(false);
      setDeleteOpen(false);
    }
  }

  const playable = tracks.filter((tr) => tr.videoUrl || tr.audioUrl);

  return (
    <>
    <div className="editor playback">
      <div className="editor-top">
        {editingName ? (
          <input
            ref={nameInputRef}
            className="playback-title-input"
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
          />
        ) : (
          <button
            type="button"
            className="playback-title-btn"
            title="Click to rename"
            onClick={beginRename}
          >
            {session?.name ?? "Recording"}
          </button>
        )}
        <span className="playback-meta-chip">
          {formatDuration(duration)} · {session?.resolution ?? "—"} ·{" "}
          {session ? new Date(session.created_at).toLocaleString() : ""}
        </span>
        <div className="spacer" />
        <button type="button" className="bar-btn" onClick={() => void openFolder()}>
          <IconFolderOpen size={16} stroke={1.75} />
          Folder
        </button>
        <button
          type="button"
          className="bar-btn bar-btn-danger"
          onClick={() => setDeleteOpen(true)}
        >
          <IconTrash size={16} stroke={1.75} />
          Delete
        </button>
        <button
          type="button"
          className="bar-btn"
          onClick={() => navigate("/recordings")}
        >
          <IconArrowLeft size={16} stroke={1.75} />
          Back
        </button>
      </div>

      <div className="editor-left playback-left">
        {participants.map((p) => (
          <div key={p.id} className="avatar" title={`${p.name} · ${p.role}`}>
            {p.name.slice(0, 2).toUpperCase()}
          </div>
        ))}
      </div>

      <div
        className={`editor-canvas playback-canvas${playable.length > 1 ? " is-grid" : ""}`}
      >
        {loadError && (
          <div className="tile empty" style={{ maxWidth: "70%" }}>
            {loadError}
          </div>
        )}
        {!loadError && playable.length === 0 && (
          <div className="tile empty" style={{ maxWidth: "70%" }}>
            No playable tracks
          </div>
        )}
        {playable.map((tr) => (
          <div className="tile" key={tr.participant_id}>
            {tr.videoUrl ? (
              <video
                ref={(el) => setVideoRef(tr.participant_id, el)}
                src={tr.videoUrl}
                playsInline
                preload="auto"
                muted={!!tr.audioUrl}
                onLoadedMetadata={(e) => onMediaMeta(e.currentTarget)}
              />
            ) : (
              <div className="empty">{tr.name}</div>
            )}
            {tr.audioUrl && (
              <audio
                ref={(el) => setAudioRef(tr.participant_id, el)}
                src={tr.audioUrl}
                preload="auto"
                onLoadedMetadata={(e) => onMediaMeta(e.currentTarget)}
              />
            )}
            <div className="name-tag">
              {tr.name}
              {tr.role ? ` · ${tr.role}` : ""}
            </div>
          </div>
        ))}
      </div>

      <aside className="playback-side">
        <h3>Session</h3>
        <dl className="playback-dl">
          <div>
            <dt>Title</dt>
            <dd>{session?.name ?? "—"}</dd>
          </div>
          <div>
            <dt>Duration</dt>
            <dd>{formatDuration(duration)}</dd>
          </div>
          <div>
            <dt>Quality</dt>
            <dd>
              {session?.resolution ?? "—"} · {session?.frame_rate ?? 30}fps
            </dd>
          </div>
          <div>
            <dt>Mode</dt>
            <dd>
              {session?.mode === "audio_only" ? "Audio only" : "Audio + video"}
            </dd>
          </div>
          <div>
            <dt>Recorded</dt>
            <dd>
              {session
                ? new Date(session.created_at).toLocaleString()
                : "—"}
            </dd>
          </div>
        </dl>

        <h3>Who was here</h3>
        <p className="playback-hint">
          People in this studio session (host, guests, lobby).
        </p>
        <ul className="playback-people">
          {participants.length === 0 && (
            <li className="playback-people-empty">No participant metadata saved.</li>
          )}
          {participants.map((p) => (
            <li key={p.id}>
              <span className="playback-avatar">{p.name.slice(0, 1).toUpperCase()}</span>
              <div>
                <strong>{p.name}</strong>
                <small>
                  {p.role}
                  {p.resolution ? ` · ${p.resolution}` : ""}
                </small>
              </div>
            </li>
          ))}
        </ul>
      </aside>

      <div className="timeline playback-timeline">
        <div className="transport">
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => setPlaying((p) => !p)}
            disabled={!playable.length}
          >
            {playing ? (
              <IconPlayerPause size={16} stroke={1.75} />
            ) : (
              <IconPlayerPlay size={16} stroke={1.75} />
            )}
            {playing ? "Pause" : "Play"}
          </button>
          <span style={{ color: "var(--text-muted)", fontSize: "0.85rem" }}>
            {t.toFixed(1)}s / {duration.toFixed(1)}s
          </span>
        </div>
        <div
          className="wave-track"
          onClick={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            const ratio = (e.clientX - rect.left) / rect.width;
            const next = ratio * duration;
            setPlaying(false);
            setT(next);
            seekAll(next);
          }}
        >
          <canvas className="wave-canvas" ref={canvasRef} />
          <div className="playhead" style={{ left: `${(t / duration) * 100}%` }} />
        </div>
      </div>
    </div>
    <DeleteRecordingDialog
      open={deleteOpen}
      recordingName={session?.name ?? "this recording"}
      busy={deleting}
      onClose={() => {
        if (!deleting) setDeleteOpen(false);
      }}
      onConfirm={confirmDelete}
    />
    </>
  );
}

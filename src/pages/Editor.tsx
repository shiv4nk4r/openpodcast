import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  IconArrowBackUp,
  IconArrowForwardUp,
  IconBadgeCc,
  IconHome,
  IconLayoutGrid,
  IconMusic,
  IconPalette,
  IconPhoto,
  IconPlayerPause,
  IconPlayerPlay,
  IconPlus,
  IconRobot,
  IconShare2,
  IconSparkles,
  IconStack2,
  IconTrash,
  IconTypography,
  IconVideo,
} from "@tabler/icons-react";
import { api } from "../api";
import { useApp } from "../state";
import { useTheme } from "../theme";
import type { CaptionCue, SessionRecord } from "../types";

const TOOL_ICONS = {
  ai: IconRobot,
  brand: IconPalette,
  tracks: IconStack2,
  captions: IconBadgeCc,
  media: IconPhoto,
  stock: IconVideo,
  music: IconMusic,
  text: IconTypography,
} as const;

type Tool =
  | "ai"
  | "brand"
  | "tracks"
  | "captions"
  | "media"
  | "stock"
  | "music"
  | "text"
  | null;

interface MediaTrack {
  participant_id: string;
  name: string;
  videoUrl: string | null;
  audioUrl: string | null;
}

export default function Editor() {
  const { id } = useParams();
  const navigate = useNavigate();
  const { settings, refreshSessions } = useApp();
  const { theme } = useTheme();
  const [session, setSession] = useState<SessionRecord | null>(null);
  const [files, setFiles] = useState<string[]>([]);
  const [tracks, setTracks] = useState<MediaTrack[]>([]);
  const [tool, setTool] = useState<Tool>("captions");
  const [cues, setCues] = useState<CaptionCue[]>([]);
  const [layout, setLayout] = useState<"side" | "grid" | "speaker">("side");
  const [aspect, setAspect] = useState("16:9");
  const [playing, setPlaying] = useState(false);
  const [t, setT] = useState(0);
  const [mediaDuration, setMediaDuration] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [showNotes, setShowNotes] = useState("");
  const [trimStart, setTrimStart] = useState(0);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [captionError, setCaptionError] = useState<string | null>(null);
  const [nativeStt, setNativeStt] = useState<{
    available: boolean;
    engine: string;
  }>({ available: false, engine: "Native STT" });
  const [enabled, setEnabled] = useState<Record<string, boolean>>({});
  const [addingCue, setAddingCue] = useState(false);
  const [draftText, setDraftText] = useState("");
  const [draftSpeaker, setDraftSpeaker] = useState("");

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const videoRefs = useRef<Map<string, HTMLVideoElement>>(new Map());
  const audioRefs = useRef<Map<string, HTMLAudioElement>>(new Map());
  const syncingRef = useRef(false);

  useEffect(() => {
    if (!id) return;
    (async () => {
      try {
        const s = await api.getSession(id);
        setSession(s);
        setFiles(await api.listSessionFiles(id));
        const media = await api.getSessionMedia(id);
        const nameById = new Map(
          (s?.participants ?? []).map((p) => [p.id, p.name] as const),
        );
        const mapped: MediaTrack[] = media.map((m) => ({
          participant_id: m.participant_id,
          name: nameById.get(m.participant_id) ?? m.participant_id,
          videoUrl: m.video_path ? convertFileSrc(m.video_path) : null,
          audioUrl: m.audio_path ? convertFileSrc(m.audio_path) : null,
        }));
        setTracks(mapped);
        setEnabled(
          Object.fromEntries(mapped.map((m) => [m.participant_id, true])),
        );
        if (!mapped.some((m) => m.videoUrl || m.audioUrl)) {
          setLoadError(
            "No recorded video/audio found for this session yet. Record in the studio first.",
          );
        } else {
          setLoadError(null);
        }
        try {
          setCues(await api.loadCaptions(id));
        } catch {
          /* no captions yet */
        }
        try {
          setNativeStt(await api.nativeSttStatus());
        } catch {
          /* ignore */
        }
      } catch (e) {
        setLoadError(String(e));
      }
    })();
  }, [id]);

  const duration = Math.max(
    session?.duration_secs || 0,
    mediaDuration || 0,
    1,
  );

  const visibleTracks = useMemo(() => {
    const active = tracks.filter((tr) => enabled[tr.participant_id] !== false);
    if (layout === "speaker") return active.slice(0, 1);
    if (layout === "side") return active.slice(0, 2);
    return active.slice(0, 4);
  }, [tracks, enabled, layout]);

  function setVideoRef(pid: string, el: HTMLVideoElement | null) {
    if (el) videoRefs.current.set(pid, el);
    else videoRefs.current.delete(pid);
  }

  function setAudioRef(pid: string, el: HTMLAudioElement | null) {
    if (el) audioRefs.current.set(pid, el);
    else audioRefs.current.delete(pid);
  }

  function seekAll(time: number) {
    syncingRef.current = true;
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
    syncingRef.current = false;
  }

  async function playAll() {
    seekAll(t);
    const tasks: Promise<void>[] = [];
    videoRefs.current.forEach((v, pid) => {
      if (enabled[pid] === false) return;
      v.muted = false;
      tasks.push(v.play().then(() => undefined).catch(() => undefined));
    });
    audioRefs.current.forEach((a, pid) => {
      if (enabled[pid] === false) return;
      // Avoid double audio when video already has audio track
      const hasVideo = tracks.find((tr) => tr.participant_id === pid)?.videoUrl;
      if (hasVideo) {
        a.pause();
        return;
      }
      tasks.push(a.play().then(() => undefined).catch(() => undefined));
    });
    await Promise.all(tasks);
  }

  function pauseAll() {
    videoRefs.current.forEach((v) => v.pause());
    audioRefs.current.forEach((a) => a.pause());
  }

  useEffect(() => {
    if (playing) {
      void playAll();
    } else {
      pauseAll();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing]);

  // Keep React time state in sync with the first available media clock
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const firstVideo = [...videoRefs.current.values()].find(
        (v) => !v.paused && !v.ended,
      );
      const firstAudio = [...audioRefs.current.values()].find(
        (a) => !a.paused && !a.ended,
      );
      const clock = firstVideo ?? firstAudio;
      if (clock) {
        setT(clock.currentTime);
        if (clock.ended || clock.currentTime >= duration - 0.05) {
          setPlaying(false);
          setT(Math.min(clock.currentTime, duration));
          pauseAll();
          return;
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, duration]);

  useEffect(() => {
    if (playing) return;
    seekAll(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, playing]);

  useEffect(() => {
    const c = canvasRef.current;
    if (!c) return;
    const ctx = c.getContext("2d");
    if (!ctx) return;
    const styles = getComputedStyle(document.documentElement);
    const bg = styles.getPropertyValue("--surface-2").trim() || "#f7f8fb";
    const waveA = styles.getPropertyValue("--green").trim() || "#16a34a";
    const waveB = styles.getPropertyValue("--violet").trim() || "#6366f1";
    const dim =
      theme === "light" ? "rgba(16,20,28,0.12)" : "rgba(0,0,0,0.45)";
    const w = c.width;
    const h = c.height;
    ctx.fillStyle = bg;
    ctx.fillRect(0, 0, w, h);
    const speakers = Math.max(tracks.length, 1);
    for (let s = 0; s < speakers; s++) {
      ctx.strokeStyle = s % 2 === 0 ? waveA : waveB;
      ctx.beginPath();
      const yBase = h / 2 + (s - (speakers - 1) / 2) * 12;
      for (let x = 0; x < w; x++) {
        const amp =
          Math.sin(x * 0.05 + s) * 10 + Math.sin(x * 0.13 + s * 2) * 6;
        const y = yBase + amp;
        if (x === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.stroke();
    }
    if (trimStart > 0) {
      ctx.fillStyle = dim;
      ctx.fillRect(0, 0, (trimStart / duration) * w, h);
    }
  }, [tracks, trimStart, duration, cues, theme]);

  const tools = useMemo(
    () =>
      [
        ["ai", "AI"],
        ["brand", "Brand"],
        ["tracks", "Tracks"],
        ["captions", "Captions"],
        ["media", "Media"],
        ["stock", "Stock"],
        ["music", "Music"],
        ["text", "Text"],
      ] as const,
    [],
  );

  if (!id) return null;

  async function persistCues(next: CaptionCue[]) {
    const saved = await api.saveCaptions(id!, next);
    setCues(saved);
    return saved;
  }

  async function onGenerateCaptions(engine: "native" | "ai") {
    setBusy(
      engine === "native"
        ? `Generating captions with ${nativeStt.engine}…`
        : "Generating captions with AI…",
    );
    setCaptionError(null);
    try {
      const result = await api.generateCaptions(id!, engine);
      setCues(result);
      setTool("captions");
    } catch (e) {
      setCaptionError(String(e).replace(/^Error:\s*/i, ""));
      setTool("captions");
    } finally {
      setBusy(null);
    }
  }

  async function onAddManualCue() {
    const text = draftText.trim();
    if (!text) return;
    const speaker =
      draftSpeaker.trim() ||
      tracks[0]?.name ||
      settings.display_name ||
      "Host";
    const startMs = Math.floor(t * 1000);
    const endMs = startMs + 3000;
    const next: CaptionCue[] = [
      ...cues,
      {
        index: cues.length + 1,
        start_ms: startMs,
        end_ms: endMs,
        speaker,
        text,
      },
    ];
    setBusy("Saving subtitle…");
    setCaptionError(null);
    try {
      await persistCues(next);
      setDraftText("");
      setAddingCue(false);
    } catch (e) {
      setCaptionError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function updateCue(index: number, patch: Partial<CaptionCue>) {
    const next = cues.map((c) => (c.index === index ? { ...c, ...patch } : c));
    setCues(next);
  }

  async function commitCues() {
    setBusy("Saving subtitles…");
    setCaptionError(null);
    try {
      await persistCues(cues);
    } catch (e) {
      setCaptionError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function removeCue(index: number) {
    const next = cues.filter((c) => c.index !== index);
    setBusy("Saving…");
    try {
      await persistCues(next);
    } catch (e) {
      setCaptionError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function onShowNotes() {
    setBusy("Writing show notes…");
    try {
      const transcript = cues.map((c) => `[${c.speaker}] ${c.text}`).join("\n");
      const notes = await api.generateShowNotes(
        transcript || `Podcast: ${session?.name ?? "Episode"}`,
      );
      setShowNotes(notes);
      setTool("ai");
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function onExport() {
    setBusy("Exporting…");
    try {
      const path = await api.exportSession(
        {
          sessionId: id!,
          layout,
          includeCaptions: cues.length > 0,
          trimStartMs: Math.floor(trimStart * 1000),
          trimEndMs: null,
        },
        cues,
      );
      await refreshSessions();
      alert(`Exported to:\n${path}`);
    } catch (e) {
      alert(String(e));
    } finally {
      setBusy(null);
    }
  }

  function onMediaMeta(el: HTMLMediaElement) {
    if (Number.isFinite(el.duration) && el.duration > 0) {
      setMediaDuration((d) => Math.max(d, el.duration));
    }
  }

  const activeCue = cues.find(
    (c) => t * 1000 >= c.start_ms && t * 1000 <= c.end_ms,
  );

  return (
    <div className="editor" style={{ position: "relative" }}>
      <div className="editor-top">
        <button onClick={() => navigate("/")} title="Home" aria-label="Home">
          <IconHome size={18} stroke={1.75} />
        </button>
        <strong>{session?.name ?? "Podcast"}</strong>
        <button
          onClick={() => {
            const next = Math.max(0, t - 1);
            setT(next);
            seekAll(next);
          }}
          title="Back 1s"
          aria-label="Back 1s"
        >
          <IconArrowBackUp size={18} stroke={1.75} />
        </button>
        <button
          onClick={() => {
            const next = Math.min(duration, t + 1);
            setT(next);
            seekAll(next);
          }}
          title="Forward 1s"
          aria-label="Forward 1s"
        >
          <IconArrowForwardUp size={18} stroke={1.75} />
        </button>
        <select value={aspect} onChange={(e) => setAspect(e.target.value)}>
          <option>16:9</option>
          <option>9:16</option>
          <option>1:1</option>
        </select>
        <button
          onClick={() =>
            setLayout((l) => (l === "side" ? "grid" : l === "grid" ? "speaker" : "side"))
          }
          title="Layout"
        >
          <IconLayoutGrid size={16} stroke={1.75} />
          Layout
        </button>
        <div className="spacer" />
        {tracks.slice(0, 4).map((p) => (
          <div key={p.participant_id} className="avatar" title={p.name}>
            {p.name.slice(0, 1)}
          </div>
        ))}
        <button className="bar-btn" onClick={() => void onShowNotes()} title="Magic notes">
          <IconSparkles size={18} stroke={1.75} />
        </button>
        <button className="share-btn" onClick={() => void onExport()}>
          <IconShare2 size={16} stroke={1.75} />
          Share
        </button>
      </div>

      <div className="editor-left">
        {tracks.map((p) => (
          <div key={p.participant_id} className="avatar" title={p.name}>
            {p.name.slice(0, 2).toUpperCase()}
          </div>
        ))}
      </div>

      <div
        className={`editor-canvas ${layout === "grid" ? "is-grid" : ""}`}
        style={{
          aspectRatio: aspect === "9:16" ? "9/16" : aspect === "1:1" ? "1" : "16/9",
        }}
      >
        {visibleTracks.length === 0 && (
          <div className="tile empty" style={{ maxWidth: "70%" }}>
            {loadError ?? "No playable tracks"}
          </div>
        )}
        {visibleTracks.map((tr) => (
          <div className="tile" key={tr.participant_id}>
            {tr.videoUrl ? (
              <video
                ref={(el) => setVideoRef(tr.participant_id, el)}
                src={tr.videoUrl}
                playsInline
                preload="auto"
                onLoadedMetadata={(e) => onMediaMeta(e.currentTarget)}
                onError={() =>
                  setLoadError(
                    "Could not load video. Re-record or check session files in Media.",
                  )
                }
                style={{ transform: "none" }}
              />
            ) : (
              <div className="empty">{tr.name}</div>
            )}
            {tr.audioUrl && !tr.videoUrl && (
              <audio
                ref={(el) => setAudioRef(tr.participant_id, el)}
                src={tr.audioUrl}
                preload="auto"
                onLoadedMetadata={(e) => onMediaMeta(e.currentTarget)}
              />
            )}
            {/* Hidden audio companion when we only want WAV under a silent video — skipped if video has sound */}
            <div className="name-tag">{tr.name}</div>
            {activeCue && (
              <div
                style={{
                  position: "absolute",
                  bottom: 42,
                  left: 16,
                  right: 16,
                  textAlign: "center",
                  background: "rgba(0,0,0,0.65)",
                  padding: "8px 12px",
                  borderRadius: 8,
                }}
              >
                {activeCue.text}
              </div>
            )}
          </div>
        ))}
      </div>

      <div className="editor-right">
        {tools.map(([key, label]) => {
          const ToolIcon = TOOL_ICONS[key];
          return (
            <button
              key={key}
              className={`tool-icon ${tool === key ? "active" : ""}`}
              onClick={() => setTool(tool === key ? null : key)}
              title={label}
            >
              <ToolIcon size={20} stroke={1.75} />
            </button>
          );
        })}
      </div>

      {tool && (
        <div className="tool-panel">
          {tool === "captions" && (
            <>
              <h3 style={{ marginTop: 0 }}>Captions</h3>
              <p className="caption-hint">
                Use on-device speech recognition
                {nativeStt.available ? ` (${nativeStt.engine})` : ""}, your AI
                provider, or type cues at the playhead.
              </p>
              <div className="caption-actions">
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={!!busy || !nativeStt.available}
                  title={
                    nativeStt.available
                      ? `Transcribe with ${nativeStt.engine}`
                      : "On-device captions require macOS or Windows"
                  }
                  onClick={() => void onGenerateCaptions("native")}
                >
                  <IconBadgeCc size={16} stroke={1.75} />
                  Generate on device
                </button>
                <button
                  type="button"
                  className="btn btn-secondary"
                  disabled={!!busy}
                  onClick={() => void onGenerateCaptions("ai")}
                >
                  <IconSparkles size={16} stroke={1.75} />
                  Generate with AI
                </button>
                <button
                  type="button"
                  className="btn btn-secondary"
                  disabled={!!busy}
                  onClick={() => {
                    setAddingCue((v) => !v);
                    setDraftSpeaker(
                      tracks[0]?.name || settings.display_name || "Host",
                    );
                    setCaptionError(null);
                  }}
                >
                  <IconPlus size={16} stroke={1.75} />
                  Add subtitle
                </button>
              </div>
              {!settings.ai_has_key && (
                <p className="caption-hint">
                  AI captions need a key —{" "}
                  <Link to="/settings">open Settings</Link> for an
                  OpenAI-compatible Whisper endpoint. On-device works without
                  it.
                </p>
              )}
              {captionError && (
                <div className="caption-error">
                  <p style={{ margin: 0 }}>{captionError}</p>
                  {/permission|Speech Recognition|denied|authorized/i.test(
                    captionError,
                  ) && (
                    <button
                      type="button"
                      className="btn btn-secondary"
                      style={{ marginTop: 10 }}
                      onClick={() => void api.openSpeechSettings()}
                    >
                      Open Speech settings
                    </button>
                  )}
                </div>
              )}
              {addingCue && (
                <div className="caption-draft">
                  <label>
                    Speaker
                    <input
                      value={draftSpeaker}
                      onChange={(e) => setDraftSpeaker(e.target.value)}
                      placeholder="Speaker"
                    />
                  </label>
                  <label>
                    Text at {(t).toFixed(1)}s
                    <textarea
                      value={draftText}
                      onChange={(e) => setDraftText(e.target.value)}
                      placeholder="Type the subtitle…"
                      rows={3}
                      autoFocus
                    />
                  </label>
                  <div className="caption-actions">
                    <button
                      type="button"
                      className="btn btn-primary"
                      disabled={!draftText.trim() || !!busy}
                      onClick={() => void onAddManualCue()}
                    >
                      Save cue
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost"
                      onClick={() => {
                        setAddingCue(false);
                        setDraftText("");
                      }}
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              )}
              <div className="cue-list" style={{ marginTop: 12 }}>
                {cues.length === 0 && !addingCue && (
                  <p style={{ color: "var(--text-muted)" }}>
                    No captions yet. Generate with AI or add one manually.
                  </p>
                )}
                {cues.map((c) => (
                  <div className="cue-item cue-item-edit" key={c.index}>
                    <div className="cue-item-head">
                      <input
                        className="cue-time"
                        type="number"
                        step="0.1"
                        min={0}
                        value={(c.start_ms / 1000).toFixed(1)}
                        onChange={(e) =>
                          void updateCue(c.index, {
                            start_ms: Math.max(
                              0,
                              Math.round(parseFloat(e.target.value || "0") * 1000),
                            ),
                          })
                        }
                        onBlur={() => void commitCues()}
                        aria-label="Start seconds"
                      />
                      <span className="cue-sep">–</span>
                      <input
                        className="cue-time"
                        type="number"
                        step="0.1"
                        min={0}
                        value={(c.end_ms / 1000).toFixed(1)}
                        onChange={(e) =>
                          void updateCue(c.index, {
                            end_ms: Math.max(
                              c.start_ms + 200,
                              Math.round(parseFloat(e.target.value || "0") * 1000),
                            ),
                          })
                        }
                        onBlur={() => void commitCues()}
                        aria-label="End seconds"
                      />
                      <input
                        className="cue-speaker"
                        value={c.speaker}
                        onChange={(e) =>
                          void updateCue(c.index, { speaker: e.target.value })
                        }
                        onBlur={() => void commitCues()}
                        aria-label="Speaker"
                      />
                      <button
                        type="button"
                        className="cue-delete"
                        title="Delete"
                        onClick={() => void removeCue(c.index)}
                      >
                        <IconTrash size={14} stroke={1.75} />
                      </button>
                    </div>
                    <textarea
                      className="cue-text"
                      value={c.text}
                      rows={2}
                      onChange={(e) =>
                        void updateCue(c.index, { text: e.target.value })
                      }
                      onBlur={() => void commitCues()}
                    />
                  </div>
                ))}
              </div>
            </>
          )}
          {tool === "tracks" && (
            <>
              <h3 style={{ marginTop: 0 }}>Tracks</h3>
              {tracks.map((p) => (
                <label key={p.participant_id} className="toggle">
                  <span>
                    {p.name}
                    <small style={{ display: "block", color: "var(--text-muted)" }}>
                      {p.videoUrl ? "video" : ""}
                      {p.videoUrl && p.audioUrl ? " + " : ""}
                      {p.audioUrl ? "audio" : ""}
                      {!p.videoUrl && !p.audioUrl ? "missing" : ""}
                    </small>
                  </span>
                  <input
                    type="checkbox"
                    checked={enabled[p.participant_id] !== false}
                    onChange={(e) =>
                      setEnabled((prev) => ({
                        ...prev,
                        [p.participant_id]: e.target.checked,
                      }))
                    }
                  />
                </label>
              ))}
            </>
          )}
          {tool === "ai" && (
            <>
              <h3 style={{ marginTop: 0 }}>AI tools</h3>
              {!settings.ai_has_key && (
                <p style={{ color: "var(--text-muted)" }}>
                  Connect an OpenAI-compatible provider in{" "}
                  <Link to="/settings">Settings</Link>.
                </p>
              )}
              <button className="btn btn-secondary" onClick={() => void onShowNotes()}>
                Generate show notes
              </button>
              {showNotes && (
                <pre
                  style={{
                    whiteSpace: "pre-wrap",
                    fontSize: "0.85rem",
                    marginTop: 12,
                    background: "var(--bg)",
                    padding: 12,
                    borderRadius: 8,
                  }}
                >
                  {showNotes}
                </pre>
              )}
            </>
          )}
          {tool === "media" && (
            <>
              <h3 style={{ marginTop: 0 }}>Your media</h3>
              <ul style={{ paddingLeft: 18, fontSize: "0.85rem" }}>
                {files.map((f) => (
                  <li key={f}>{f}</li>
                ))}
              </ul>
              {loadError && (
                <p style={{ color: "var(--record)", fontSize: "0.85rem" }}>{loadError}</p>
              )}
              <button
                className="btn btn-secondary"
                style={{ marginTop: 8 }}
                onClick={async () => {
                  const path = await api.revealSessionFolder(id!);
                  alert(path);
                }}
              >
                Reveal folder
              </button>
            </>
          )}
          {(tool === "stock" || tool === "music" || tool === "brand" || tool === "text") && (
            <>
              <h3 style={{ marginTop: 0 }}>{tool}</h3>
              <p style={{ color: "var(--text-muted)" }}>
                Chrome matches Riverside. This panel ships as a stub in v1.
              </p>
            </>
          )}
        </div>
      )}

      <div className="timeline">
        <div className="transport">
          <button
            className="btn btn-secondary"
            onClick={() => setPlaying((p) => !p)}
            disabled={!tracks.some((tr) => tr.videoUrl || tr.audioUrl)}
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
          <label style={{ marginLeft: "auto", fontSize: "0.85rem", color: "var(--text-muted)" }}>
            Trim start{" "}
            <input
              type="range"
              min={0}
              max={duration}
              step={0.1}
              value={trimStart}
              onChange={(e) => setTrimStart(Number(e.target.value))}
            />
          </label>
          {busy && <span style={{ color: "var(--accent)" }}>{busy}</span>}
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
          <canvas className="wave-canvas" ref={canvasRef} width={1200} height={72} />
          <div className="playhead" style={{ left: `${(t / duration) * 100}%` }} />
        </div>
      </div>
    </div>
  );
}

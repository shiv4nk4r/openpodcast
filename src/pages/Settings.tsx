import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { open } from "@tauri-apps/plugin-shell";
import {
  IconArrowLeft,
  IconCamera,
  IconFolder,
  IconMicrophone,
  IconRefresh,
  IconVideo,
  IconVolume,
} from "@tabler/icons-react";
import { api } from "../api";
import { Select } from "../components/Select";
import { pickPreferred, useDeviceList } from "../hooks/useDeviceList";
import { useApp } from "../state";
import { useTheme, type ThemeId } from "../theme";
import { resetOnboarding } from "./Onboarding";

const THEME_SWATCHES: Record<ThemeId, string> = {
  "night-booth":
    "linear-gradient(135deg, #070809 30%, #1a1e24 55%, #c4f542 100%)",
  "signal-dark":
    "linear-gradient(135deg, #05070c 30%, #141c2a 55%, #5ce1ff 100%)",
  graphite: "linear-gradient(135deg, #0e0e0e 30%, #1f1f1f 55%, #e8c07a 100%)",
};

export default function Settings() {
  const navigate = useNavigate();
  const { settings, refreshSettings } = useApp();
  const { theme, setTheme, themes } = useTheme();
  const devices = useDeviceList();
  const previewRef = useRef<HTMLVideoElement | null>(null);
  const previewStream = useRef<MediaStream | null>(null);

  const [displayName, setDisplayName] = useState(settings.display_name);
  const [folder, setFolder] = useState(settings.recording_folder ?? "");
  const [echo, setEcho] = useState(settings.echo_cancellation);
  const [cameraId, setCameraId] = useState(settings.preferred_camera_id ?? "");
  const [micId, setMicId] = useState(settings.preferred_mic_id ?? "");
  const [speakerId, setSpeakerId] = useState(settings.preferred_speaker_id ?? "");
  const [favoriteCams, setFavoriteCams] = useState<string[]>(
    settings.preferred_camera_id ? [settings.preferred_camera_id] : [],
  );
  const [favoriteMics, setFavoriteMics] = useState<string[]>(
    settings.preferred_mic_id ? [settings.preferred_mic_id] : [],
  );
  const [paths, setPaths] = useState<{
    data: string;
    sessions: string;
    models: string;
  } | null>(null);
  const [oauthCode, setOauthCode] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [micLevel, setMicLevel] = useState(0);

  // AI provider / Whisper — paused for now
  // const [aiUrl, setAiUrl] = useState(settings.ai_base_url ?? "");
  // const [aiKey, setAiKey] = useState("");
  // const [whisperReady, setWhisperReady] = useState(false);

  useEffect(() => {
    setDisplayName(settings.display_name);
    setFolder(settings.recording_folder ?? "");
    setEcho(settings.echo_cancellation);
    setCameraId(settings.preferred_camera_id ?? "");
    setMicId(settings.preferred_mic_id ?? "");
    setSpeakerId(settings.preferred_speaker_id ?? "");
    if (settings.preferred_camera_id) setFavoriteCams([settings.preferred_camera_id]);
    if (settings.preferred_mic_id) setFavoriteMics([settings.preferred_mic_id]);
  }, [settings]);

  useEffect(() => {
    if (!cameraId && devices.cameras.length) {
      const id = pickPreferred(devices.cameras, settings.preferred_camera_id);
      setCameraId(id);
      setFavoriteCams((prev) => (prev.length ? prev : id ? [id] : []));
    }
    if (!micId && devices.mics.length) {
      const id = pickPreferred(devices.mics, settings.preferred_mic_id);
      setMicId(id);
      setFavoriteMics((prev) => (prev.length ? prev : id ? [id] : []));
    }
    if (!speakerId && devices.speakers.length) {
      setSpeakerId(pickPreferred(devices.speakers, settings.preferred_speaker_id));
    }
  }, [
    devices.cameras,
    devices.mics,
    devices.speakers,
    cameraId,
    micId,
    speakerId,
    settings.preferred_camera_id,
    settings.preferred_mic_id,
    settings.preferred_speaker_id,
  ]);

  useEffect(() => {
    api.getAppPaths().then(setPaths).catch(() => undefined);
    // api.whisperModelStatus().then((s) => setWhisperReady(s.ready)).catch(() => undefined);
  }, []);

  useEffect(() => {
    let cancelled = false;
    let raf = 0;
    let audioCtx: AudioContext | null = null;

    async function startPreview() {
      previewStream.current?.getTracks().forEach((t) => t.stop());
      previewStream.current = null;
      if (!cameraId && !micId) return;
      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          video: cameraId ? { deviceId: { exact: cameraId } } : false,
          audio: micId
            ? { deviceId: { exact: micId }, echoCancellation: echo }
            : false,
        });
        if (cancelled) {
          stream.getTracks().forEach((t) => t.stop());
          return;
        }
        previewStream.current = stream;
        if (previewRef.current && cameraId) {
          previewRef.current.srcObject = stream;
          await previewRef.current.play().catch(() => undefined);
        }
        if (stream.getAudioTracks()[0]) {
          audioCtx = new AudioContext();
          void audioCtx.resume();
          const source = audioCtx.createMediaStreamSource(stream);
          const analyser = audioCtx.createAnalyser();
          analyser.fftSize = 2048;
          analyser.smoothingTimeConstant = 0.8;
          source.connect(analyser);
          const data = new Float32Array(analyser.fftSize);
          let smoothed = 0;
          const tick = () => {
            if (cancelled) return;
            if (audioCtx?.state === "suspended") void audioCtx.resume();
            analyser.getFloatTimeDomainData(data);
            let sumSquares = 0;
            let peak = 0;
            for (let i = 0; i < data.length; i++) {
              const s = data[i];
              sumSquares += s * s;
              peak = Math.max(peak, Math.abs(s));
            }
            const rms = Math.sqrt(sumSquares / data.length);
            const raw = Math.min(1, rms * 3.2 + peak * 0.55);
            const boosted = Math.min(1, Math.pow(raw, 0.65) * 1.35);
            smoothed =
              boosted > smoothed
                ? smoothed * 0.35 + boosted * 0.65
                : smoothed * 0.85 + boosted * 0.15;
            setMicLevel(smoothed);
            raf = requestAnimationFrame(tick);
          };
          tick();
        }
      } catch (e) {
        setMsg(`Device preview: ${String(e)}`);
      }
    }

    void startPreview();
    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
      void audioCtx?.close();
      previewStream.current?.getTracks().forEach((t) => t.stop());
      previewStream.current = null;
    };
  }, [cameraId, micId, echo]);

  async function save() {
    setSaving(true);
    setMsg(null);
    try {
      const activeCam =
        (favoriteCams.includes(cameraId) ? cameraId : favoriteCams[0]) ||
        cameraId ||
        null;
      const activeMic =
        (favoriteMics.includes(micId) ? micId : favoriteMics[0]) || micId || null;
      await api.saveSettings({
        ...settings,
        display_name: displayName,
        recording_folder: folder || null,
        echo_cancellation: echo,
        preferred_camera_id: activeCam,
        preferred_mic_id: activeMic,
        preferred_speaker_id: speakerId || null,
      });
      // if (aiUrl || aiKey) await api.setAiProvider(aiUrl, aiKey);
      await refreshSettings();
      setMsg("Saved");
    } catch (e) {
      setMsg(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function connectDrive() {
    try {
      const url = await api.beginDriveOauth();
      await open(url);
      setMsg(
        "Browser opened. After Google consent, paste the ?code= value from the redirect URL below.",
      );
    } catch (e) {
      setMsg(String(e));
    }
  }

  async function finishDrive() {
    try {
      await api.completeDriveOauth(oauthCode.trim());
      await refreshSettings();
      setMsg("Google Drive connected");
    } catch (e) {
      setMsg(String(e));
    }
  }

  const cameraOptions = devices.cameras.map((d) => ({
    value: d.deviceId,
    label: d.label,
    description: favoriteCams.includes(d.deviceId) ? "In favorites" : undefined,
  }));
  const micOptions = devices.mics.map((d) => ({
    value: d.deviceId,
    label: d.label,
    description: favoriteMics.includes(d.deviceId) ? "In favorites" : undefined,
  }));
  const speakerOptions = devices.speakers.map((d) => ({
    value: d.deviceId,
    label: d.label,
  }));

  return (
    <div className="rs-lobby rs-recordings rs-settings">
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
            onClick={() => navigate("/recordings")}
          >
            <IconVideo size={16} stroke={1.75} />
            Recordings
          </button>
        </div>
      </header>

      <div className="rs-rec-shell rs-settings-shell">
        <p className="rs-lobby-eyebrow">Preferences</p>
        <h1 className="rs-lobby-title">Settings</h1>
        <p className="rs-rec-sub">
          Theme the booth, set your name, and choose the devices you record with.
        </p>

        <section className="rs-settings-card">
          <div className="rs-settings-card-head">
            <h2>Appearance</h2>
            <p>In-app look — no native system chrome for pickers.</p>
          </div>
          <div className="theme-grid rs-settings-themes">
            {themes.map((t) => (
              <button
                key={t.id}
                type="button"
                className={`theme-card ${theme === t.id ? "is-active" : ""}`}
                onClick={() => setTheme(t.id)}
              >
                <div
                  className="theme-swatch"
                  style={{ background: THEME_SWATCHES[t.id] }}
                />
                <strong>{t.label}</strong>
                <span>{t.description}</span>
              </button>
            ))}
          </div>
        </section>

        <section className="rs-settings-card">
          <div className="rs-settings-card-head">
            <h2>Profile</h2>
            <p>How you show up in the lobby and where takes are stored.</p>
          </div>
          <div className="rs-settings-fields">
            <div className="field">
              <label>Display name</label>
              <input
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder="Host"
              />
            </div>
            <div className="field">
              <label>Recording folder override</label>
              <div className="rs-settings-input-row">
                <IconFolder size={16} stroke={1.75} />
                <input
                  value={folder}
                  onChange={(e) => setFolder(e.target.value)}
                  placeholder="Leave empty to use app data"
                />
              </div>
            </div>
            <button
              type="button"
              className={`rs-settings-toggle${echo ? " is-on" : ""}`}
              onClick={() => setEcho((v) => !v)}
              aria-pressed={echo}
            >
              <span>
                <strong>Echo cancellation</strong>
                <small>Default for new studio sessions</small>
              </span>
              <span className="rs-settings-switch" aria-hidden />
            </button>
          </div>
        </section>

        <section className="rs-settings-card">
          <div className="rs-settings-card-head rs-settings-card-head-row">
            <div>
              <h2>Camera & microphone</h2>
              <p>
                {devices.loading
                  ? "Scanning devices…"
                  : `${devices.cameras.length} cameras · ${devices.mics.length} mics · ${devices.speakers.length} speakers`}
              </p>
            </div>
            <button
              type="button"
              className="rs-rec-back"
              onClick={() => void devices.refresh()}
            >
              <IconRefresh size={15} stroke={1.75} />
              Refresh
            </button>
          </div>

          <div className="rs-settings-devices">
            <div className="rs-settings-preview">
              {cameraId ? (
                <video
                  ref={previewRef}
                  muted
                  playsInline
                  autoPlay
                  className="rs-settings-preview-video"
                />
              ) : (
                <div className="rs-settings-preview-empty">
                  <IconCamera size={28} stroke={1.5} />
                  Select a camera
                </div>
              )}
            </div>

            <div className="rs-settings-device-fields">
              <Select
                multiple
                icon={<IconCamera size={18} stroke={1.75} />}
                label={`Favorite cameras${devices.cameras.length > 1 ? ` (${devices.cameras.length})` : ""}`}
                hint="Multi-select the cameras you use often."
                value={favoriteCams}
                onChange={(vals) => {
                  setFavoriteCams(vals);
                  if (vals.length && !vals.includes(cameraId)) setCameraId(vals[0]);
                  if (!vals.length) setCameraId("");
                }}
                options={cameraOptions}
                emptyLabel="No camera found"
                placeholder="Select cameras…"
              />
              <Select
                label="Active camera"
                icon={<IconCamera size={18} stroke={1.75} />}
                value={cameraId}
                onChange={setCameraId}
                options={(favoriteCams.length
                  ? devices.cameras.filter((d) => favoriteCams.includes(d.deviceId))
                  : devices.cameras
                ).map((d) => ({ value: d.deviceId, label: d.label }))}
                emptyLabel="No camera selected"
              />
              <Select
                multiple
                icon={<IconMicrophone size={18} stroke={1.75} />}
                label={`Favorite microphones${devices.mics.length > 1 ? ` (${devices.mics.length})` : ""}`}
                hint="Multi-select mics on this machine."
                value={favoriteMics}
                onChange={(vals) => {
                  setFavoriteMics(vals);
                  if (vals.length && !vals.includes(micId)) setMicId(vals[0]);
                  if (!vals.length) setMicId("");
                }}
                options={micOptions}
                emptyLabel="No microphone found"
                placeholder="Select microphones…"
              />
              <Select
                label="Active microphone"
                icon={<IconMicrophone size={18} stroke={1.75} />}
                value={micId}
                onChange={setMicId}
                options={(favoriteMics.length
                  ? devices.mics.filter((d) => favoriteMics.includes(d.deviceId))
                  : devices.mics
                ).map((d) => ({ value: d.deviceId, label: d.label }))}
                emptyLabel="No microphone selected"
              />
              <div className="level-meter rs-settings-meter">
                <span style={{ width: `${Math.round(micLevel * 100)}%` }} />
              </div>
              <Select
                label="Speakers / output"
                icon={<IconVolume size={18} stroke={1.75} />}
                value={speakerId}
                onChange={setSpeakerId}
                options={speakerOptions}
                emptyLabel="System default"
                placeholder="System default"
              />
            </div>
          </div>
        </section>

        <section className="rs-settings-card">
          <div className="rs-settings-card-head">
            <h2>Google Drive</h2>
            <p>
              Status:{" "}
              <strong className={settings.drive_connected ? "is-ok" : ""}>
                {settings.drive_connected ? "Connected" : "Not connected"}
              </strong>
            </p>
          </div>
          {!settings.drive_connected ? (
            <div className="rs-settings-fields">
              <button
                type="button"
                className="rs-join-btn rs-settings-primary"
                onClick={() => void connectDrive()}
              >
                Connect Google Drive
              </button>
              <div className="field">
                <label>OAuth code (from redirect)</label>
                <input
                  value={oauthCode}
                  onChange={(e) => setOauthCode(e.target.value)}
                  placeholder="Paste code here"
                />
              </div>
              <button
                type="button"
                className="btn btn-secondary"
                onClick={() => void finishDrive()}
              >
                Complete connection
              </button>
            </div>
          ) : (
            <button
              type="button"
              className="btn btn-secondary"
              onClick={async () => {
                await api.disconnectDrive();
                await refreshSettings();
                setMsg("Google Drive disconnected");
              }}
            >
              Disconnect
            </button>
          )}
        </section>

        {/* AI provider — paused for now
        <section className="rs-settings-card">
          <div className="rs-settings-card-head">
            <h2>AI provider</h2>
            <p>
              Used for subtitles (Whisper) and show notes. OpenAI, Groq, or any
              OpenAI-compatible endpoint.
            </p>
          </div>
          <div className="rs-settings-fields">
            <div className="field">
              <label>Base URL</label>
              <input
                value={aiUrl}
                onChange={(e) => setAiUrl(e.target.value)}
                placeholder="https://api.openai.com/v1"
              />
            </div>
            <div className="field">
              <label>API key</label>
              <input
                type="password"
                value={aiKey}
                onChange={(e) => setAiKey(e.target.value)}
                placeholder={settings.ai_has_key ? "•••••••• (saved)" : ""}
              />
            </div>
          </div>
        </section>
        */}

        {/* Subtitles / Whisper model — paused for now
        <section className="rs-settings-card">
          <div className="rs-settings-card-head">
            <h2>Subtitles</h2>
            <p>
              Captions can use on-device speech recognition or your AI Whisper
              endpoint. Optional local Whisper model:
            </p>
          </div>
          <p>Model: {whisperReady ? "Downloaded" : "Not downloaded"}</p>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={async () => {
              setMsg("Downloading Whisper model…");
              try {
                await api.downloadWhisperModel();
                setWhisperReady(true);
                setMsg("Model ready");
              } catch (e) {
                setMsg(String(e));
              }
            }}
          >
            Download ggml-base.en
          </button>
        </section>
        */}

        <section className="rs-settings-card">
          <div className="rs-settings-card-head">
            <h2>Onboarding</h2>
            <p>Replay the first-run setup for this machine.</p>
          </div>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => {
              resetOnboarding();
              navigate("/onboarding", { replace: true });
            }}
          >
            Replay onboarding
          </button>
          {paths && (
            <p className="rs-settings-paths">
              Data · {paths.data}
            </p>
          )}
        </section>

        <div className="rs-settings-footer">
          <button
            type="button"
            className="rs-join-btn rs-settings-primary"
            disabled={saving}
            onClick={() => void save()}
          >
            {saving ? "Saving…" : "Save settings"}
          </button>
          {msg && <p className="rs-settings-msg">{msg}</p>}
        </div>
      </div>
    </div>
  );
}

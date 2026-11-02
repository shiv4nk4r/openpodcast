import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  IconCamera,
  IconCameraOff,
  IconChevronDown,
  IconHeadphones,
  IconMicrophone,
  IconMicrophoneOff,
  IconSparkles,
  IconVolume,
  IconWaveSine,
} from "@tabler/icons-react";
import { api } from "../api";
import { LiveCameraPreview } from "../components/LiveCameraPreview";
import { Select } from "../components/Select";
import { pickPreferred, useDeviceList } from "../hooks/useDeviceList";
import { useLocalMedia } from "../hooks/useLocalMedia";
import {
  FRAME_RATE_OPTIONS,
  RESOLUTION_OPTIONS,
} from "../lib/studioQuality";
import { useApp } from "../state";
import type { StudioMode } from "../types";

type Panel = "host" | "invite";

export default function Lobby({ initialPanel }: { initialPanel?: Panel }) {
  const { studio, settings, setStudio, refreshSettings } = useApp();
  const navigate = useNavigate();
  const [panel, setPanel] = useState<Panel>(initialPanel ?? "host");
  const [booting, setBooting] = useState(!studio);
  const [bootError, setBootError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [joinError, setJoinError] = useState<string | null>(null);
  const [invite, setInvite] = useState("");

  const videoMode = (studio?.settings.mode ?? "audio_video") === "audio_video";
  const studioName = studio?.settings.name ?? "OpenPodcast Studio";
  const resolution = studio?.settings.resolution ?? "1080p";
  const fps = studio?.settings.frame_rate ?? 30;
  const canEditQuality = studio?.role !== "guest";

  const deviceList = useDeviceList();
  const media = useLocalMedia({
    video: videoMode,
    audio: true,
    echoCancellation: settings.echo_cancellation,
    skipAutoStart: true,
    resolution,
    frameRate: fps,
  });

  const [name, setName] = useState(settings.display_name || "");
  const [headphones, setHeadphones] = useState(true);
  const [audioId, setAudioId] = useState(settings.preferred_mic_id ?? "");
  const [videoId, setVideoId] = useState(settings.preferred_camera_id ?? "");
  const [speakerId, setSpeakerId] = useState(settings.preferred_speaker_id ?? "");
  const [camOn, setCamOn] = useState(true);
  const [micOn, setMicOn] = useState(true);
  const [qualityOpen, setQualityOpen] = useState(false);
  const qualityRef = useRef<HTMLDivElement | null>(null);

  // Host studio bootstrap (still fine while viewing invite panel)
  useEffect(() => {
    if (studio) {
      setBooting(false);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const studioSettings = {
          name: "Untitled Recording",
          mode: "audio_video" as StudioMode,
          frame_rate: 30,
          resolution: "1080p",
        };
        const res = await api.createStudio(
          studioSettings,
          settings.display_name || "Host",
        );
        if (cancelled) return;
        setStudio({
          role: "host",
          sessionId: res.session_id,
          participantId: "host",
          invite: res.invite,
          settings: studioSettings,
          displayName: settings.display_name || "Host",
        });
      } catch (e) {
        if (!cancelled) setBootError(String(e));
      } finally {
        if (!cancelled) setBooting(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [studio, settings.display_name, setStudio]);

  useEffect(() => {
    const nextMic = pickPreferred(deviceList.mics, settings.preferred_mic_id);
    const nextCam = pickPreferred(deviceList.cameras, settings.preferred_camera_id);
    const nextOut = pickPreferred(deviceList.speakers, settings.preferred_speaker_id);
    if (nextMic && nextMic !== audioId) setAudioId(nextMic);
    if (nextCam && nextCam !== videoId) setVideoId(nextCam);
    if (nextOut && nextOut !== speakerId) setSpeakerId(nextOut);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceList.mics, deviceList.cameras, deviceList.speakers]);

  useEffect(() => {
    if (booting || (!audioId && !videoId)) return;
    void media.start({
      audio: audioId || undefined,
      video: videoMode ? videoId || undefined : undefined,
    });
  }, [booting, audioId, videoId, videoMode, resolution, fps]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    media.toggleTrack("video", camOn);
  }, [camOn]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    media.toggleTrack("audio", micOn);
  }, [micOn]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!qualityOpen) return;
    const onDoc = (e: MouseEvent) => {
      if (!qualityRef.current?.contains(e.target as Node)) {
        setQualityOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setQualityOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [qualityOpen]);

  async function applyQuality(next: { resolution?: string; frameRate?: number }) {
    if (!studio || studio.role === "guest") return;
    const settingsNext = {
      ...studio.settings,
      resolution: next.resolution ?? studio.settings.resolution,
      frame_rate: next.frameRate ?? studio.settings.frame_rate,
    };
    const nextStudio = { ...studio, settings: settingsNext };
    setStudio(nextStudio);
    try {
      await api.updateStudioSettings(studio.sessionId, settingsNext);
      const invite = await api.getInvite().catch(() => undefined);
      if (invite) setStudio({ ...nextStudio, invite });
    } catch (e) {
      console.warn("Failed to persist studio quality", e);
    }
  }

  async function persistDevices(next?: {
    mic?: string;
    camera?: string;
    speaker?: string;
  }) {
    await api.saveSettings({
      ...settings,
      display_name: name.trim() || (panel === "invite" ? "Guest" : "Host"),
      preferred_mic_id: next?.mic ?? (audioId || null),
      preferred_camera_id: next?.camera ?? (videoId || null),
      preferred_speaker_id: next?.speaker ?? (speakerId || null),
    });
    await refreshSettings();
  }

  async function joinAsHost() {
    if (!studio) return;
    await persistDevices();
    setStudio({
      ...studio,
      displayName: name.trim() || "Host",
    });
    navigate("/studio");
  }

  async function joinWithInvite() {
    const ticket = invite.trim();
    if (!ticket) {
      setJoinError("Paste an invite to join.");
      return;
    }
    setBusy(true);
    setJoinError(null);
    try {
      await persistDevices();
      await api.leaveStudio().catch(() => undefined);
      const displayName = name.trim() || settings.display_name || "Guest";
      const res = await api.joinStudio(ticket, displayName);
      setStudio({
        role: "guest",
        sessionId: res.session_id,
        participantId: res.participant_id,
        settings: res.studio,
        displayName,
      });
      navigate("/studio");
    } catch (e) {
      setJoinError(String(e).replace(/^Error:\s*/i, ""));
    } finally {
      setBusy(false);
    }
  }

  function showInvitePanel() {
    // Swap left panel only — keep camera / devices mounted
    setJoinError(null);
    setPanel("invite");
  }

  function showHostPanel() {
    setJoinError(null);
    setPanel("host");
  }

  if (booting) {
    return (
      <div className="rs-lobby">
        <div className="rs-lobby-loading">Preparing your studio…</div>
      </div>
    );
  }

  if (bootError) {
    return (
      <div className="rs-lobby">
        <div className="rs-lobby-loading" style={{ color: "var(--record)" }}>
          {bootError}
        </div>
      </div>
    );
  }

  return (
    <div className="rs-lobby">
      <div className="rs-lobby-shell">
        <section className="rs-lobby-left">
          {panel === "host" ? (
            <>
              <p className="rs-lobby-eyebrow">You’re about to join {studioName}</p>
              <h1 className="rs-lobby-title">Let’s check your cam and mic</h1>

              <div className="rs-name-row">
                <input
                  className="rs-name-input"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Your name"
                />
                <span className="rs-role-pill">Host</span>
              </div>

              <div className="rs-headphone-toggle">
                <button
                  type="button"
                  className={!headphones ? "is-active" : ""}
                  onClick={() => setHeadphones(false)}
                >
                  <IconVolume size={16} stroke={1.75} />
                  I am not using headphones
                </button>
                <button
                  type="button"
                  className={headphones ? "is-active" : ""}
                  onClick={() => setHeadphones(true)}
                >
                  <IconHeadphones size={16} stroke={1.75} />
                  I am using headphones
                </button>
              </div>

              <button
                type="button"
                className="rs-join-btn"
                onClick={() => void joinAsHost()}
              >
                Join Studio
              </button>

              <p className="rs-lobby-foot">
                You are joining as a host.{" "}
                <button type="button" className="rs-link" onClick={showInvitePanel}>
                  Join with invite
                </button>
              </p>
            </>
          ) : (
            <>
              <p className="rs-lobby-eyebrow">You’re joining with an invite</p>
              <h1 className="rs-lobby-title">Let’s check your cam and mic</h1>

              <div className="rs-name-row">
                <input
                  className="rs-name-input"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Your name"
                />
                <span className="rs-role-pill">Guest</span>
              </div>

              <div className="rs-name-row rs-invite-row">
                <input
                  className="rs-name-input"
                  value={invite}
                  onChange={(e) => setInvite(e.target.value)}
                  placeholder="Paste invite (opcast_…)"
                  autoFocus
                  spellCheck={false}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && invite.trim() && !busy) {
                      void joinWithInvite();
                    }
                  }}
                />
              </div>

              <div className="rs-headphone-toggle">
                <button
                  type="button"
                  className={!headphones ? "is-active" : ""}
                  onClick={() => setHeadphones(false)}
                >
                  <IconVolume size={16} stroke={1.75} />
                  I am not using headphones
                </button>
                <button
                  type="button"
                  className={headphones ? "is-active" : ""}
                  onClick={() => setHeadphones(true)}
                >
                  <IconHeadphones size={16} stroke={1.75} />
                  I am using headphones
                </button>
              </div>

              {joinError && <p className="rs-join-error">{joinError}</p>}

              <button
                type="button"
                className="rs-join-btn"
                disabled={busy || !invite.trim()}
                onClick={() => void joinWithInvite()}
              >
                {busy ? "Joining…" : "Join Studio"}
              </button>

              <p className="rs-lobby-foot">
                You are joining as a guest.{" "}
                <button type="button" className="rs-link" onClick={showHostPanel}>
                  Back to host studio
                </button>
              </p>
            </>
          )}
        </section>

        <section className="rs-lobby-right">
          <div className="rs-preview-card">
            <div
              className={`rs-preview-frame${qualityOpen ? " is-quality-open" : ""}`}
            >
              <div className="rs-preview-media">
                {videoMode && camOn ? (
                  <LiveCameraPreview
                    className="rs-preview-video"
                    setVideoEl={media.setVideoEl}
                  />
                ) : (
                  <div className="rs-preview-fallback">
                    {(name || "Y").slice(0, 1).toUpperCase()}
                  </div>
                )}
              </div>
              <div className="rs-quality" ref={qualityRef}>
                <button
                  type="button"
                  className={`rs-preview-badge${canEditQuality ? " is-editable" : ""}`}
                  disabled={!canEditQuality}
                  aria-expanded={qualityOpen}
                  aria-haspopup="dialog"
                  title={
                    canEditQuality
                      ? "Change resolution and frame rate"
                      : "Recording quality (set by host)"
                  }
                  onClick={() => {
                    if (canEditQuality) setQualityOpen((v) => !v);
                  }}
                >
                  {resolution} / {fps}fps
                  {canEditQuality && (
                    <IconChevronDown size={12} stroke={2.25} />
                  )}
                </button>
                {qualityOpen && canEditQuality && (
                  <div className="rs-quality-menu" role="dialog" aria-label="Recording quality">
                    <Select
                      label="Resolution"
                      value={resolution}
                      onChange={(value) => void applyQuality({ resolution: value })}
                      options={RESOLUTION_OPTIONS.map((o) => ({
                        value: o.value,
                        label: o.label,
                        description: o.description,
                      }))}
                    />
                    <Select
                      label="Frame rate"
                      value={String(fps)}
                      onChange={(value) =>
                        void applyQuality({ frameRate: Number(value) })
                      }
                      options={FRAME_RATE_OPTIONS.map((o) => ({
                        value: o.value,
                        label: o.label,
                        description: o.description,
                      }))}
                    />
                  </div>
                )}
              </div>
              <button type="button" className="rs-effects-btn" title="Effects">
                <IconSparkles size={14} stroke={2} />
                Effects
              </button>
              <div className="rs-preview-controls">
                <button
                  type="button"
                  className={`rs-circle-btn ${!camOn ? "is-off" : ""}`}
                  onClick={() => setCamOn((v) => !v)}
                  title="Camera"
                >
                  {camOn ? (
                    <IconCamera size={18} stroke={1.75} />
                  ) : (
                    <IconCameraOff size={18} stroke={1.75} />
                  )}
                </button>
                <button
                  type="button"
                  className={`rs-circle-btn ${!micOn ? "is-off" : ""}`}
                  onClick={() => setMicOn((v) => !v)}
                  title="Mic"
                >
                  {micOn ? (
                    <IconMicrophone size={18} stroke={1.75} />
                  ) : (
                    <IconMicrophoneOff size={18} stroke={1.75} />
                  )}
                </button>
              </div>
            </div>

            <div className="rs-device-stack">
              <Select
                label="Camera"
                icon={<IconCamera size={18} stroke={1.75} />}
                value={videoId}
                onChange={async (id) => {
                  setVideoId(id);
                  await media.start({
                    audio: audioId || undefined,
                    video: id,
                  });
                  await persistDevices({ camera: id });
                }}
                options={deviceList.cameras.map((d) => ({
                  value: d.deviceId,
                  label: d.label,
                }))}
                emptyLabel="No camera found"
              />
              <div className="rs-mic-block">
                <Select
                  label="Microphone"
                  icon={<IconMicrophone size={18} stroke={1.75} />}
                  value={audioId}
                  onChange={async (id) => {
                    setAudioId(id);
                    await media.start({
                      audio: id,
                      video: videoMode ? videoId || undefined : undefined,
                    });
                    await persistDevices({ mic: id });
                  }}
                  options={deviceList.mics.map((d) => ({
                    value: d.deviceId,
                    label: d.label,
                  }))}
                  emptyLabel="No microphone found"
                />
                <div className="rs-inline-meter" aria-label="Microphone level">
                  <span style={{ width: `${Math.round(media.level * 100)}%` }} />
                </div>
              </div>
              <Select
                label="Speakers"
                icon={<IconVolume size={18} stroke={1.75} />}
                value={speakerId}
                onChange={async (id) => {
                  setSpeakerId(id);
                  await persistDevices({ speaker: id });
                }}
                options={deviceList.speakers.map((d) => ({
                  value: d.deviceId,
                  label: d.label,
                }))}
                emptyLabel="System default"
                placeholder="System default"
              />
            </div>
          </div>
        </section>
      </div>

      <nav className="rs-lobby-nav" aria-label="More">
        <button type="button" onClick={() => navigate("/recordings")}>
          <IconWaveSine size={16} stroke={1.75} />
          Past recordings
        </button>
      </nav>
    </div>
  );
}

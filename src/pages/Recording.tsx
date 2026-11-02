import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  IconCamera,
  IconCameraOff,
  IconLayoutGrid,
  IconMicrophone,
  IconMicrophoneOff,
  IconPhoneOff,
  IconCopy,
  IconScreenShare,
  IconScript,
  IconSettings,
  IconUserPlus,
  IconUsers,
  IconVolume,
  IconX,
} from "@tabler/icons-react";
import { api } from "../api";
import { LiveCameraPreview } from "../components/LiveCameraPreview";
import { Select } from "../components/Select";
import { pickPreferred, useDeviceList } from "../hooks/useDeviceList";
import { blobToBytes, useLocalMedia } from "../hooks/useLocalMedia";
import { useApp } from "../state";
import type { PeerInfo } from "../types";

type Layout = "auto" | "grid" | "speaker";
type SideTab = "people" | "layout";

const LAYOUT_OPTIONS: {
  id: Layout;
  label: string;
  description: string;
}[] = [
  {
    id: "auto",
    label: "Auto",
    description: "Fills the stage based on how many people are here",
  },
  {
    id: "grid",
    label: "Equal grid",
    description: "Same-size tiles in a balanced grid",
  },
  {
    id: "speaker",
    label: "Spotlight",
    description: "You stay large; others stack beside you",
  },
];

export default function Recording() {
  const { studio, settings, setStudio, refreshSessions } = useApp();
  const navigate = useNavigate();
  const videoMode = studio?.settings.mode === "audio_video";
  const deviceList = useDeviceList();
  const media = useLocalMedia({
    video: !!videoMode,
    audio: true,
    echoCancellation: settings.echo_cancellation,
    skipAutoStart: true,
    resolution: studio?.settings.resolution,
    frameRate: studio?.settings.frame_rate,
  });

  const [peers, setPeers] = useState<PeerInfo[]>([]);
  const [inviteOpen, setInviteOpen] = useState(true);
  const [inviteCopied, setInviteCopied] = useState(false);
  const [sideTab, setSideTab] = useState<SideTab>("people");
  const [peopleOpen, setPeopleOpen] = useState(true);
  const [micOn, setMicOn] = useState(true);
  const [camOn, setCamOn] = useState(true);
  const [layout, setLayout] = useState<Layout>("auto");
  const [recording, setRecording] = useState(false);
  const [hasRecording, setHasRecording] = useState(false);
  const [countdown, setCountdown] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [uploadFrac, setUploadFrac] = useState(0);
  const [showStayModal, setShowStayModal] = useState(false);
  const [scriptOpen, setScriptOpen] = useState(false);
  const [script, setScript] = useState("");
  const [echo, setEcho] = useState(settings.echo_cancellation);
  const [audioDevice, setAudioDevice] = useState(settings.preferred_mic_id ?? "");
  const [videoDevice, setVideoDevice] = useState(settings.preferred_camera_id ?? "");
  const [speakerDevice, setSpeakerDevice] = useState(settings.preferred_speaker_id ?? "");
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [renamingTitle, setRenamingTitle] = useState(false);
  const startedAt = useRef<number>(0);
  const timerRef = useRef<number>(0);
  const mediaBooted = useRef(false);
  const titleInputRef = useRef<HTMLInputElement | null>(null);
  /** Everyone who appeared in the lobby / studio during this session */
  const rosterRef = useRef<Map<string, PeerInfo>>(new Map());

  useEffect(() => {
    if (!studio) navigate("/", { replace: true });
  }, [studio, navigate]);

  // Seed host / self into the session roster for playback metadata
  useEffect(() => {
    if (!studio) return;
    rosterRef.current.set(studio.participantId, {
      id: studio.participantId,
      name: studio.displayName || (studio.role === "host" ? "Host" : "Guest"),
      role: studio.role === "host" ? "Host" : "Guest",
      admitted: true,
    });
  }, [studio]);

  useEffect(() => {
    if (!studio || studio.role !== "host") return;
    const id = window.setInterval(async () => {
      try {
        const list = await api.listPeers();
        setPeers(list);
        for (const p of list) {
          rosterRef.current.set(p.id, {
            ...p,
            role: p.role || "Guest",
          });
        }
      } catch {
        /* ignore */
      }
    }, 1500);
    return () => clearInterval(id);
  }, [studio]);

  useEffect(() => {
    const nextMic = pickPreferred(deviceList.mics, settings.preferred_mic_id);
    const nextCam = pickPreferred(deviceList.cameras, settings.preferred_camera_id);
    const nextOut = pickPreferred(deviceList.speakers, settings.preferred_speaker_id);
    if (nextMic) setAudioDevice(nextMic);
    if (nextCam) setVideoDevice(nextCam);
    if (nextOut) setSpeakerDevice(nextOut);
  }, [
    deviceList.mics,
    deviceList.cameras,
    deviceList.speakers,
    settings.preferred_mic_id,
    settings.preferred_camera_id,
    settings.preferred_speaker_id,
  ]);

  useEffect(() => {
    if (!audioDevice && !videoDevice) return;
    mediaBooted.current = true;
    void media.start({
      audio: audioDevice || undefined,
      video: videoMode ? videoDevice || undefined : undefined,
    });
  }, [audioDevice, videoDevice, videoMode]); // eslint-disable-line react-hooks/exhaustive-deps

  async function saveDevicePrefs(next: {
    mic?: string;
    camera?: string;
    speaker?: string;
  }) {
    await api.saveSettings({
      ...settings,
      preferred_mic_id: next.mic ?? (audioDevice || null),
      preferred_camera_id: next.camera ?? (videoDevice || null),
      preferred_speaker_id: next.speaker ?? (speakerDevice || null),
    });
  }

  const people = useMemo(() => {
    const self = {
      id: studio?.participantId ?? "host",
      name: `${studio?.displayName ?? "You"} (You)`,
      role: studio?.role === "host" ? "Host" : "Guest",
      resolution: studio?.settings.resolution ?? "1080p",
      admitted: true,
    };
    return [self, ...peers.filter((p) => p.admitted)];
  }, [studio, peers]);

  useEffect(() => {
    if (!editingTitle) return;
    const el = titleInputRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, [editingTitle]);

  if (!studio) return null;
  const active = studio;

  async function copyInvite() {
    try {
      const invite = active.invite ?? (await api.getInvite());
      await navigator.clipboard.writeText(invite);
      setInviteCopied(true);
      setTimeout(() => setInviteCopied(false), 2000);
    } catch (e) {
      alert(String(e));
    }
  }

  function beginRenameTitle() {
    if (active.role !== "host" || renamingTitle) return;
    setTitleDraft(active.settings.name || "Untitled Recording");
    setEditingTitle(true);
  }

  async function commitRenameTitle() {
    if (renamingTitle) return;
    const next = titleDraft.trim();
    setEditingTitle(false);
    if (!next || next === active.settings.name) {
      setTitleDraft(active.settings.name);
      return;
    }
    const settingsNext = { ...active.settings, name: next };
    const nextStudio = { ...active, settings: settingsNext };
    setStudio(nextStudio);
    setRenamingTitle(true);
    try {
      await api.updateStudioSettings(active.sessionId, settingsNext);
      const invite = await api.getInvite().catch(() => undefined);
      if (invite) setStudio({ ...nextStudio, invite });
    } catch (e) {
      alert(String(e));
      setStudio(active);
    } finally {
      setRenamingTitle(false);
    }
  }

  function cancelRenameTitle() {
    setEditingTitle(false);
    setTitleDraft(active.settings.name);
  }

  function startCountdown() {
    if (active.role !== "host") return;
    setCountdown(5);
    let n = 5;
    const id = window.setInterval(() => {
      n -= 1;
      if (n <= 0) {
        clearInterval(id);
        setCountdown(null);
        void beginRecord();
      } else {
        setCountdown(n);
      }
    }, 1000);
  }

  async function beginRecord() {
    setRecording(true);
    startedAt.current = Date.now();
    timerRef.current = window.setInterval(() => {
      setElapsed(Math.floor((Date.now() - startedAt.current) / 1000));
    }, 250);
    try {
      await api.startAudioRecording(
        active.sessionId,
        active.participantId,
        deviceList.mics.find((d) => d.deviceId === audioDevice)?.label,
      );
    } catch {
      /* MediaRecorder still captures */
    }
    try {
      media.startRecording();
    } catch {
      /* ignore */
    }
  }

  async function stopRecord() {
    setRecording(false);
    clearInterval(timerRef.current);
    const duration = (Date.now() - startedAt.current) / 1000;
    setShowStayModal(true);
    setUploadFrac(0.1);

    try {
      await api.stopAudioRecording(active.participantId);
    } catch {
      /* ok */
    }

    // Capture a still while the camera preview is still live
    try {
      const thumb = await media.grabJpegFrame();
      if (thumb?.length) {
        await api.saveSessionThumbnail(active.sessionId, thumb);
      }
    } catch {
      /* optional */
    }

    // Only persist video if we actually started MediaRecorder via Record
    const blob = await media.stopRecording();
    if (blob && blob.size > 0) {
      const bytes = await blobToBytes(blob);
      const ext = blob.type.includes("mp4") ? "mp4" : "webm";
      await api.saveVideoTrack(active.sessionId, active.participantId, bytes, ext);
    }

    setUploadFrac(0.55);
    // Persist host + everyone who showed up in the lobby/studio
    const byId = new Map<string, { id: string; name: string; role: string; resolution: string }>();
    byId.set(active.participantId, {
      id: active.participantId,
      name: (active.displayName || "Host").replace(" (You)", ""),
      role: active.role === "host" ? "Host" : "Guest",
      resolution: active.settings.resolution,
    });
    for (const p of rosterRef.current.values()) {
      byId.set(p.id, {
        id: p.id,
        name: p.name,
        role: p.admitted
          ? p.role || "Guest"
          : p.role
            ? `${p.role} · lobby`
            : "Lobby",
        resolution: active.settings.resolution,
      });
    }
    for (const p of people) {
      byId.set(p.id, {
        id: p.id,
        name: p.name.replace(" (You)", ""),
        role: p.role,
        resolution: active.settings.resolution,
      });
    }
    const participants = [...byId.values()];
    await api.finalizeSession(active.sessionId, duration, participants, {});
    setHasRecording(true);
    setUploadFrac(0.75);

    if (settings.drive_connected) {
      try {
        await api.uploadSessionToDrive(active.sessionId);
        setUploadFrac(1);
      } catch {
        setUploadFrac(0.9);
      }
    } else {
      setUploadFrac(1);
    }

    await refreshSessions();
  }

  async function leave() {
    let saved = hasRecording;
    if (recording) {
      await stopRecord();
      saved = true;
    }
    await api.leaveStudio().catch(() => undefined);
    const id = active.sessionId;
    setStudio(null);
    if (saved) {
      await refreshSessions().catch(() => undefined);
      navigate("/recordings");
      return;
    }
    // Never pressed Record — discard preview session so it never appears in Recordings
    await api.discardSession(id).catch(() => undefined);
    await refreshSessions().catch(() => undefined);
    navigate("/");
  }

  function formatTime(s: number) {
    const m = Math.floor(s / 60);
    const sec = s % 60;
    return `${m.toString().padStart(2, "0")}:${sec.toString().padStart(2, "0")}`;
  }

  const levelPct = Math.round(media.level * 100);
  const showInviteTile = active.role === "host" && inviteOpen;
  const guestTiles = people.slice(1);
  const tileCount = 1 + (showInviteTile ? 1 : 0) + guestTiles.length;
  const stageMulti = tileCount > 1;
  const stageCountClass = stageMulti
    ? ` is-count-${Math.min(tileCount, 6)}`
    : "";
  const stageLayoutClass =
    stageMulti && layout !== "auto" ? ` is-layout-${layout}` : "";

  return (
    <div className="rs-studio">
      <header className="rs-topbar">
        <div className="rs-top-left">
          <strong className="rs-logo">OpenPodcast</strong>
          <span className="rs-sep">/</span>
          {editingTitle ? (
            <input
              ref={titleInputRef}
              className="rs-title-input"
              value={titleDraft}
              aria-label="Recording name"
              disabled={renamingTitle}
              onChange={(e) => setTitleDraft(e.target.value)}
              onBlur={() => void commitRenameTitle()}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  e.currentTarget.blur();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  cancelRenameTitle();
                }
              }}
            />
          ) : active.role === "host" ? (
            <button
              type="button"
              className="rs-title-btn"
              title="Click to rename"
              onClick={beginRenameTitle}
            >
              {active.settings.name || "Untitled Recording"}
            </button>
          ) : (
            <span className="rs-title-readonly">
              {active.settings.name || "Untitled Recording"}
            </span>
          )}
        </div>
        <div className="rs-top-right">
          <button type="button" className="rs-icon-btn" onClick={() => navigate("/settings")}>
            <IconSettings size={18} stroke={1.75} />
          </button>
          {active.role === "host" && (
            <button
              type="button"
              className={`rs-invite-btn${inviteOpen ? " is-active" : ""}`}
              onClick={() => setInviteOpen((v) => !v)}
            >
              <IconUserPlus size={16} stroke={1.75} />
              Invite
            </button>
          )}
          <div className="avatar">{active.displayName.slice(0, 1).toUpperCase()}</div>
        </div>
      </header>

      <div className="rs-studio-body">
        <div className="rs-stage-wrap">
          <div
            className={`rs-stage${
              stageMulti
                ? ` is-grid${stageCountClass}${stageLayoutClass}`
                : " is-solo"
            }`}
          >
            <div className="rs-stage-tile">
              {videoMode && camOn ? (
                <LiveCameraPreview
                  className="rs-stage-preview"
                  setVideoEl={media.setVideoEl}
                />
              ) : (
                <div className="rs-stage-empty">{active.displayName}</div>
              )}
              <div className="rs-stage-name">{active.displayName}</div>
              <div className="rs-vumeter" aria-hidden>
                {Array.from({ length: 12 }).map((_, i) => (
                  <span
                    key={i}
                    className={levelPct > (i + 1) * (100 / 12) ? "is-on" : ""}
                  />
                ))}
              </div>
            </div>

            {showInviteTile && (
              <div className="rs-stage-tile rs-invite-tile">
                <button
                  type="button"
                  className="rs-invite-dismiss"
                  onClick={() => setInviteOpen(false)}
                  aria-label="Hide invite"
                >
                  <IconX size={16} stroke={2} />
                </button>
                <div className="rs-invite-tile-inner">
                  <div className="rs-invite-icon" aria-hidden>
                    <IconUserPlus size={28} stroke={1.5} />
                  </div>
                  <h2>Invite people</h2>
                  <p>
                    Share this invite so guests can join your lobby and enter the
                    studio.
                  </p>
                  <button
                    type="button"
                    className="rs-invite-copy"
                    onClick={() => void copyInvite()}
                  >
                    <IconCopy size={16} stroke={1.75} />
                    {inviteCopied ? "Copied" : "Copy invite"}
                  </button>
                </div>
              </div>
            )}

            {guestTiles.map((p) => (
              <div className="rs-stage-tile rs-stage-empty" key={p.id}>
                <div className="rs-stage-name">{p.name}</div>
              </div>
            ))}
          </div>

          {scriptOpen && (
            <div className="rs-script-float">
              <strong>Script</strong>
              <textarea
                value={script}
                onChange={(e) => setScript(e.target.value)}
                placeholder="Talking points…"
              />
            </div>
          )}
        </div>

        {peopleOpen && (
          <aside className="rs-people">
            <div className="rs-people-head">
              <h2>{sideTab === "people" ? "People" : "Layout"}</h2>
              <button type="button" onClick={() => setPeopleOpen(false)} aria-label="Close">
                <IconX size={16} stroke={2} />
              </button>
            </div>

            {sideTab === "people" && (
              <div className="rs-people-list">
                {people.map((p) => (
                  <div className="rs-person-card" key={p.id}>
                    <div className="rs-person-top">
                      <div>
                        <strong>{p.name.replace(" (You)", "")}</strong>
                        <div className="rs-muted">
                          {p.role} · {active.settings.resolution}
                        </div>
                      </div>
                    </div>
                    {p.id === active.participantId && (
                      <>
                        <div className="rs-seg-meter" aria-label="Microphone level">
                          {Array.from({ length: 20 }).map((_, i) => (
                            <i
                              key={i}
                              className={levelPct > (i + 1) * 5 ? "is-on" : ""}
                            />
                          ))}
                        </div>
                        <Select
                          label="Camera"
                          icon={<IconCamera size={16} stroke={1.75} />}
                          value={videoDevice}
                          onChange={async (id) => {
                            setVideoDevice(id);
                            await media.start({
                              audio: audioDevice || undefined,
                              video: id,
                            });
                            await saveDevicePrefs({ camera: id });
                          }}
                          options={deviceList.cameras.map((d) => ({
                            value: d.deviceId,
                            label: d.label,
                          }))}
                          emptyLabel="No camera found"
                        />
                        <Select
                          label="Microphone"
                          icon={<IconMicrophone size={16} stroke={1.75} />}
                          value={audioDevice}
                          onChange={async (id) => {
                            setAudioDevice(id);
                            await media.start({
                              audio: id,
                              video: videoMode ? videoDevice || undefined : undefined,
                            });
                            await saveDevicePrefs({ mic: id });
                          }}
                          options={deviceList.mics.map((d) => ({
                            value: d.deviceId,
                            label: d.label,
                          }))}
                          emptyLabel="No microphone found"
                        />
                        <label className="toggle">
                          <span>Echo Cancellation</span>
                          <input
                            type="checkbox"
                            checked={echo}
                            onChange={(e) => setEcho(e.target.checked)}
                          />
                        </label>
                      </>
                    )}
                  </div>
                ))}
                {active.role === "host" &&
                  peers
                    .filter((g) => !g.admitted)
                    .map((g) => (
                      <div className="rs-person-card" key={`wait-${g.id}`}>
                        <strong>{g.name}</strong>
                        <div className="rs-muted">Waiting in lobby</div>
                        <button
                          type="button"
                          className="rs-copy-btn"
                          style={{ marginTop: 10 }}
                          onClick={() => void api.admitGuest(g.id)}
                        >
                          Admit
                        </button>
                      </div>
                    ))}
              </div>
            )}

            {sideTab === "layout" && (
              <div className="rs-layout-panel">
                <p className="rs-layout-hint">
                  How the stage arranges when more than one tile is open.
                </p>
                <div className="rs-layout-options">
                  {LAYOUT_OPTIONS.map((opt) => (
                    <button
                      key={opt.id}
                      type="button"
                      className={`rs-layout-option${layout === opt.id ? " is-active" : ""}`}
                      onClick={() => setLayout(opt.id)}
                    >
                      <strong>{opt.label}</strong>
                      <span>{opt.description}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}
          </aside>
        )}

        <nav className="rs-rail">
          {(
            [
              ["people", "People", IconUsers],
              ["layout", "Layout", IconLayoutGrid],
            ] as const
          ).map(([id, label, RailIcon]) => (
            <button
              key={id}
              type="button"
              className={sideTab === id && peopleOpen ? "is-active" : ""}
              title={label}
              onClick={() => {
                if (peopleOpen && sideTab === id) {
                  setPeopleOpen(false);
                  return;
                }
                setSideTab(id);
                setPeopleOpen(true);
              }}
            >
              <RailIcon size={20} stroke={1.75} />
            </button>
          ))}
        </nav>
      </div>

      <div className="rs-bottom-bar">
        {active.role === "host" && (
          <button
            type="button"
            className={`rs-record ${recording ? "is-live" : ""}`}
            onClick={() => (recording ? void stopRecord() : startCountdown())}
          >
            <span className="rs-record-dot" />
            {recording ? formatTime(elapsed) : "Record"}
          </button>
        )}
        <button
          type="button"
          className={`rs-bar-item ${!micOn ? "is-off" : ""}`}
          onClick={() => {
            const next = !micOn;
            setMicOn(next);
            media.toggleTrack("audio", next);
          }}
        >
          {micOn ? (
            <IconMicrophone size={20} stroke={1.75} />
          ) : (
            <IconMicrophoneOff size={20} stroke={1.75} />
          )}
          Mic
        </button>
        <button
          type="button"
          className={`rs-bar-item ${!camOn ? "is-off" : ""}`}
          disabled={!videoMode}
          onClick={() => {
            const next = !camOn;
            setCamOn(next);
            media.toggleTrack("video", next);
          }}
        >
          {camOn ? (
            <IconCamera size={20} stroke={1.75} />
          ) : (
            <IconCameraOff size={20} stroke={1.75} />
          )}
          Cam
        </button>
        <button type="button" className="rs-bar-item">
          <IconVolume size={20} stroke={1.75} />
          Speaker
        </button>
        <button
          type="button"
          className="rs-bar-item"
          onClick={() => setScriptOpen((v) => !v)}
        >
          <IconScript size={20} stroke={1.75} />
          Script
        </button>
        {active.role === "host" && (
          <button
            type="button"
            className={`rs-bar-item${inviteOpen ? " is-active" : ""}`}
            onClick={() => setInviteOpen((v) => !v)}
          >
            <IconUserPlus size={20} stroke={1.75} />
            Invite
          </button>
        )}
        <button type="button" className="rs-bar-item">
          <IconScreenShare size={20} stroke={1.75} />
          Share
        </button>
        <button type="button" className="rs-bar-item is-leave" onClick={() => void leave()}>
          <IconPhoneOff size={20} stroke={1.75} />
          Leave
        </button>
      </div>

      {countdown !== null && <div className="countdown">{countdown}</div>}

      {showStayModal && (
        <div className="modal-backdrop">
          <div className="modal">
            <h3>Stay until uploads finish</h3>
            <p style={{ color: "var(--text-muted)" }}>
              High-quality tracks are saving locally
              {settings.drive_connected ? " and uploading to Google Drive" : ""}.
            </p>
            <div className="level-meter" style={{ margin: "16px 0" }}>
              <span style={{ width: `${uploadFrac * 100}%` }} />
            </div>
            <button
              className="btn btn-primary"
              disabled={uploadFrac < 1}
              onClick={() => {
                setShowStayModal(false);
                void leave();
              }}
            >
              {uploadFrac < 1 ? "Uploading…" : "View recordings"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

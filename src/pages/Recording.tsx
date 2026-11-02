import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { useNavigate } from "react-router-dom";
import {
  IconCamera,
  IconCameraOff,
  IconCheck,
  IconLayoutGrid,
  IconMicrophone,
  IconMicrophoneOff,
  IconMinus,
  IconPhoneOff,
  IconPlayerPause,
  IconPlayerPlay,
  IconPlus,
  IconCopy,
  IconScreenShare,
  IconScreenShareOff,
  IconScript,
  IconSettings,
  IconUserPlus,
  IconUsers,
  IconVolume,
  IconVolumeOff,
  IconX,
} from "@tabler/icons-react";
import { api } from "../api";
import { LiveCameraPreview } from "../components/LiveCameraPreview";
import { Select } from "../components/Select";
import { TransferProgressBar } from "../components/TransferProgressBar";
import { pickPreferred, useDeviceList } from "../hooks/useDeviceList";
import { blobToUint8, useLocalMedia } from "../hooks/useLocalMedia";
import {
  useRemotePeerVideo,
  useStudioPreview,
} from "../hooks/useStudioPreview";
import {
  firstJoinOffsetsMs,
} from "../lib/presence";
import {
  computeTransferStats,
  pushSample,
  type TransferSample,
  type TransferStats,
} from "../lib/transferProgress";
import { useApp } from "../state";
import type { PeerInfo, PresenceEvent } from "../types";

function StageVuMeter({ level }: { level: number }) {
  const levelPct = Math.round(Math.min(1, Math.max(0, level)) * 100);
  return (
    <div className="rs-vumeter" aria-hidden>
      {Array.from({ length: 12 }).map((_, i) => (
        <span
          key={i}
          className={levelPct > (i + 1) * (100 / 12) ? "is-on" : ""}
        />
      ))}
    </div>
  );
}

function CamOffPlaceholder({ name }: { name: string }) {
  return (
    <div className="rs-cam-off" aria-label={`${name} camera off`}>
      <IconCameraOff size={28} stroke={1.5} aria-hidden />
      <span className="rs-cam-off-name">{name}</span>
    </div>
  );
}

function PeerVolumeControl({
  name,
  volume,
  onChange,
}: {
  name: string;
  volume: number;
  onChange: (v: number) => void;
}) {
  const muted = volume <= 0.001;
  return (
    <div className="rs-peer-vol">
      <button
        type="button"
        className={`rs-peer-vol-mute${muted ? " is-muted" : ""}`}
        title={muted ? `Unmute ${name}` : `Mute ${name} (local only)`}
        aria-label={muted ? `Unmute ${name}` : `Mute ${name}`}
        onClick={() => onChange(muted ? 1 : 0)}
      >
        {muted ? (
          <IconVolumeOff size={15} stroke={1.75} />
        ) : (
          <IconVolume size={15} stroke={1.75} />
        )}
      </button>
      <input
        type="range"
        className="rs-peer-vol-slider"
        min={0}
        max={2}
        step={0.05}
        value={volume}
        aria-label={`Volume for ${name} (local only)`}
        title="Local volume — only on your machine"
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span className="rs-peer-vol-pct">{Math.round(volume * 100)}%</span>
    </div>
  );
}

function RemotePeerTile({
  peer,
  waiting,
  admitDisabled,
  onAdmit,
}: {
  peer: {
    id: string;
    name: string;
    admitted: boolean;
  };
  waiting: boolean;
  admitDisabled?: boolean;
  onAdmit: () => void;
}) {
  const { imgRef, hasFrame, camOff, level } = useRemotePeerVideo(
    peer.id,
    peer.admitted,
  );
  const showCamOff = peer.admitted && !waiting && (!hasFrame || camOff);
  return (
    <div
      className={`rs-stage-tile${peer.admitted && hasFrame && !camOff ? "" : " rs-stage-empty"}${waiting ? " is-waiting" : ""}${showCamOff ? " is-cam-off" : ""}`}
    >
      <img
        ref={imgRef}
        className="rs-stage-preview rs-remote-preview"
        alt=""
        hidden={!hasFrame || camOff}
      />
      {showCamOff && <CamOffPlaceholder name={peer.name} />}
      <div className="rs-stage-name">{peer.name}</div>
      {peer.admitted && <StageVuMeter level={level} />}
      {waiting && (
        <div className="rs-stage-waiting">
          <span>Waiting in lobby</span>
          <button
            type="button"
            className="rs-invite-copy"
            disabled={admitDisabled}
            title={
              admitDisabled
                ? `Studio is full (max ${MAX_GUESTS} guests)`
                : "Admit guest"
            }
            onClick={onAdmit}
          >
            {admitDisabled ? "Studio full" : "Admit"}
          </button>
        </div>
      )}
    </div>
  );
}

type Layout = "auto" | "grid" | "speaker";
type SideTab = "people" | "layout";

/** 1 host + 3 guests — must match src-tauri MAX_GUESTS. */
const MAX_GUESTS = 3;

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
  const { studio, settings, setStudio, refreshSessions, persistSettings } =
    useApp();
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
  const [peopleOpen, setPeopleOpen] = useState(false);
  const [micOn, setMicOn] = useState(true);
  const [camOn, setCamOn] = useState(true);
  const [layout, setLayout] = useState<Layout>("auto");
  const [recording, setRecording] = useState(false);
  const [hasRecording, setHasRecording] = useState(false);
  const [countdown, setCountdown] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [uploadFrac, setUploadFrac] = useState(0);
  const [showStayModal, setShowStayModal] = useState(false);
  const [transferStatus, setTransferStatus] = useState<
    "idle" | "saving" | "sending" | "done" | "failed"
  >("idle");
  const [transferMsg, setTransferMsg] = useState("");
  const [resumingTransfer, setResumingTransfer] = useState(false);
  const [xferStats, setXferStats] = useState<TransferStats>({
    bytes: 0,
    total: 1,
    pct: 0,
    bytesPerSec: 0,
    etaSec: null,
  });
  const [xferFile, setXferFile] = useState<string | null>(null);
  const xferSamplesRef = useRef<TransferSample[]>([]);
  const xferListeningRef = useRef(false);
  /** Host: per-file receive progress while collecting guest tracks. */
  const hostRecvFilesRef = useRef<Map<string, { bytes: number; total: number }>>(
    new Map(),
  );
  const hostRecvStartedAt = useRef(0);
  const [recvElapsed, setRecvElapsed] = useState(0);
  const [hostRecvActive, setHostRecvActive] = useState(false);
  const [sharing, setSharing] = useState(false);
  const [speakerOpen, setSpeakerOpen] = useState(false);
  /** Local-only listen levels for remote peers (does not change their recording). */
  const [peerVolumes, setPeerVolumes] = useState<Record<string, number>>({});
  const [echo, setEcho] = useState(settings.echo_cancellation);
  const [audioDevice, setAudioDevice] = useState(settings.preferred_mic_id ?? "");
  const [videoDevice, setVideoDevice] = useState(settings.preferred_camera_id ?? "");
  const [speakerDevice, setSpeakerDevice] = useState(settings.preferred_speaker_id ?? "");
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [renamingTitle, setRenamingTitle] = useState(false);
  const [prompterOpen, setPrompterOpen] = useState(false);
  const [prompterEditing, setPrompterEditing] = useState(true);
  const [prompterPlaying, setPrompterPlaying] = useState(false);
  const [prompterSpeed, setPrompterSpeed] = useState(1);
  const [prompterSize, setPrompterSize] = useState(2);
  const [prompterScript, setPrompterScript] = useState(() => {
    try {
      return localStorage.getItem("openpodcast.teleprompter") ?? "";
    } catch {
      return "";
    }
  });
  const [prompterPos, setPrompterPos] = useState({ x: 18, y: 18 });
  const [prompterDragging, setPrompterDragging] = useState(false);
  const startedAt = useRef<number>(0);
  const timerRef = useRef<number>(0);
  const countdownTimerRef = useRef<number>(0);
  const recordingRef = useRef(false);
  /** Host session id from RecordStart — used when enqueueing guest track return. */
  const hostSessionIdRef = useRef<string | null>(null);
  /** Prevents the studio-null effect from bouncing to `/` during intentional leave. */
  const leavingRef = useRef(false);
  const mediaBooted = useRef(false);
  const titleInputRef = useRef<HTMLInputElement | null>(null);
  const screenStreamRef = useRef<MediaStream | null>(null);
  const speakerMenuRef = useRef<HTMLDivElement | null>(null);
  const prompterScrollRef = useRef<HTMLDivElement | null>(null);
  const prompterRef = useRef<HTMLElement | null>(null);
  const prompterDragRef = useRef<{
    pointerId: number;
    offsetX: number;
    offsetY: number;
  } | null>(null);
  /** Everyone who appeared in the lobby / studio during this session */
  const rosterRef = useRef<Map<string, PeerInfo>>(new Map());
  /** Join/leave on the take timeline (host only). */
  const presenceEventsRef = useRef<PresenceEvent[]>([]);
  const presentIdsRef = useRef<Set<string>>(new Set());

  const getVideoEl = useCallback(() => {
    // LiveCameraPreview registers the hidden <video> through media.setVideoEl
    return (
      document.querySelector(
        ".rs-stage-tile .rs-live-video-hidden",
      ) as HTMLVideoElement | null
    );
  }, []);
  const getStream = useCallback(() => media.stream.current, [media.stream]);

  const hasAdmittedGuest = peers.some((p) => p.admitted);
  useStudioPreview({
    active: !!studio,
    role: studio?.role === "guest" ? "guest" : "host",
    hostHasAdmittedGuest: hasAdmittedGuest,
    participantId: studio?.participantId ?? "host",
    getVideoEl,
    getStream,
    camOn: camOn && !!videoMode,
    micOn,
    peerVolumes,
  });

  function peerVolume(id: string) {
    const v = peerVolumes[id];
    return v == null ? 1 : v;
  }

  function setPeerVolume(id: string, volume: number) {
    const next = Math.max(0, Math.min(2, volume));
    setPeerVolumes((prev) =>
      prev[id] === next ? prev : { ...prev, [id]: next },
    );
  }

  useEffect(() => {
    // Only bounce home for unexpected studio loss — leave() navigates itself.
    if (!studio && !leavingRef.current) {
      navigate("/", { replace: true });
    }
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
    let cancelled = false;
    const refreshPeers = async () => {
      try {
        const list = await api.listPeers();
        if (cancelled) return;
        setPeers(list);
        for (const p of list) {
          rosterRef.current.set(p.id, {
            ...p,
            role: p.role || "Guest",
          });
        }
        if (list.some((p) => !p.admitted)) {
          setSideTab("people");
          setPeopleOpen(true);
        }
        // Presence: detect mid-take joins/leaves from the live roster.
        if (recordingRef.current) {
          const admitted = new Set(
            list.filter((p) => p.admitted).map((p) => p.id),
          );
          for (const id of [...presentIdsRef.current]) {
            if (id === studio.participantId) continue;
            if (!admitted.has(id)) {
              const meta = rosterRef.current.get(id);
              pushPresence(id, "leave", meta?.name);
            }
          }
          for (const p of list) {
            if (!p.admitted) continue;
            if (p.id === studio.participantId) continue;
            if (!presentIdsRef.current.has(p.id)) {
              pushPresence(p.id, "join", p.name);
            }
          }
        }
      } catch {
        /* ignore */
      }
    };
    void refreshPeers();
    const id = window.setInterval(() => void refreshPeers(), 800);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [studio]);

  // Guests learn the full roster (host + every guest) over the studio wire.
  useEffect(() => {
    if (!studio || studio.role !== "guest") return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        unlisten = await listen<{ peers: PeerInfo[] }>("studio-roster", (ev) => {
          const list = ev.payload?.peers ?? [];
          setPeers(list);
          for (const p of list) {
            rosterRef.current.set(p.id, {
              ...p,
              role: p.role || (p.id === "host" ? "Host" : "Guest"),
            });
          }
        });
      } catch {
        /* browser preview */
      }
    })();
    return () => {
      unlisten?.();
    };
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
    await persistSettings({
      preferred_mic_id: next.mic ?? (audioDevice || null),
      preferred_camera_id: next.camera ?? (videoDevice || null),
      preferred_speaker_id: next.speaker ?? (speakerDevice || null),
    });
  }

  const waitingPeers = useMemo(
    () => (studio?.role === "host" ? peers.filter((p) => !p.admitted) : []),
    [studio, peers],
  );

  const admittedGuestCount = useMemo(
    () => peers.filter((p) => p.id !== "host" && p.admitted).length,
    [peers],
  );
  const studioFull = admittedGuestCount >= MAX_GUESTS;

  function pushPresence(
    participantId: string,
    kind: "join" | "leave",
    name?: string,
  ) {
    if (!recordingRef.current || !startedAt.current) return;
    const t_ms = Math.max(0, Date.now() - startedAt.current);
    const last = presenceEventsRef.current[presenceEventsRef.current.length - 1];
    if (
      last &&
      last.participant_id === participantId &&
      last.kind === kind &&
      t_ms - last.t_ms < 250
    ) {
      return;
    }
    presenceEventsRef.current.push({
      participant_id: participantId,
      kind,
      t_ms,
      name,
    });
    if (kind === "join") presentIdsRef.current.add(participantId);
    else presentIdsRef.current.delete(participantId);
  }

  async function admitGuest(id: string) {
    try {
      await api.admitGuest(id);
      const list = await api.listPeers();
      setPeers(list);
      const guest = list.find((p) => p.id === id);
      if (recordingRef.current && guest?.admitted) {
        pushPresence(id, "join", guest.name);
      }
    } catch (e) {
      alert(String(e).replace(/^Error:\s*/i, ""));
    }
  }

  const people = useMemo(() => {
    const selfId = studio?.participantId ?? "host";
    const self = {
      id: selfId,
      name: `${studio?.displayName ?? "You"} (You)`,
      role: studio?.role === "host" ? "Host" : "Guest",
      resolution: studio?.settings.resolution ?? "1080p",
      admitted: true,
    };
    if (studio?.role === "guest") {
      // Everyone in the call: host + admitted guests (except self).
      const remotes = peers
        .filter((p) => p.id !== selfId && (p.admitted || p.id === "host"))
        .map((p) => ({
          id: p.id,
          name:
            p.id === "host"
              ? p.name || studio.hostName || "Host"
              : p.name,
          role: p.id === "host" ? "Host" : "Guest",
          resolution: studio.settings.resolution ?? "1080p",
          admitted: true,
        }));
      // Fallback before first roster arrives.
      if (!remotes.length) {
        remotes.push({
          id: "host",
          name: studio.hostName || "Host",
          role: "Host",
          resolution: studio.settings.resolution ?? "1080p",
          admitted: true,
        });
      }
      return [self, ...remotes];
    }
    // Host stage includes waiting guests so Admit is visible without hunting.
    return [
      self,
      ...peers
        .filter((p) => p.id !== "host")
        .map((p) => ({
          id: p.id,
          name: p.name,
          role: p.admitted ? "Guest" : "Waiting",
          resolution: studio?.settings.resolution ?? "1080p",
          admitted: p.admitted,
        })),
    ];
  }, [studio, peers]);

  useEffect(() => {
    if (!editingTitle) return;
    const el = titleInputRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, [editingTitle]);

  useEffect(() => {
    if (!speakerOpen) return;
    const onDoc = (e: MouseEvent) => {
      if (!speakerMenuRef.current?.contains(e.target as Node)) {
        setSpeakerOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSpeakerOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [speakerOpen]);

  useEffect(() => {
    if (!speakerDevice) return;
    void applySpeakerOutput(speakerDevice);
  }, [speakerDevice]);

  useEffect(() => {
    try {
      localStorage.setItem("openpodcast.teleprompter", prompterScript);
    } catch {
      /* ignore quota / private mode */
    }
  }, [prompterScript]);

  // CSS transform scroll (compositor) so disk/IPC jank doesn't stall the teleprompter.
  useEffect(() => {
    if (!prompterOpen || !prompterPlaying || prompterEditing) return;
    const viewport = prompterScrollRef.current;
    const mover = viewport?.querySelector(
      ".rs-prompter-mover",
    ) as HTMLElement | null;
    if (!viewport || !mover) return;

    const startY = viewport.scrollTop;
    const distance = Math.max(0, mover.scrollHeight - viewport.clientHeight);
    const remaining = Math.max(0, distance - startY);
    if (remaining < 4) {
      setPrompterPlaying(false);
      return;
    }
    viewport.scrollTop = 0;
    // Match prior rAF feel: ~0.35 + speed*0.45 px per frame at 60fps.
    const pxPerSec = (0.35 + prompterSpeed * 0.45) * 60;
    const durationSec = remaining / pxPerSec;
    mover.style.setProperty("--prompter-from", `${-startY}px`);
    mover.style.setProperty("--prompter-distance", `${distance}px`);
    mover.style.setProperty("--prompter-duration", `${durationSec}s`);
    mover.classList.add("is-scrolling");

    const onEnd = () => setPrompterPlaying(false);
    mover.addEventListener("animationend", onEnd);
    return () => {
      mover.removeEventListener("animationend", onEnd);
      const style = getComputedStyle(mover);
      const matrix = style.transform;
      let y = -startY;
      if (matrix && matrix !== "none") {
        const parts = matrix.match(/matrix\((.+)\)/);
        if (parts) {
          const nums = parts[1].split(",").map((v) => parseFloat(v.trim()));
          y = nums[5] ?? y;
        } else {
          const t3 = matrix.match(/matrix3d\((.+)\)/);
          if (t3) {
            const nums = t3[1].split(",").map((v) => parseFloat(v.trim()));
            y = nums[13] ?? y;
          }
        }
      }
      mover.classList.remove("is-scrolling");
      mover.style.removeProperty("--prompter-from");
      mover.style.removeProperty("--prompter-distance");
      mover.style.removeProperty("--prompter-duration");
      mover.style.transform = "";
      // Freeze via scrollTop so pause/resume continues from here.
      viewport.scrollTop = Math.min(distance, Math.max(0, -y));
    };
  }, [prompterOpen, prompterPlaying, prompterEditing, prompterSpeed]);

  useEffect(() => {
    if (!prompterOpen) setPrompterPlaying(false);
  }, [prompterOpen]);

  function clampPrompterPos(x: number, y: number) {
    const panel = prompterRef.current;
    const parent = panel?.offsetParent as HTMLElement | null;
    if (!panel || !parent) return { x, y };
    const maxX = Math.max(8, parent.clientWidth - panel.offsetWidth - 8);
    const maxY = Math.max(8, parent.clientHeight - panel.offsetHeight - 8);
    return {
      x: Math.min(Math.max(8, x), maxX),
      y: Math.min(Math.max(8, y), maxY),
    };
  }

  function onPrompterPointerDown(e: ReactPointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("button")) return;
    const panel = prompterRef.current;
    if (!panel) return;
    e.preventDefault();
    panel.setPointerCapture(e.pointerId);
    prompterDragRef.current = {
      pointerId: e.pointerId,
      offsetX: e.clientX - panel.getBoundingClientRect().left,
      offsetY: e.clientY - panel.getBoundingClientRect().top,
    };
    setPrompterDragging(true);
  }

  function onPrompterPointerMove(e: ReactPointerEvent<HTMLElement>) {
    const drag = prompterDragRef.current;
    const panel = prompterRef.current;
    const parent = panel?.offsetParent as HTMLElement | null;
    if (!drag || drag.pointerId !== e.pointerId || !panel || !parent) return;
    const parentRect = parent.getBoundingClientRect();
    const next = clampPrompterPos(
      e.clientX - parentRect.left - drag.offsetX,
      e.clientY - parentRect.top - drag.offsetY,
    );
    setPrompterPos(next);
  }

  function onPrompterPointerUp(e: ReactPointerEvent<HTMLElement>) {
    const drag = prompterDragRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    prompterDragRef.current = null;
    setPrompterDragging(false);
    try {
      prompterRef.current?.releasePointerCapture(e.pointerId);
    } catch {
      /* already released */
    }
  }

  if (!studio) {
    return (
      <div className="rs-studio rs-studio-leaving" aria-busy>
        <p>{leavingRef.current ? "Opening recordings…" : "Leaving studio…"}</p>
      </div>
    );
  }
  const active = studio;

  async function copyInvite() {
    try {
      // Always refresh from the host endpoint so the invite includes a live relay URL
      // and the host's current display name.
      const invite = await api.getInvite(active.displayName);
      setStudio({ ...active, invite });
      await navigator.clipboard.writeText(invite);
      setInviteCopied(true);
      setTimeout(() => setInviteCopied(false), 2000);
    } catch (e) {
      alert(String(e));
    }
  }

  async function applySpeakerOutput(deviceId: string) {
    const nodes = Array.from(document.querySelectorAll("audio, video"));
    await Promise.all(
      nodes.map(async (node) => {
        const el = node as HTMLMediaElement & {
          setSinkId?: (id: string) => Promise<void>;
        };
        if (typeof el.setSinkId !== "function") return;
        try {
          await el.setSinkId(deviceId);
        } catch {
          /* unsupported / locked device */
        }
      }),
    );
  }

  async function selectSpeaker(deviceId: string) {
    setSpeakerDevice(deviceId);
    setSpeakerOpen(false);
    await applySpeakerOutput(deviceId);
    await saveDevicePrefs({ speaker: deviceId });
  }

  async function stopScreenShare(restoreCamera = true) {
    const screen = screenStreamRef.current;
    screenStreamRef.current = null;
    screen?.getTracks().forEach((t) => t.stop());
    setSharing(false);
    if (!restoreCamera || !studio) return;
    try {
      await media.start({
        audio: audioDevice || undefined,
        video: videoMode ? videoDevice || undefined : undefined,
      });
      media.toggleTrack("audio", micOn);
      media.toggleTrack("video", camOn && videoMode);
    } catch (e) {
      console.warn("Failed to restore camera after screen share", e);
    }
  }

  async function toggleScreenShare() {
    if (sharing) {
      await stopScreenShare(true);
      return;
    }
    try {
      const display = await navigator.mediaDevices.getDisplayMedia({
        video: {
          frameRate: active.settings.frame_rate || 30,
        },
        audio: false,
      });
      const screenTrack = display.getVideoTracks()[0];
      if (!screenTrack) {
        display.getTracks().forEach((t) => t.stop());
        return;
      }

      const stream = media.stream.current;
      if (!stream) {
        display.getTracks().forEach((t) => t.stop());
        alert("Start your camera or mic before sharing your screen.");
        return;
      }

      // Swap camera video for the screen track (preview + recorder share this stream)
      stream.getVideoTracks().forEach((t) => {
        stream.removeTrack(t);
        t.stop();
      });
      stream.addTrack(screenTrack);
      screenStreamRef.current = display;
      setSharing(true);
      setCamOn(true);

      screenTrack.onended = () => {
        void stopScreenShare(true);
      };
    } catch (e) {
      // User cancelled the picker — stay quiet
      if (String(e).includes("NotAllowedError") || String(e).includes("AbortError")) {
        return;
      }
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
      const invite = await api
        .getInvite(nextStudio.displayName)
        .catch(() => undefined);
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

  function runCountdown(seconds: number, onDone?: () => void) {
    if (countdownTimerRef.current) {
      clearInterval(countdownTimerRef.current);
      countdownTimerRef.current = 0;
    }
    let n = Math.max(1, Math.floor(seconds));
    setCountdown(n);
    countdownTimerRef.current = window.setInterval(() => {
      n -= 1;
      if (n <= 0) {
        clearInterval(countdownTimerRef.current);
        countdownTimerRef.current = 0;
        setCountdown(null);
        onDone?.();
      } else {
        setCountdown(n);
      }
    }, 1000);
  }

  function startCountdown() {
    if (active.role !== "host") return;
    void api.signalRecordCountdown(5).catch((e) => {
      console.error("signalRecordCountdown failed", e);
    });
    runCountdown(5, () => {
      void beginRecord();
    });
  }

  async function beginRecord() {
    if (!active || recordingRef.current) return;
    recordingRef.current = true;
    setRecording(true);
    startedAt.current = Date.now();
    presenceEventsRef.current = [];
    presentIdsRef.current = new Set();
    timerRef.current = window.setInterval(() => {
      setElapsed(Math.floor((Date.now() - startedAt.current) / 1000));
    }, 250);

    // Host: seed presence for everyone already in the take, then signal guests.
    if (active.role === "host") {
      pushPresence(
        active.participantId,
        "join",
        (active.displayName || "Host").replace(" (You)", ""),
      );
      for (const p of peers) {
        if (p.admitted && p.id !== active.participantId) {
          pushPresence(p.id, "join", p.name);
        }
      }
      try {
        await api.signalRecordStart(active.sessionId);
      } catch (e) {
        console.error("signalRecordStart failed", e);
      }
    }

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
      const ext = media.pickRecorderExtension();
      await api.beginVideoTrack(
        active.sessionId,
        active.participantId,
        ext,
      );
      media.startRecording({
        // Longer slices = fewer IPC copies on the UI thread (teleprompter stays smooth).
        timesliceMs: 4000,
        onChunk: async (chunk) => {
          // Yield so rAF / CSS work can run before we copy a multi‑MB buffer.
          await new Promise<void>((r) => setTimeout(r, 0));
          const bytes = await blobToUint8(chunk);
          if (!bytes.byteLength) return;
          await api.appendVideoChunk(
            active.sessionId,
            active.participantId,
            bytes,
            ext,
          );
        },
      });
    } catch (e) {
      console.error("Failed to start streamed video recording", e);
      try {
        media.startRecording();
      } catch {
        /* ignore */
      }
    }
  }

  async function flushLocalTracks(onProgress?: (msg: string) => void) {
    onProgress?.("Stopping microphone capture…");
    try {
      await Promise.race([
        api.stopAudioRecording(active!.participantId),
        new Promise<never>((_, rej) =>
          setTimeout(() => rej(new Error("Audio stop timed out")), 12_000),
        ),
      ]);
    } catch (e) {
      console.warn("stopAudioRecording", e);
    }
    onProgress?.("Finalizing camera recording…");
    try {
      const blob = await media.stopRecording();
      if (blob && blob.size > 0) {
        onProgress?.("Writing video file…");
        const bytes = await blobToUint8(blob);
        const ext = blob.type.includes("mp4") ? "mp4" : "webm";
        await api.saveVideoTrack(
          active!.sessionId,
          active!.participantId,
          bytes,
          ext,
        );
      } else {
        await api
          .finishVideoTrack(active!.sessionId, active!.participantId)
          .catch(() => undefined);
      }
    } catch (e) {
      console.error("Video finalize failed", e);
      try {
        await api.finishVideoTrack(active!.sessionId, active!.participantId);
      } catch {
        /* ignore */
      }
      throw e;
    }
  }

  async function waitForGuestVideos(guestIds: string[], timeoutMs: number) {
    if (!guestIds.length) return;
    const pending = new Set(guestIds);
    hostRecvFilesRef.current = new Map();
    xferSamplesRef.current = [];
    hostRecvStartedAt.current = Date.now();
    setRecvElapsed(0);
    setHostRecvActive(true);
    setXferFile(null);
    setXferStats({
      bytes: 0,
      total: 1,
      pct: 0,
      bytesPerSec: 0,
      etaSec: null,
    });

    const tick = window.setInterval(() => {
      if (!hostRecvStartedAt.current) return;
      setRecvElapsed(
        Math.floor((Date.now() - hostRecvStartedAt.current) / 1000),
      );
    }, 250);

    const { listen } = await import("@tauri-apps/api/event");
    await new Promise<void>((resolve) => {
      let unlisten: (() => void) | undefined;
      const timer = window.setTimeout(() => {
        unlisten?.();
        resolve();
      }, timeoutMs);
      void listen<{
        participant_id: string;
        file_name: string;
        status: string;
        bytes_received: number;
        total_bytes: number;
        overall_bytes?: number;
        overall_total?: number;
      }>("studio-transfer", (ev) => {
        const p = ev.payload;
        if (!p?.participant_id || !p.file_name) return;

        const key = `${p.participant_id}:${p.file_name}`;
        const total = Math.max(
          1,
          p.overall_total || p.total_bytes || 1,
        );
        const bytes = Math.min(
          total,
          p.overall_bytes || p.bytes_received || 0,
        );
        hostRecvFilesRef.current.set(key, { bytes, total });

        let sumBytes = 0;
        let sumTotal = 0;
        for (const f of hostRecvFilesRef.current.values()) {
          sumBytes += f.bytes;
          sumTotal += f.total;
        }
        sumTotal = Math.max(1, sumTotal);
        xferSamplesRef.current = pushSample(
          xferSamplesRef.current,
          sumBytes,
          sumTotal,
        );
        const stats = computeTransferStats(xferSamplesRef.current);
        setXferStats(stats);
        setXferFile(p.file_name || null);
        setUploadFrac(0.55 + 0.4 * (stats.pct / 100));

        const guestsDone = guestIds.length - pending.size;
        setTransferMsg(
          pending.size
            ? `Receiving guest videos… ${guestsDone}/${guestIds.length} finished · ${p.file_name}`
            : "All guest videos received.",
        );

        if (
          p.status === "done" &&
          p.file_name.includes("-video.") &&
          pending.has(p.participant_id)
        ) {
          pending.delete(p.participant_id);
          if (pending.size === 0) {
            window.clearTimeout(timer);
            unlisten?.();
            resolve();
          }
        }
      }).then((fn) => {
        unlisten = fn;
      });
    });

    window.clearInterval(tick);
    setRecvElapsed(
      Math.floor((Date.now() - hostRecvStartedAt.current) / 1000),
    );
    setXferStats((s) => ({
      ...s,
      bytes: Math.max(s.bytes, s.total),
      total: Math.max(s.total, 1),
      pct: 100,
      etaSec: 0,
    }));
    setHostRecvActive(false);
  }

  async function sendTracksToHost() {
    if (!active) return;
    const hostSid = hostSessionIdRef.current || active.sessionId;
    const invite = active.invite || "";
    setTransferStatus("sending");
    setTransferMsg("Connecting to host and sending full-quality tracks…");
    setResumingTransfer(true);

    // Seed progress from any prior resume offsets so the bar doesn't reset to 0%.
    let seedBytes = 0;
    let seedTotal = 1;
    try {
      const queued = await api.enqueueTrackReturn(
        active.sessionId,
        hostSid,
        active.participantId,
        invite,
      );
      const pending = (await api.listPendingTransfers()).find(
        (p) => p.id === queued.id,
      );
      if (pending) {
        const sizes = pending.file_sizes ?? {};
        const offsets = pending.offsets ?? {};
        const names = pending.file_names?.length
          ? pending.file_names
          : Object.keys(sizes);
        seedTotal = 0;
        seedBytes = 0;
        for (const name of names) {
          const size = sizes[name] ?? 0;
          const off = offsets[name] ?? 0;
          seedTotal += size;
          seedBytes += size > 0 ? Math.min(off, size) : off;
        }
        seedTotal = Math.max(1, seedTotal || seedBytes || 1);
        seedBytes = Math.min(seedTotal, seedBytes);
      }
    } catch {
      /* enqueue optional if files missing */
    }

    xferSamplesRef.current =
      seedBytes > 0
        ? [{ bytes: seedBytes, total: seedTotal, at: Date.now() }]
        : [];
    setXferFile(
      seedBytes > 0
        ? `Resuming from ${Math.round((seedBytes / seedTotal) * 100)}%…`
        : null,
    );
    setXferStats({
      bytes: seedBytes,
      total: seedTotal,
      pct: Math.min(100, (seedBytes / seedTotal) * 100),
      bytesPerSec: 0,
      etaSec: null,
    });
    setUploadFrac(Math.max(0.05, seedBytes / seedTotal));

    let unlisten: (() => void) | undefined;
    xferListeningRef.current = true;
    try {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<{
        file_name: string;
        status: string;
        bytes_received: number;
        total_bytes: number;
        overall_bytes?: number;
        overall_total?: number;
      }>("studio-transfer", (ev) => {
        if (!xferListeningRef.current || !ev.payload) return;
        const p = ev.payload;
        if (typeof p.status === "string" && p.status.startsWith("retry:")) {
          const n = p.status.slice("retry:".length);
          setTransferMsg(
            `Host blip — reconnecting (attempt ${n}/3). Progress is saved.`,
          );
          return;
        }
        const total = Math.max(1, p.overall_total || p.total_bytes || 1);
        const bytes = Math.min(total, p.overall_bytes || p.bytes_received || 0);
        xferSamplesRef.current = pushSample(
          xferSamplesRef.current,
          bytes,
          total,
        );
        const stats = computeTransferStats(xferSamplesRef.current);
        setXferStats(stats);
        setXferFile(p.file_name || null);
        setUploadFrac(Math.max(0.05, stats.pct / 100));
        if (stats.bytes > 0) {
          setTransferMsg(
            p.file_name
              ? `Sending ${p.file_name}…`
              : "Sending your high-quality video to the host…",
          );
        }
      });
    } catch {
      /* browser preview */
    }

    try {
      await api.returnLocalTracks(
        active.sessionId,
        active.participantId,
        invite,
        hostSid,
      );
      setUploadFrac(1);
      setXferStats((s) => ({
        ...s,
        bytes: s.total,
        pct: 100,
        etaSec: 0,
      }));
      setTransferStatus("done");
      setTransferMsg("Host has your full-quality tracks.");
    } catch (e) {
      console.error("Guest track return failed", e);
      setTransferStatus("failed");
      setTransferMsg(
        String(e).replace(/^Error:\s*/i, "") ||
          "Could not reach the host. Your files are safe here — resume from Recordings anytime.",
      );
    } finally {
      xferListeningRef.current = false;
      unlisten?.();
      setResumingTransfer(false);
    }
  }

  /** Cut live cam/mic/share the moment the take ends — HQ file save continues after. */
  function muteMeetingAV() {
    setMicOn(false);
    setCamOn(false);
    media.toggleTrack("audio", false);
    media.toggleTrack("video", false);
    if (sharing) {
      void stopScreenShare(false);
    }
  }

  async function stopGuestRecord() {
    if (!active || active.role !== "guest") return;
    recordingRef.current = false;
    setRecording(false);
    clearInterval(timerRef.current);
    const duration = (Date.now() - startedAt.current) / 1000;
    muteMeetingAV();
    setShowStayModal(true);
    setTransferStatus("saving");
    setTransferMsg("Saving your full-quality recording on this computer…");
    setUploadFrac(0.15);
    try {
      await flushLocalTracks((msg) => {
        setTransferMsg(msg);
        setUploadFrac((f) => Math.min(0.4, f + 0.08));
      });
      setUploadFrac(0.4);
      setTransferMsg("Saving session metadata…");
      try {
        await api.finalizeSession(
          active.sessionId,
          duration,
          [
            {
              id: active.participantId,
              name: (active.displayName || "Guest").replace(" (You)", ""),
              role: "Guest",
              resolution: active.settings.resolution,
            },
          ],
          {},
        );
      } catch (e) {
        console.warn("guest finalizeSession", e);
      }
      setHasRecording(true);
      setUploadFrac(0.5);
      setTransferMsg("Saved locally — sending to host…");
      await refreshSessions().catch(() => undefined);
      await sendTracksToHost();
    } catch (e) {
      console.error("Guest stop failed", e);
      setHasRecording(true);
      setTransferStatus("failed");
      setTransferMsg(
        `Could not finish saving locally: ${String(e).replace(/^Error:\s*/i, "")}. You can still try Send to host if files were written.`,
      );
      setUploadFrac(0.4);
    }
  }

  async function stopRecord() {
    if (!active) return;
    if (active.role === "guest") {
      await stopGuestRecord();
      return;
    }

    recordingRef.current = false;
    setRecording(false);
    clearInterval(timerRef.current);
    const duration = (Date.now() - startedAt.current) / 1000;

    // Snapshot thumbnail while the camera is still hot, then cut live A/V.
    try {
      const thumb = await media.grabJpegFrame();
      if (thumb?.length) {
        await api.saveSessionThumbnail(active.sessionId, thumb);
      }
    } catch {
      /* optional */
    }
    muteMeetingAV();

    setShowStayModal(true);
    setTransferStatus("saving");
    setTransferMsg(
      "Meeting ended. Saving host tracks and collecting guest videos — keep this window open until sends finish.",
    );
    setUploadFrac(0.1);

    let guestIds: string[] = [];
    try {
      guestIds = await api.signalRecordStop();
    } catch (e) {
      console.error("signalRecordStop failed", e);
    }

    try {
      await flushLocalTracks((msg) => {
        setTransferMsg(msg);
        setUploadFrac((f) => Math.min(0.5, f + 0.08));
      });
    } catch (e) {
      alert(
        `Recording could not finish saving video: ${String(e)}\n\nAudio may still be available.`,
      );
    }

    setUploadFrac(0.55);
    setTransferStatus("sending");
    setTransferMsg(
      guestIds.length
        ? `Waiting for ${guestIds.length} guest upload${guestIds.length === 1 ? "" : "s"} (up to ~90s now; they can resume later while you stay in studio)…`
        : "No admitted guests — finishing host recording…",
    );
    // Don't block for multi-hour uploads — finalize with what we have;
    // inbox stays open until Leave so late/resumed guest transfers still land.
    if (guestIds.length) {
      await waitForGuestVideos(guestIds, 90 * 1000);
    }

    const byId = new Map<
      string,
      { id: string; name: string; role: string; resolution: string }
    >();
    byId.set(active.participantId, {
      id: active.participantId,
      name: (active.displayName || "Host").replace(" (You)", ""),
      role: "Host",
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
    const presenceEvents = [...presenceEventsRef.current];
    const padOffsets = firstJoinOffsetsMs(presenceEvents);
    await api.finalizeSession(
      active.sessionId,
      duration,
      participants,
      padOffsets,
      presenceEvents,
    );
    setHasRecording(true);
    setUploadFrac(1);
    setTransferStatus("done");
    setTransferMsg(
      guestIds.length
        ? "Episode saved. Keep the studio open if a guest still needs to resume sending."
        : "Episode saved on this computer.",
    );
    await refreshSessions();
  }

  // Guests mirror host countdown, then start/stop HQ local recording with the host.
  useEffect(() => {
    if (!studio || studio.role !== "guest") return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        unlisten = await listen<{
          kind?: string;
          session_id?: string;
          seconds?: number;
        }>("studio-record", (ev) => {
          const kind = ev.payload?.kind;
          if (kind === "countdown" && !recordingRef.current) {
            const seconds = ev.payload?.seconds ?? 5;
            runCountdown(seconds);
          } else if (kind === "start" && !recordingRef.current) {
            if (countdownTimerRef.current) {
              clearInterval(countdownTimerRef.current);
              countdownTimerRef.current = 0;
            }
            setCountdown(null);
            if (ev.payload?.session_id) {
              hostSessionIdRef.current = ev.payload.session_id;
            }
            void beginRecord();
          } else if (kind === "stop" && recordingRef.current) {
            void stopGuestRecord();
          }
        });
      } catch {
        /* browser preview */
      }
    })();
    return () => {
      unlisten?.();
      if (countdownTimerRef.current) {
        clearInterval(countdownTimerRef.current);
        countdownTimerRef.current = 0;
      }
    };
  }, [studio]); // eslint-disable-line react-hooks/exhaustive-deps

  async function leave() {
    if (leavingRef.current) return;
    leavingRef.current = true;
    let saved = hasRecording;
    if (recording) {
      await stopRecord();
      saved = true;
    }
    await stopScreenShare(false);
    await api.leaveStudio().catch(() => undefined);
    const id = active.sessionId;
    if (!saved) {
      // Never pressed Record — discard preview session so it never appears in Recordings
      await api.discardSession(id).catch(() => undefined);
    }
    await refreshSessions().catch(() => undefined);
    const dest = saved ? "/recordings" : "/";
    // Navigate first so the studio-null effect cannot replace us back to `/`.
    navigate(dest, { replace: true });
    setStudio(null);
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
            <div
              className={`rs-stage-tile${
                (videoMode && camOn) || sharing
                  ? ""
                  : " rs-stage-empty is-cam-off"
              }`}
            >
              {/* Keep preview mounted while cam is off so capture can resume cleanly */}
              {videoMode || sharing ? (
                <div
                  className="rs-stage-preview-host"
                  hidden={!(camOn || sharing)}
                >
                  <LiveCameraPreview
                    className="rs-stage-preview"
                    setVideoEl={media.setVideoEl}
                    mirror={!sharing}
                    maxFps={recording ? 12 : 24}
                  />
                </div>
              ) : null}
              {!(videoMode && camOn) && !sharing && (
                <CamOffPlaceholder name={active.displayName} />
              )}
              <div className="rs-stage-name">
                {sharing ? "Screen share" : active.displayName}
              </div>
              <StageVuMeter level={media.level} />
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
                    Share this invite so guests can join from any network. Both
                    of you must stay online. Guests paste it in the lobby and enter the
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
              <RemotePeerTile
                key={p.id}
                peer={p}
                waiting={!p.admitted}
                admitDisabled={studioFull}
                onAdmit={() => void admitGuest(p.id)}
              />
            ))}
          </div>

          {prompterOpen && (
            <aside
              ref={prompterRef}
              className={`rs-prompter${prompterDragging ? " is-dragging" : ""}`}
              aria-label="Teleprompter"
              style={{ left: prompterPos.x, top: prompterPos.y }}
              onPointerMove={onPrompterPointerMove}
              onPointerUp={onPrompterPointerUp}
              onPointerCancel={onPrompterPointerUp}
            >
              <div
                className="rs-prompter-head"
                onPointerDown={onPrompterPointerDown}
              >
                <strong>Teleprompter</strong>
                <div className="rs-prompter-head-actions">
                  <button
                    type="button"
                    className={!prompterEditing ? "is-active" : ""}
                    onClick={() => {
                      setPrompterPlaying(false);
                      setPrompterEditing((v) => {
                        const next = !v;
                        if (!next) {
                          requestAnimationFrame(() => {
                            const el = prompterScrollRef.current;
                            if (el) el.scrollTop = 0;
                          });
                        }
                        return next;
                      });
                    }}
                  >
                    {prompterEditing ? "Read" : "Edit"}
                  </button>
                  <button
                    type="button"
                    aria-label="Close teleprompter"
                    onClick={() => setPrompterOpen(false)}
                  >
                    <IconX size={16} stroke={2} />
                  </button>
                </div>
              </div>

              {prompterEditing ? (
                <textarea
                  className="rs-prompter-editor"
                  value={prompterScript}
                  onChange={(e) => setPrompterScript(e.target.value)}
                  placeholder="Paste your talking points or full script here…"
                  spellCheck
                />
              ) : (
                <div
                  ref={prompterScrollRef}
                  className={`rs-prompter-scroll is-size-${prompterSize}`}
                >
                  <div className="rs-prompter-mover">
                    <div className="rs-prompter-pad" />
                    <p className="rs-prompter-text">
                      {prompterScript.trim() ||
                        "Add a script in Edit, then switch to Read."}
                    </p>
                    <div className="rs-prompter-pad" />
                  </div>
                </div>
              )}

              <div className="rs-prompter-controls">
                <button
                  type="button"
                  className={`rs-prompter-play${prompterPlaying ? " is-live" : ""}`}
                  disabled={prompterEditing || !prompterScript.trim()}
                  onClick={() => setPrompterPlaying((v) => !v)}
                  title={prompterPlaying ? "Pause scroll" : "Start scroll"}
                >
                  {prompterPlaying ? (
                    <IconPlayerPause size={16} stroke={1.75} />
                  ) : (
                    <IconPlayerPlay size={16} stroke={1.75} />
                  )}
                  {prompterPlaying ? "Pause" : "Scroll"}
                </button>

                <div className="rs-prompter-stepper" title="Scroll speed (0.25× steps)">
                  <button
                    type="button"
                    aria-label="Slower"
                    disabled={prompterSpeed <= 0.5}
                    onClick={() =>
                      setPrompterSpeed((s) =>
                        Math.round(Math.max(0.5, s - 0.25) * 100) / 100,
                      )
                    }
                  >
                    <IconMinus size={14} stroke={2} />
                  </button>
                  <span>{Number(prompterSpeed.toFixed(2))}x</span>
                  <button
                    type="button"
                    aria-label="Faster"
                    disabled={prompterSpeed >= 5}
                    onClick={() =>
                      setPrompterSpeed((s) =>
                        Math.round(Math.min(5, s + 0.25) * 100) / 100,
                      )
                    }
                  >
                    <IconPlus size={14} stroke={2} />
                  </button>
                </div>

                <div className="rs-prompter-stepper" title="Text size">
                  <button
                    type="button"
                    aria-label="Smaller text"
                    disabled={prompterSize <= 1}
                    onClick={() => setPrompterSize((s) => Math.max(1, s - 1))}
                  >
                    A
                  </button>
                  <button
                    type="button"
                    aria-label="Larger text"
                    className="is-lg"
                    disabled={prompterSize >= 3}
                    onClick={() => setPrompterSize((s) => Math.min(3, s + 1))}
                  >
                    A
                  </button>
                </div>
              </div>
            </aside>
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
                          {p.admitted
                            ? `${p.role} · ${active.settings.resolution}`
                            : "Waiting in lobby"}
                        </div>
                      </div>
                    </div>
                    {p.id !== active.participantId && p.admitted && (
                      <PeerVolumeControl
                        name={p.name.replace(" (You)", "")}
                        volume={peerVolume(p.id)}
                        onChange={(v) => setPeerVolume(p.id, v)}
                      />
                    )}
                    {!p.admitted && active.role === "host" && (
                      <button
                        type="button"
                        className="rs-copy-btn"
                        style={{ marginTop: 10 }}
                        disabled={studioFull}
                        title={
                          studioFull
                            ? `Studio is full (max ${MAX_GUESTS} guests)`
                            : "Admit guest"
                        }
                        onClick={() => void admitGuest(p.id)}
                      >
                        {studioFull ? "Studio full" : "Admit"}
                      </button>
                    )}
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
              title={
                id === "people" && waitingPeers.length
                  ? `${label} · ${waitingPeers.length} waiting`
                  : label
              }
              style={{ position: "relative" }}
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
              {id === "people" && waitingPeers.length > 0 && (
                <span className="rs-rail-badge">{waitingPeers.length}</span>
              )}
            </button>
          ))}
          <button
            type="button"
            className={prompterOpen ? "is-active" : ""}
            title="Teleprompter"
            aria-pressed={prompterOpen}
            onClick={() => setPrompterOpen((v) => !v)}
          >
            <IconScript size={20} stroke={1.75} />
          </button>
        </nav>
      </div>

      <div className="rs-bottom-bar">
        {active.role === "host" && (
          <button
            type="button"
            className={`rs-record ${recording ? "is-live" : ""}`}
            title={
              recording
                ? "Stop recording and end the meeting"
                : "Start recording"
            }
            onClick={() => (recording ? void stopRecord() : startCountdown())}
          >
            <span className="rs-record-dot" />
            {recording ? (
              <span className="rs-record-live-label">
                <span className="rs-record-time">{formatTime(elapsed)}</span>
                <span className="rs-record-end">Stop &amp; end meeting</span>
              </span>
            ) : (
              "Record"
            )}
          </button>
        )}
        {active.role === "guest" && (recording || countdown !== null) && (
          <div
            className={`rs-record is-live rs-record-status${
              countdown !== null ? " is-countdown" : ""
            }`}
            role="status"
            aria-live="polite"
            title={
              recording ? "Host started recording" : "Recording starting…"
            }
          >
            <span className="rs-record-dot" />
            <span className="rs-record-live-label">
              <span className="rs-record-time">
                {recording ? formatTime(elapsed) : "Get ready"}
              </span>
              <span className="rs-record-end">
                {recording ? "Recording" : "Recording soon"}
              </span>
            </span>
          </div>
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
          disabled={!videoMode || sharing}
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
        <div className="rs-bar-menu" ref={speakerMenuRef}>
          <button
            type="button"
            className={`rs-bar-item${speakerOpen ? " is-active" : ""}`}
            aria-expanded={speakerOpen}
            aria-haspopup="listbox"
            onClick={() => setSpeakerOpen((v) => !v)}
          >
            <IconVolume size={20} stroke={1.75} />
            Speaker
          </button>
          {speakerOpen && (
            <div className="rs-bar-popover" role="listbox" aria-label="Speakers">
              <p className="rs-bar-popover-label">Output</p>
              {deviceList.speakers.length === 0 ? (
                <p className="rs-bar-popover-empty">No speakers found</p>
              ) : (
                deviceList.speakers.map((d) => {
                  const selected = d.deviceId === speakerDevice;
                  return (
                    <button
                      key={d.deviceId}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      className={`rs-bar-popover-item${selected ? " is-selected" : ""}`}
                      onClick={() => void selectSpeaker(d.deviceId)}
                    >
                      <span>{d.label || "Speaker"}</span>
                      {selected && <IconCheck size={14} stroke={2.5} />}
                    </button>
                  );
                })
              )}
            </div>
          )}
        </div>
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
        <button
          type="button"
          className={`rs-bar-item${sharing ? " is-active" : ""}`}
          onClick={() => void toggleScreenShare()}
        >
          {sharing ? (
            <IconScreenShareOff size={20} stroke={1.75} />
          ) : (
            <IconScreenShare size={20} stroke={1.75} />
          )}
          {sharing ? "Stop" : "Share"}
        </button>
        <button type="button" className="rs-bar-item is-leave" onClick={() => void leave()}>
          <IconPhoneOff size={20} stroke={1.75} />
          Leave
        </button>
      </div>

      {countdown !== null && <div className="countdown">{countdown}</div>}

      {showStayModal && (
        <div className="modal-backdrop">
          <div className="modal rs-transfer-modal">
            <h3>
              {active.role === "guest"
                ? transferStatus === "done"
                  ? "Sent to host"
                  : transferStatus === "failed"
                    ? "Saved locally — resume when host is back"
                    : transferStatus === "sending"
                      ? "Uploading to host"
                      : "Finishing your recording"
                : transferStatus === "done"
                  ? "Meeting ended — recording saved"
                  : "Meeting ended — finishing up"}
            </h3>
            <p style={{ color: "var(--text-muted)" }}>
              {transferMsg ||
                (active.role === "guest"
                  ? "Camera and mic are off. Your full-quality video stays on this computer until the host has a copy. If the link drops, we keep your place and you can resume."
                  : "Camera and mic are off. Guest high-quality videos arrive here when they finish sending — keep the studio open so they can reconnect.")}
            </p>
            {active.role === "guest" &&
            (transferStatus === "sending" ||
              transferStatus === "done" ||
              transferStatus === "failed") ? (
              <div style={{ margin: "16px 0" }}>
                <TransferProgressBar
                  stats={
                    transferStatus === "done"
                      ? { ...xferStats, pct: 100, etaSec: 0 }
                      : xferStats
                  }
                  done={transferStatus === "done"}
                  error={
                    transferStatus === "failed" ? transferMsg : null
                  }
                  label={
                    xferFile ? `Sending ${xferFile}` : "Syncing to host"
                  }
                />
              </div>
            ) : active.role === "host" &&
              (hostRecvActive ||
                (transferStatus !== "saving" && recvElapsed > 0)) ? (
              <div style={{ margin: "16px 0" }}>
                <TransferProgressBar
                  stats={
                    transferStatus === "done" && !hostRecvActive
                      ? { ...xferStats, pct: 100, etaSec: 0 }
                      : xferStats
                  }
                  done={transferStatus === "done" && !hostRecvActive}
                  label={
                    xferFile
                      ? `Receiving ${xferFile}`
                      : "Waiting for guest uploads…"
                  }
                  doneLabel="Guest videos received"
                  doneMeta="received"
                  elapsedSec={recvElapsed}
                />
              </div>
            ) : (
              <div style={{ margin: "16px 0" }}>
                <div className="rs-xfer-top" style={{ marginBottom: 8 }}>
                  <span className="rs-xfer-label">
                    {transferStatus === "saving"
                      ? "Saving local tracks…"
                      : "Working…"}
                  </span>
                  <span className="rs-xfer-pct">
                    {Math.round(Math.max(uploadFrac, 0.05) * 100)}%
                  </span>
                </div>
                <div className="level-meter">
                  <span
                    style={{ width: `${Math.max(uploadFrac, 0.05) * 100}%` }}
                  />
                </div>
              </div>
            )}
            <div className="rs-transfer-actions">
              {active.role === "guest" && transferStatus !== "saving" && (
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={resumingTransfer || transferStatus === "sending"}
                  onClick={() => void sendTracksToHost()}
                >
                  {resumingTransfer || transferStatus === "sending"
                    ? "Sending…"
                    : transferStatus === "failed"
                      ? "Resume send to host"
                      : transferStatus === "done"
                        ? "Send again"
                        : "Send to host"}
                </button>
              )}
              {hasRecording && (
                <button
                  type="button"
                  className="btn btn-secondary"
                  onClick={() => {
                    void api
                      .revealSessionFolder(active.sessionId)
                      .catch((e) => alert(String(e)));
                  }}
                >
                  Show files on this computer
                </button>
              )}
              <button
                type="button"
                className="btn btn-secondary"
                disabled={transferStatus === "saving"}
                onClick={() => {
                  setShowStayModal(false);
                  void leave();
                }}
              >
                {active.role === "guest"
                  ? transferStatus === "done"
                    ? "View recordings"
                    : "Leave — retry later from Recordings"
                  : transferStatus === "done"
                    ? "View recordings"
                    : "Leave studio"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

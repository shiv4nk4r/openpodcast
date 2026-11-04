export type StudioMode = "audio_only" | "audio_video";

export interface StudioSettings {
  name: string;
  mode: StudioMode;
  frame_rate: number;
  resolution: string;
}

export interface Participant {
  id: string;
  name: string;
  role: string;
  resolution: string;
}

/** Join/leave on the take timeline (ms from Record). */
export interface PresenceEvent {
  participant_id: string;
  kind: "join" | "leave" | string;
  t_ms: number;
  name?: string;
}

export type RenderLayout = "dynamic" | "equal" | "normal";

export interface PresenceInterval {
  participant_id: string;
  start_ms: number;
  end_ms: number | null;
}

export interface SessionRecord {
  id: string;
  name: string;
  mode: StudioMode;
  frame_rate: number;
  resolution: string;
  created_at: string;
  duration_secs: number;
  participants: Participant[];
  has_video: boolean;
  local_path: string;
  drive_uploaded: boolean;
}

/** Voice EQ band — mirrors src/lib/eq.ts / Rust EqBand. */
export interface EqBandSettings {
  id: string;
  type: string;
  frequency: number;
  gain: number;
  q: number;
  enabled: boolean;
}

export interface EqProfileSettings {
  id: string;
  name: string;
  bands: EqBandSettings[];
  bypass: boolean;
}

export interface AppSettings {
  display_name: string;
  recording_folder: string | null;
  drive_connected: boolean;
  ai_base_url: string | null;
  ai_has_key: boolean;
  echo_cancellation: boolean;
  preferred_camera_id: string | null;
  preferred_mic_id: string | null;
  preferred_speaker_id: string | null;
  /** "dark" | "light" */
  theme: string;
  /** User-saved EQ profiles (builtins live in code). */
  eq_profiles: EqProfileSettings[];
  /** Builtin or saved profile id. */
  active_eq_profile_id: string;
}

export interface InviteTicket {
  /** 2 = iroh cross-network. Version 1 was LAN-only TCP. */
  version: number;
  iroh_ticket: string;
  password: string;
  studio: StudioSettings;
  host_name: string;
  created_at: string;
}

export interface PeerInfo {
  id: string;
  name: string;
  role: string;
  admitted: boolean;
}

export interface DeviceInfo {
  id: string;
  label: string;
  kind: string;
}

export interface CaptionCue {
  index: number;
  start_ms: number;
  end_ms: number;
  speaker: string;
  text: string;
}

export interface StudioRuntime {
  role: "host" | "guest";
  sessionId: string;
  participantId: string;
  invite?: string;
  /** Present for guests — the host's display name from the invite. */
  hostName?: string;
  settings: StudioSettings;
  displayName: string;
}

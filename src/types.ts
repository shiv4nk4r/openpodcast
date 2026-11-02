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
}

export interface InviteTicket {
  version: number;
  host_addrs: string[];
  port: number;
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
  settings: StudioSettings;
  displayName: string;
}

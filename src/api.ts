import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  CaptionCue,
  DeviceInfo,
  InviteTicket,
  PeerInfo,
  SessionRecord,
  StudioSettings,
} from "./types";

/**
 * Tauri 2 renames command argument keys to camelCase on the IPC boundary.
 * Nested serde structs keep their Rust field names (snake_case) unless renamed.
 */
export const api = {
  getSettings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) =>
    invoke<AppSettings>("save_app_settings", { settings }),
  setAiProvider: (baseUrl: string, apiKey: string) =>
    invoke<AppSettings>("set_ai_provider", {
      baseUrl,
      apiKey,
    }),
  getAppPaths: () =>
    invoke<{
      data: string;
      sessions: string;
      recordings: string;
      drafts: string;
      models: string;
    }>("get_app_paths"),
  listSessions: () => invoke<SessionRecord[]>("list_sessions"),
  getSession: (id: string) => invoke<SessionRecord | null>("get_session", { id }),
  renameSession: (sessionId: string, name: string) =>
    invoke<SessionRecord>("rename_session", { sessionId, name }),
  createStudio: (settings: StudioSettings, hostName: string) =>
    invoke<{
      session_id: string;
      invite: string;
      peers: PeerInfo[];
      session: SessionRecord;
    }>("create_studio", { settings, hostName }),
  getInvite: (hostName?: string) =>
    invoke<string>("get_invite", { hostName: hostName ?? null }),
  updateStudioSettings: (sessionId: string, settings: StudioSettings) =>
    invoke<StudioSettings>("update_studio_settings", { sessionId, settings }),
  listPeers: () => invoke<PeerInfo[]>("list_peers"),
  admitGuest: (participantId: string) =>
    invoke<void>("admit_guest", { participantId }),
  sendPreviewFrame: (
    participantId: string,
    jpegB64?: string,
    pcmB64?: string,
  ) =>
    invoke<void>("send_preview_frame", {
      participantId,
      jpegB64: jpegB64 ?? null,
      pcmB64: pcmB64 ?? null,
    }),
  decodeInvite: (invite: string) =>
    invoke<InviteTicket>("decode_invite", { invite }),
  joinStudio: (invite: string, displayName: string) =>
    invoke<{
      session_id: string;
      participant_id: string;
      studio: StudioSettings;
      host_name: string;
      connected: boolean;
      draft_path?: string;
      local_path?: string;
    }>("join_studio", { invite, displayName }),
  listAudioDevices: () => invoke<DeviceInfo[]>("list_audio_devices"),
  startAudioRecording: (
    sessionId: string,
    participantId: string,
    deviceName?: string,
  ) =>
    invoke<string>("start_audio_recording", {
      sessionId,
      participantId,
      deviceName: deviceName ?? null,
    }),
  getMicLevel: (participantId: string) =>
    invoke<number>("get_mic_level", { participantId }),
  stopAudioRecording: (participantId: string) =>
    invoke<string>("stop_audio_recording", { participantId }),
  saveVideoTrack: (
    sessionId: string,
    participantId: string,
    data: number[] | Uint8Array,
    extension: string,
  ) =>
    invoke<string>("save_video_track", {
      sessionId,
      participantId,
      // Pass bytes directly — Array.from on multi‑MB video freezes the UI.
      data: data instanceof Uint8Array ? data : new Uint8Array(data),
      extension,
    }),
  /** Truncate/create the on-disk video file before streaming chunks. */
  beginVideoTrack: (
    sessionId: string,
    participantId: string,
    extension: string,
  ) =>
    invoke<string>("begin_video_track", {
      sessionId,
      participantId,
      extension,
    }),
  /** Append one MediaRecorder timeslice (keeps long recordings off the JS heap). */
  appendVideoChunk: (
    sessionId: string,
    participantId: string,
    data: Uint8Array,
    extension: string,
  ) =>
    invoke<number>("append_video_chunk", {
      sessionId,
      participantId,
      data,
      extension,
    }),
  finalizeSession: (
    sessionId: string,
    durationSecs: number,
    participants: { id: string; name: string; role: string; resolution: string }[],
    padOffsetsMs: Record<string, number>,
    presenceEvents?: {
      participant_id: string;
      kind: string;
      t_ms: number;
      name?: string;
    }[],
  ) =>
    invoke<SessionRecord>("finalize_session", {
      sessionId,
      durationSecs,
      participants,
      padOffsetsMs,
      presenceEvents: presenceEvents ?? null,
    }),
  getSessionTimeline: (sessionId: string) =>
    invoke<{
      duration_secs: number;
      offsets_ms: Record<string, number>;
      presence_events: {
        participant_id: string;
        kind: string;
        t_ms: number;
        name?: string;
      }[];
      participants: {
        id: string;
        name: string;
        role: string;
        resolution: string;
      }[];
    }>("get_session_timeline", { sessionId }),
  renderSession: (
    sessionId: string,
    layout: string,
    mode: "video" | "audio" = "video",
    opts?: { resolution?: string; fps?: number },
  ) =>
    invoke<string>("render_session", {
      sessionId,
      layout,
      mode,
      resolution: opts?.resolution ?? null,
      fps: opts?.fps ?? null,
    }),
  /** FCPXML timeline for DaVinci Resolve / Premiere / Final Cut. */
  exportNleTimeline: (
    sessionId: string,
    opts?: { resolution?: string; fps?: number },
  ) =>
    invoke<string>("export_nle_timeline", {
      sessionId,
      resolution: opts?.resolution ?? null,
      fps: opts?.fps ?? null,
    }),
  discardSession: (sessionId: string) =>
    invoke<void>("discard_session", { sessionId }),
  /** Permanently delete a saved recording and its local files. */
  deleteSession: (sessionId: string) =>
    invoke<void>("discard_session", { sessionId }),
  leaveStudio: () => invoke<void>("leave_studio"),
  beginDriveOauth: () => invoke<string>("begin_drive_oauth"),
  completeDriveOauth: (code: string) =>
    invoke<AppSettings>("complete_drive_oauth", { code }),
  saveGoogleClientSecret: (secret: string) =>
    invoke<boolean>("save_google_client_secret", { secret }),
  googleClientSecretConfigured: () =>
    invoke<boolean>("google_client_secret_configured"),
  disconnectDrive: () => invoke<AppSettings>("disconnect_drive"),
  uploadSessionToDrive: (sessionId: string) =>
    invoke<{ uploaded: unknown[] }>("upload_session_to_drive", {
      sessionId,
    }),
  whisperModelStatus: () => invoke<{ ready: boolean }>("whisper_model_status"),
  downloadWhisperModel: () => invoke<void>("download_whisper_model"),
  nativeSttStatus: () =>
    invoke<{ available: boolean; engine: string }>("native_stt_status"),
  openSpeechSettings: () => invoke<void>("open_speech_settings"),
  generateCaptions: (
    sessionId: string,
    engine: "native" | "ai" | "auto" = "auto",
  ) => invoke<CaptionCue[]>("generate_captions", { sessionId, engine }),
  loadCaptions: (sessionId: string) =>
    invoke<CaptionCue[]>("load_captions", { sessionId }),
  saveCaptions: (sessionId: string, cues: CaptionCue[]) =>
    invoke<CaptionCue[]>("save_captions", { sessionId, cues }),
  generateShowNotes: (transcript: string) =>
    invoke<string>("generate_show_notes_cmd", { transcript }),
  exportSession: (
    req: {
      sessionId: string;
      layout: string;
      includeCaptions: boolean;
      trimStartMs: number;
      trimEndMs: number | null;
    },
    cues: CaptionCue[],
  ) =>
    invoke<string>("export_session_cmd", {
      // Nested ExportRequest uses serde field names (snake_case) unless renamed.
      req: {
        session_id: req.sessionId,
        layout: req.layout,
        include_captions: req.includeCaptions,
        trim_start_ms: req.trimStartMs,
        trim_end_ms: req.trimEndMs,
      },
      cues,
    }),
  listSessionFiles: (sessionId: string) =>
    invoke<string[]>("list_session_files", { sessionId }),
  getSessionMedia: (sessionId: string) =>
    invoke<
      {
        participant_id: string;
        video_path: string | null;
        audio_path: string | null;
      }[]
    >("get_session_media", { sessionId }),
  saveSessionThumbnail: (sessionId: string, jpeg: number[]) =>
    invoke<string>("save_session_thumbnail", { sessionId, jpeg }),
  getSessionThumbnail: (sessionId: string) =>
    invoke<string | null>("get_session_thumbnail", { sessionId }),
  revealSessionFolder: (sessionId: string) =>
    invoke<string>("reveal_session_folder", { sessionId }),
  /** Host: guests start local HQ recording into the shared take. */
  signalRecordStart: (sessionId: string) =>
    invoke<void>("signal_record_start", { sessionId }),
  /** Host: guests stop and push tracks back. Returns admitted guest ids. */
  signalRecordStop: () => invoke<string[]>("signal_record_stop"),
  /** Guest: queue HQ tracks for durable delivery (survives leaving lobby). */
  enqueueTrackReturn: (
    localSessionId: string,
    hostSessionId: string,
    participantId: string,
    invite: string,
  ) =>
    invoke<{ id: string }>("enqueue_track_return", {
      localSessionId,
      hostSessionId,
      participantId,
      invite,
    }),
  listPendingTransfers: () =>
    invoke<
      {
        id: string;
        host_session_id: string;
        participant_id: string;
        invite: string;
        local_session_id: string;
        file_names: string[];
        last_error: string | null;
      }[]
    >("list_pending_transfers"),
  /** Guest: send local HQ tracks (live or reconnect). Resumes large uploads. */
  returnLocalTracks: (
    sessionId: string,
    participantId: string,
    invite?: string,
    hostSessionId?: string,
  ) =>
    invoke<string[]>("return_local_tracks", {
      sessionId,
      participantId,
      invite: invite ?? null,
      hostSessionId: hostSessionId ?? null,
    }),
};

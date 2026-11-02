mod capture;
mod db;
mod drive;
mod export;
mod models;
mod native_stt;
mod p2p;
mod paths;
mod settings;
mod whisper;

use capture::AudioRecorder;
use db::Database;
use models::*;
use parking_lot::Mutex;
use p2p::StudioHost;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{State, WebviewWindowBuilder};
use uuid::Uuid;

/// Origins served by this app (Vite in `tauri dev`, custom protocol in release).
#[cfg(windows)]
fn is_app_origin(uri: &str) -> bool {
    let rest = match uri
        .strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"))
    {
        Some(rest) => rest,
        None => return false,
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    host == "tauri.localhost" || host == "localhost" || host == "127.0.0.1"
}

/// WebView2 needs an explicit PermissionRequested answer for mic/camera.
/// macOS WKWebView already grants media capture by default in wry.
#[cfg(windows)]
fn grant_webview_media_permissions(win: &tauri::WebviewWindow) {
    let _ = win.with_webview(|webview| {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_13, ICoreWebView2Profile4, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION,
            COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        };
        use webview2_com::{
            take_pwstr, PermissionRequestedEventHandler, SetPermissionStateCompletedHandler,
        };
        use windows::core::{Interface, w};
        use windows_core::PCWSTR;

        let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
            return;
        };

        // Override any previously persisted Deny so getUserMedia works again.
        if let Ok(core13) = core.cast::<ICoreWebView2_13>() {
            if let Ok(profile) = unsafe { core13.Profile() } {
                if let Ok(profile4) = profile.cast::<ICoreWebView2Profile4>() {
                    let done = SetPermissionStateCompletedHandler::create(Box::new(|_| Ok(())));
                    let origins = [
                        w!("http://localhost:1420/"),
                        w!("https://tauri.localhost/"),
                        w!("http://tauri.localhost/"),
                        w!("http://127.0.0.1:1420/"),
                    ];
                    for origin in origins {
                        for kind in [
                            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                            COREWEBVIEW2_PERMISSION_KIND_CAMERA,
                        ] {
                            let _ = unsafe {
                                profile4.SetPermissionState(
                                    kind,
                                    PCWSTR(origin.as_ptr()),
                                    COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                                    &done,
                                )
                            };
                        }
                    }
                }
            }
        }

        let handler = PermissionRequestedEventHandler::create(Box::new(
            move |_sender, args| -> windows_core::Result<()> {
                let Some(args) = args else {
                    return Ok(());
                };
                unsafe {
                    let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
                    args.PermissionKind(&mut kind)?;
                    if kind != COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                        && kind != COREWEBVIEW2_PERMISSION_KIND_CAMERA
                    {
                        return Ok(());
                    }
                    let mut uri = windows_core::PWSTR::null();
                    args.Uri(&mut uri)?;
                    if is_app_origin(&take_pwstr(uri)) {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                    }
                }
                Ok(())
            },
        ));
        let mut token = 0i64;
        let _ = unsafe { core.add_PermissionRequested(&handler, &mut token) };
    });
}

pub struct AppState {
    pub db: Database,
    pub host: Mutex<Option<StudioHost>>,
    pub recorders: Mutex<HashMap<String, AudioRecorder>>,
    pub active_session: Mutex<Option<String>>,
    /// session_id → Recordings/... folder (created when Record starts)
    pub session_dirs: Mutex<HashMap<String, PathBuf>>,
    /// session_id → Drafts/*.json (lobby / never-recorded)
    pub session_drafts: Mutex<HashMap<String, PathBuf>>,
}

impl AppState {
    fn new() -> Result<Self, String> {
        Ok(Self {
            db: Database::open()?,
            host: Mutex::new(None),
            recorders: Mutex::new(HashMap::new()),
            active_session: Mutex::new(None),
            session_dirs: Mutex::new(HashMap::new()),
            session_drafts: Mutex::new(HashMap::new()),
        })
    }
}

fn read_sync_title(sync: &serde_json::Value) -> String {
    sync.get("studio")
        .and_then(|s| s.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Untitled Recording")
        .to_string()
}

fn write_draft_sync(state: &AppState, session_id: &str, sync: &serde_json::Value) -> Result<PathBuf, String> {
    // Resolve path with the map lock released before any second lock / FS work.
    // (if-let temporaries hold MutexGuard for the whole if/else — that deadlocks.)
    let cached = state.session_drafts.lock().get(session_id).cloned();
    let path = if let Some(existing) = cached {
        existing
    } else if let Some(existing) = paths::find_draft_file(session_id) {
        state
            .session_drafts
            .lock()
            .insert(session_id.to_string(), existing.clone());
        existing
    } else {
        let path = paths::drafts_dir()?.join(paths::structured_draft_filename(session_id));
        state
            .session_drafts
            .lock()
            .insert(session_id.to_string(), path.clone());
        path
    };
    std::fs::write(&path, serde_json::to_string_pretty(sync).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(path)
}

fn resolve_session_dir(state: &AppState, session_id: &str) -> Result<PathBuf, String> {
    if let Some(p) = state.session_dirs.lock().get(session_id).cloned() {
        return Ok(p);
    }
    if let Ok(Some(rec)) = state.db.get_session(session_id) {
        let p = PathBuf::from(&rec.local_path);
        if p.is_dir() {
            state
                .session_dirs
                .lock()
                .insert(session_id.to_string(), p.clone());
            return Ok(p);
        }
    }
    if let Some(p) = paths::find_recording_dir(session_id) {
        state
            .session_dirs
            .lock()
            .insert(session_id.to_string(), p.clone());
        return Ok(p);
    }
    Err(format!("Recording folder not found for session {session_id}"))
}

/// Promote a lobby draft into `Recordings/<structured-name>/` the first time we record.
fn ensure_recording_dir(state: &AppState, session_id: &str) -> Result<PathBuf, String> {
    if let Ok(existing) = resolve_session_dir(state, session_id) {
        return Ok(existing);
    }

    let mut sync = serde_json::json!({ "session_id": session_id });
    let draft_path = state
        .session_drafts
        .lock()
        .get(session_id)
        .cloned()
        .or_else(|| paths::find_draft_file(session_id));

    if let Some(ref draft) = draft_path {
        if let Ok(raw) = std::fs::read_to_string(draft) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                sync = v;
            }
        }
    }
    sync["session_id"] = serde_json::json!(session_id);

    let title = read_sync_title(&sync);
    let folder = paths::structured_recording_name(&title, session_id);
    let dir = paths::recordings_dir()?.join(folder);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let sync_path = dir.join("sync.json");
    std::fs::write(
        &sync_path,
        serde_json::to_string_pretty(&sync).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    if let Some(draft) = draft_path {
        let _ = std::fs::remove_file(&draft);
        state.session_drafts.lock().remove(session_id);
    }

    state
        .session_dirs
        .lock()
        .insert(session_id.to_string(), dir.clone());
    Ok(dir)
}

fn update_session_sync(state: &AppState, session_id: &str, mutate: impl FnOnce(&mut serde_json::Value)) -> Result<(), String> {
    if let Ok(dir) = resolve_session_dir(state, session_id) {
        let sync_path = dir.join("sync.json");
        let mut sync: serde_json::Value = if sync_path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&sync_path).unwrap_or_default())
                .unwrap_or(serde_json::json!({ "session_id": session_id }))
        } else {
            serde_json::json!({ "session_id": session_id })
        };
        mutate(&mut sync);
        std::fs::write(
            &sync_path,
            serde_json::to_string_pretty(&sync).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }

    let draft = state
        .session_drafts
        .lock()
        .get(session_id)
        .cloned()
        .or_else(|| paths::find_draft_file(session_id));
    let mut sync: serde_json::Value = if let Some(ref path) = draft {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_default())
            .unwrap_or(serde_json::json!({ "session_id": session_id }))
    } else {
        serde_json::json!({ "session_id": session_id })
    };
    mutate(&mut sync);
    write_draft_sync(state, session_id, &sync)?;
    Ok(())
}

#[tauri::command]
fn get_settings() -> AppSettings {
    settings::load_settings()
}

#[tauri::command]
fn save_app_settings(settings: AppSettings) -> Result<AppSettings, String> {
    settings::save_settings(&settings)?;
    Ok(settings::load_settings())
}

#[tauri::command]
fn set_ai_provider(base_url: String, api_key: String) -> Result<AppSettings, String> {
    let mut s = settings::load_settings();
    s.ai_base_url = if base_url.is_empty() {
        None
    } else {
        Some(base_url)
    };
    if !api_key.is_empty() {
        settings::set_ai_api_key(&api_key)?;
    }
    settings::save_settings(&s)?;
    Ok(settings::load_settings())
}

#[tauri::command]
fn get_app_paths() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "data": paths::app_data_dir()?.to_string_lossy(),
        "sessions": paths::recordings_dir()?.to_string_lossy(),
        "recordings": paths::recordings_dir()?.to_string_lossy(),
        "drafts": paths::drafts_dir()?.to_string_lossy(),
        "models": paths::models_dir()?.to_string_lossy(),
    }))
}

#[tauri::command]
fn list_sessions(state: State<'_, Arc<AppState>>) -> Result<Vec<SessionRecord>, String> {
    // Purge lobby-only / never-recorded sessions left over from older builds
    let all = state.db.list_sessions()?;
    for s in &all {
        let has_media = PathBuf::from(&s.local_path)
            .exists()
            .then(|| session_has_media_files(&PathBuf::from(&s.local_path)))
            .unwrap_or(false);
        if s.duration_secs < 0.4 && !has_media {
            let _ = state.db.delete_session(&s.id);
            let _ = std::fs::remove_dir_all(&s.local_path);
        }
    }
    Ok(state
        .db
        .list_sessions()?
        .into_iter()
        .filter(|s| s.duration_secs >= 0.4 || {
            PathBuf::from(&s.local_path)
                .exists()
                .then(|| session_has_media_files(&PathBuf::from(&s.local_path)))
                .unwrap_or(false)
        })
        .collect())
}

#[tauri::command]
fn get_session(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Option<SessionRecord>, String> {
    state.db.get_session(&id)
}

#[tauri::command]
async fn create_studio(
    state: State<'_, Arc<AppState>>,
    settings: StudioSettings,
    host_name: String,
) -> Result<serde_json::Value, String> {
    let session_id = Uuid::new_v4().to_string();
    // Lobby-only: keep sync.json in Drafts/ until the host actually records.
    let sync = serde_json::json!({
        "session_id": session_id,
        "studio": settings,
        "host_name": host_name,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "offsets_ms": {},
    });
    let draft = write_draft_sync(&state, &session_id, &sync)?;

    let host = StudioHost::start(settings.clone(), host_name.clone()).await?;
    let ticket = host.encode_ticket()?;
    let peers = host.peer_list();
    *state.host.lock() = Some(host);
    *state.active_session.lock() = Some(session_id.clone());

    // Do NOT insert into the recordings DB until the host actually presses
    // Record and finalize_session runs — lobby/preview must not create episodes.
    Ok(serde_json::json!({
        "session_id": session_id,
        "invite": ticket,
        "peers": peers,
        "draft_path": draft.to_string_lossy(),
    }))
}

#[tauri::command]
async fn get_invite(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let (endpoint, mut ticket) = {
        let guard = state.host.lock();
        let host = guard
            .as_ref()
            .ok_or_else(|| "No active studio".to_string())?;
        (host.endpoint().clone(), host.ticket.clone())
    };
    p2p::wait_until_dialable(&endpoint).await?;
    ticket.iroh_ticket = iroh_tickets::endpoint::EndpointTicket::new(endpoint.addr()).to_string();
    p2p::encode_ticket(&ticket)
}

#[tauri::command]
fn update_studio_settings(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    settings: StudioSettings,
) -> Result<StudioSettings, String> {
    if let Some(host) = state.host.lock().as_mut() {
        host.update_studio_settings(settings.clone());
    }

    let settings_value = serde_json::to_value(&settings).unwrap_or_default();
    update_session_sync(&state, &session_id, |sync| {
        sync["studio"] = settings_value;
    })?;

    Ok(settings)
}

#[tauri::command]
fn list_peers(state: State<'_, Arc<AppState>>) -> Result<Vec<p2p::PeerInfo>, String> {
    Ok(state
        .host
        .lock()
        .as_ref()
        .map(|h| h.peer_list())
        .unwrap_or_default())
}

#[tauri::command]
fn admit_guest(state: State<'_, Arc<AppState>>, participant_id: String) -> Result<(), String> {
    state
        .host
        .lock()
        .as_ref()
        .ok_or_else(|| "No active studio".to_string())?
        .admit(&participant_id);
    Ok(())
}

#[tauri::command]
fn decode_invite(invite: String) -> Result<InviteTicket, String> {
    p2p::decode_ticket(&invite)
}

#[tauri::command]
async fn join_studio(
    state: State<'_, Arc<AppState>>,
    invite: String,
    display_name: String,
) -> Result<serde_json::Value, String> {
    let ticket = p2p::decode_ticket(&invite)?;
    let participant_id = Uuid::new_v4().to_string();
    let session_id = Uuid::new_v4().to_string();
    let sync = serde_json::json!({
        "session_id": session_id,
        "studio": ticket.studio,
        "guest_name": display_name,
        "participant_id": participant_id,
        "joined_at": chrono::Utc::now().to_rfc3339(),
    });
    let draft = write_draft_sync(&state, &session_id, &sync)?;
    *state.active_session.lock() = Some(session_id.clone());

    // Prove we can reach the host (registers the guest). Control/file channels reconnect as needed.
    let _stream = p2p::join_studio(&ticket, display_name.clone(), participant_id.clone()).await?;
    drop(_stream);

    Ok(serde_json::json!({
        "session_id": session_id,
        "participant_id": participant_id,
        "studio": ticket.studio,
        "host_name": ticket.host_name,
        "connected": true,
        "draft_path": draft.to_string_lossy(),
    }))
}

#[tauri::command]
fn list_audio_devices() -> Result<Vec<DeviceInfo>, String> {
    AudioRecorder::list_input_devices()
}

#[tauri::command]
fn start_audio_recording(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
    device_name: Option<String>,
) -> Result<String, String> {
    let dir = ensure_recording_dir(&state, &session_id)?;
    let path = dir.join(format!("{participant_id}-audio.wav"));
    let recorder = AudioRecorder::start(path.clone(), device_name)?;
    state
        .recorders
        .lock()
        .insert(participant_id.clone(), recorder);
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_mic_level(state: State<'_, Arc<AppState>>, participant_id: String) -> f32 {
    state
        .recorders
        .lock()
        .get(&participant_id)
        .map(|r| r.level())
        .unwrap_or(0.0)
}

#[tauri::command]
fn stop_audio_recording(
    state: State<'_, Arc<AppState>>,
    participant_id: String,
) -> Result<String, String> {
    let recorder = state
        .recorders
        .lock()
        .remove(&participant_id)
        .ok_or_else(|| "No active recorder".to_string())?;
    let path = recorder.stop();
    Ok(path.to_string_lossy().to_string())
}

fn video_track_path(
    state: &AppState,
    session_id: &str,
    participant_id: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let dir = ensure_recording_dir(state, session_id)?;
    let ext = if extension.trim().is_empty() {
        "webm"
    } else {
        extension.trim().trim_start_matches('.')
    };
    Ok(dir.join(format!("{participant_id}-video.{ext}")))
}

#[tauri::command]
fn save_video_track(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
    data: Vec<u8>,
    extension: String,
) -> Result<String, String> {
    let path = video_track_path(&state, &session_id, &participant_id, &extension)?;
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// Create/truncate the video file before streaming chunks (long sessions).
#[tauri::command]
fn begin_video_track(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
    extension: String,
) -> Result<String, String> {
    let path = video_track_path(&state, &session_id, &participant_id, &extension)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::File::create(&path).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// Append one MediaRecorder timeslice to disk — keeps RAM flat for hour-long takes.
#[tauri::command]
fn append_video_chunk(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
    data: Vec<u8>,
    extension: String,
) -> Result<u64, String> {
    use std::io::Write;
    if data.is_empty() {
        return Ok(0);
    }
    let path = video_track_path(&state, &session_id, &participant_id, &extension)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(&data).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    Ok(data.len() as u64)
}

#[tauri::command]
fn rename_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    name: String,
) -> Result<SessionRecord, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Name cannot be empty".into());
    }
    state.db.rename_session(&session_id, trimmed)?;

    let _ = update_session_sync(&state, &session_id, |sync| {
        if sync.get("studio").is_none() {
            sync["studio"] = serde_json::json!({});
        }
        if let Some(studio) = sync.get_mut("studio") {
            studio["name"] = serde_json::json!(trimmed);
        }
        sync["name"] = serde_json::json!(trimmed);
    });

    state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".into())
}

#[tauri::command]
fn finalize_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    duration_secs: f64,
    participants: Vec<Participant>,
    pad_offsets_ms: HashMap<String, u64>,
) -> Result<SessionRecord, String> {
    let dir = ensure_recording_dir(&state, &session_id)?;
    for (pid, pad) in &pad_offsets_ms {
        let wav = dir.join(format!("{pid}-audio.wav"));
        if wav.exists() && *pad > 0 {
            let _ = capture::pad_wav_start(&wav, *pad);
        }
    }

    let sync_path = dir.join("sync.json");
    let mut sync: serde_json::Value = if sync_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&sync_path).unwrap_or_default())
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    sync["session_id"] = serde_json::json!(session_id);
    sync["duration_secs"] = serde_json::json!(duration_secs);
    sync["offsets_ms"] = serde_json::to_value(&pad_offsets_ms).unwrap_or_default();
    sync["participants"] = serde_json::to_value(&participants).unwrap_or_default();
    std::fs::write(&sync_path, serde_json::to_string_pretty(&sync).unwrap())
        .map_err(|e| e.to_string())?;

    let studio = sync.get("studio");
    let studio_name = studio
        .and_then(|s| s.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Episode")
        .to_string();
    let studio_fps = studio
        .and_then(|s| s.get("frame_rate"))
        .and_then(|v| v.as_u64())
        .unwrap_or(30) as u32;
    let studio_res = studio
        .and_then(|s| s.get("resolution"))
        .and_then(|v| v.as_str())
        .unwrap_or("1080p")
        .to_string();
    let studio_mode = match studio
        .and_then(|s| s.get("mode"))
        .and_then(|v| v.as_str())
        .unwrap_or("audio_video")
    {
        "audio_only" => StudioMode::AudioOnly,
        _ => StudioMode::AudioVideo,
    };

    let existing = state.db.get_session(&session_id)?;
    let mut record = existing.unwrap_or(SessionRecord {
        id: session_id.clone(),
        name: studio_name.clone(),
        mode: studio_mode.clone(),
        frame_rate: studio_fps,
        resolution: studio_res.clone(),
        created_at: chrono::Utc::now(),
        duration_secs: 0.0,
        participants: vec![],
        has_video: true,
        local_path: dir.to_string_lossy().to_string(),
        drive_uploaded: false,
    });
    if record.name.trim().is_empty() || record.name == "Episode" {
        record.name = studio_name;
    }
    record.mode = studio_mode;
    record.frame_rate = studio_fps;
    record.resolution = studio_res;
    record.duration_secs = duration_secs;
    record.participants = participants;
    record.local_path = dir.to_string_lossy().to_string();
    record.has_video = session_has_video_files(&dir);
    // Only persist real takes — ignore accidental zero-length finalizes
    if duration_secs < 0.4 && !session_has_media_files(&dir) {
        let _ = state.db.delete_session(&session_id);
        let _ = std::fs::remove_dir_all(&dir);
        return Err("Recording was too short and was discarded".into());
    }
    state.db.upsert_session(&record)?;
    Ok(record)
}

fn session_has_video_files(dir: &PathBuf) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.contains("-video.")
                    && (name.ends_with(".webm")
                        || name.ends_with(".mp4")
                        || name.ends_with(".mov"))
            })
        })
        .unwrap_or(false)
}

fn session_has_media_files(dir: &PathBuf) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with("-audio.wav")
                    || name.contains("-video.")
            })
        })
        .unwrap_or(false)
}

#[tauri::command]
fn discard_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    let _ = state.db.delete_session(&session_id);

    if let Some(dir) = state.session_dirs.lock().remove(&session_id) {
        let _ = std::fs::remove_dir_all(dir);
    } else if let Some(dir) = paths::find_recording_dir(&session_id) {
        let _ = std::fs::remove_dir_all(dir);
    }

    if let Some(draft) = state.session_drafts.lock().remove(&session_id) {
        let _ = std::fs::remove_file(draft);
    } else if let Some(draft) = paths::find_draft_file(&session_id) {
        let _ = std::fs::remove_file(draft);
    }

    // Legacy UUID folder cleanup
    if let Ok(legacy_root) = paths::legacy_sessions_dir() {
        let legacy = legacy_root.join(&session_id);
        if legacy.exists() {
            let _ = std::fs::remove_dir_all(legacy);
        }
    }

    if state
        .active_session
        .lock()
        .as_ref()
        .map(|s| s == &session_id)
        .unwrap_or(false)
    {
        *state.active_session.lock() = None;
    }
    Ok(())
}

#[tauri::command]
fn leave_studio(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Some(host) = state.host.lock().take() {
        host.shutdown();
    }
    state.recorders.lock().clear();
    *state.active_session.lock() = None;
    Ok(())
}

#[tauri::command]
fn begin_drive_oauth() -> Result<String, String> {
    drive::begin_oauth()
}

#[tauri::command]
async fn complete_drive_oauth(code: String) -> Result<AppSettings, String> {
    drive::complete_oauth_with_code(code).await?;
    let mut s = settings::load_settings();
    s.drive_connected = true;
    settings::save_settings(&s)?;
    Ok(settings::load_settings())
}

#[tauri::command]
fn disconnect_drive() -> Result<AppSettings, String> {
    drive::disconnect()?;
    Ok(settings::load_settings())
}

#[tauri::command]
async fn upload_session_to_drive(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    if !drive::is_connected() {
        return Err("Connect Google Drive in Settings first".into());
    }
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    let dir = PathBuf::from(&session.local_path);
    let mut uploaded = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "wav" | "mp4" | "webm" | "srt" | "json") {
                let id = drive::upload_file(&path, &session.name, |_| {}).await?;
                uploaded.push(serde_json::json!({
                    "file": path.file_name().and_then(|n| n.to_str()),
                    "drive_id": id,
                }));
            }
        }
    }
    state.db.mark_drive_uploaded(&session_id)?;
    Ok(serde_json::json!({ "uploaded": uploaded }))
}

#[tauri::command]
fn whisper_model_status() -> serde_json::Value {
    serde_json::json!({ "ready": whisper::model_ready() })
}

#[tauri::command]
async fn download_whisper_model() -> Result<(), String> {
    whisper::download_model(|_| {}).await?;
    Ok(())
}

#[tauri::command]
fn native_stt_status() -> serde_json::Value {
    serde_json::json!({
        "available": native_stt::available(),
        "engine": native_stt::engine_label(),
    })
}

#[tauri::command]
fn open_speech_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition")
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        // Speech privacy / microphone related settings
        std::process::Command::new("cmd")
            .args(["/C", "start", "ms-settings:privacy-speech"])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("Speech settings are only available on macOS and Windows".into())
    }
}

#[tauri::command]
async fn generate_captions(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    engine: Option<String>,
) -> Result<Vec<CaptionCue>, String> {
    let engine = engine.unwrap_or_else(|| "auto".into());
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    let dir = PathBuf::from(&session.local_path);
    let mut all = Vec::new();
    let mut errors = Vec::new();

    async fn transcribe_one(
        path: &PathBuf,
        speaker: &str,
        engine: &str,
    ) -> Result<Vec<CaptionCue>, String> {
        match engine {
            "native" => native_stt::transcribe_wav(path, speaker).await,
            "ai" => whisper::transcribe_wav_ai_only(path, speaker).await,
            _ => {
                // auto: native → AI → local whisper CLI
                match native_stt::transcribe_wav(path, speaker).await {
                    Ok(cues) if !cues.is_empty() => Ok(cues),
                    Ok(_) | Err(_) => whisper::transcribe_wav(path, speaker).await,
                }
            }
        }
    }

    for p in &session.participants {
        let wav = dir.join(format!("{}-audio.wav", p.id));
        if wav.exists() {
            match transcribe_one(&wav, &p.name, &engine).await {
                Ok(cues) => all.extend(cues),
                Err(e) => errors.push(format!("{}: {e}", p.name)),
            }
        }
    }
    // Also pick up any *-audio.wav not in participants
    if all.is_empty() {
        for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with("-audio.wav") {
                    let speaker = name.trim_end_matches("-audio.wav");
                    match transcribe_one(&path, speaker, &engine).await {
                        Ok(cues) => all.extend(cues),
                        Err(e) => errors.push(format!("{speaker}: {e}")),
                    }
                }
            }
        }
    }
    if all.is_empty() {
        if errors.is_empty() {
            return Err(
                "No audio tracks found to transcribe. Record in the studio first, or type subtitles manually."
                    .into(),
            );
        }
        return Err(errors.join("\n"));
    }
    let merged = whisper::merge_cues(all);
    let srt = whisper::cues_to_srt(&merged);
    std::fs::write(dir.join("captions.srt"), &srt).map_err(|e| e.to_string())?;
    Ok(merged)
}

#[tauri::command]
fn load_captions(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<CaptionCue>, String> {
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    let path = PathBuf::from(&session.local_path).join("captions.srt");
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(whisper::parse_srt(&raw))
}

#[tauri::command]
fn save_captions(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    cues: Vec<CaptionCue>,
) -> Result<Vec<CaptionCue>, String> {
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    let dir = PathBuf::from(&session.local_path);
    let merged = whisper::merge_cues(cues);
    let srt = whisper::cues_to_srt(&merged);
    std::fs::write(dir.join("captions.srt"), &srt).map_err(|e| e.to_string())?;
    Ok(merged)
}

#[tauri::command]
async fn generate_show_notes_cmd(transcript: String) -> Result<String, String> {
    whisper::generate_show_notes(&transcript).await
}

#[tauri::command]
fn export_session_cmd(
    state: State<'_, Arc<AppState>>,
    req: export::ExportRequest,
    cues: Vec<CaptionCue>,
) -> Result<String, String> {
    let session = state
        .db
        .get_session(&req.session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    let dir = PathBuf::from(&session.local_path);
    let out = export::export_session(&dir, &req, &cues)?;
    Ok(out.to_string_lossy().to_string())
}

#[tauri::command]
fn list_session_files(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<String>, String> {
    let dir = resolve_session_dir(&state, &session_id)?;
    let mut files = Vec::new();
    if dir.exists() {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().is_file() {
                files.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    Ok(files)
}

#[derive(serde::Serialize)]
struct SessionMediaTrack {
    participant_id: String,
    video_path: Option<String>,
    audio_path: Option<String>,
}

#[tauri::command]
fn get_session_media(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<SessionMediaTrack>, String> {
    let dir = resolve_session_dir(&state, &session_id)?;
    let mut by_id: std::collections::HashMap<String, SessionMediaTrack> =
        std::collections::HashMap::new();

    if dir.exists() {
        for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let abs = path.to_string_lossy().to_string();
            if let Some(pid) = name.strip_suffix("-audio.wav") {
                by_id
                    .entry(pid.to_string())
                    .or_insert_with(|| SessionMediaTrack {
                        participant_id: pid.to_string(),
                        video_path: None,
                        audio_path: None,
                    })
                    .audio_path = Some(abs);
            } else if let Some(pid) = name
                .strip_suffix("-video.webm")
                .or_else(|| name.strip_suffix("-video.mp4"))
                .or_else(|| name.strip_suffix("-video.mkv"))
            {
                by_id
                    .entry(pid.to_string())
                    .or_insert_with(|| SessionMediaTrack {
                        participant_id: pid.to_string(),
                        video_path: None,
                        audio_path: None,
                    })
                    .video_path = Some(abs);
            }
        }
    }

    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.participant_id.cmp(&b.participant_id));
    Ok(out)
}

#[tauri::command]
fn save_session_thumbnail(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    jpeg: Vec<u8>,
) -> Result<String, String> {
    if jpeg.is_empty() {
        return Err("Empty thumbnail".into());
    }
    let dir = ensure_recording_dir(&state, &session_id)?;
    let path = dir.join("thumbnail.jpg");
    std::fs::write(&path, jpeg).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_session_thumbnail(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<String>, String> {
    let dir = resolve_session_dir(&state, &session_id).ok();
    let path = dir.map(|d| d.join("thumbnail.jpg"));
    if let Some(path) = path {
        if path.is_file() {
            return Ok(Some(path.to_string_lossy().to_string()));
        }
    }
    Ok(None)
}

#[tauri::command]
fn reveal_session_folder(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<String, String> {
    let dir = resolve_session_dir(&state, &session_id)?;
    if !dir.exists() {
        return Err("Session folder not found".into());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(dir.to_string_lossy().to_string())
}

/// Guest → host track return over the invite TCP session (resumable chunks).
#[tauri::command]
async fn return_tracks_to_host(
    state: State<'_, Arc<AppState>>,
    invite: String,
    participant_id: String,
    session_id: String,
) -> Result<Vec<String>, String> {
    let ticket = p2p::decode_ticket(&invite)?;
    let mut stream = p2p::join_studio(&ticket, participant_id.clone(), participant_id.clone()).await?;
    let dir = resolve_session_dir(&state, &session_id)?;
    let mut sent = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.contains("-audio.") || name.contains("-video.") {
                p2p::send_file(&mut stream, &participant_id, &path).await?;
                sent.push(name.to_string());
            }
        }
    }
    Ok(sent)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::new().expect("failed to init app state");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(Arc::new(state))
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_app_settings,
            set_ai_provider,
            get_app_paths,
            list_sessions,
            get_session,
            rename_session,
            create_studio,
            get_invite,
            update_studio_settings,
            list_peers,
            admit_guest,
            decode_invite,
            join_studio,
            list_audio_devices,
            start_audio_recording,
            get_mic_level,
            stop_audio_recording,
            save_video_track,
            begin_video_track,
            append_video_chunk,
            finalize_session,
            discard_session,
            leave_studio,
            begin_drive_oauth,
            complete_drive_oauth,
            disconnect_drive,
            upload_session_to_drive,
            whisper_model_status,
            download_whisper_model,
            native_stt_status,
            open_speech_settings,
            generate_captions,
            load_captions,
            save_captions,
            generate_show_notes_cmd,
            export_session_cmd,
            list_session_files,
            get_session_media,
            save_session_thumbnail,
            get_session_thumbnail,
            reveal_session_folder,
            return_tracks_to_host,
        ])
        .setup(|app| {
            let conf = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .expect("main window config missing");
            let window = WebviewWindowBuilder::from_config(app.handle(), &conf)?
                .on_permission_request(|_webview, kind| match kind {
                    PermissionKind::Microphone | PermissionKind::Camera => {
                        PermissionResponse::Allow
                    }
                    _ => PermissionResponse::Default,
                })
                .build()?;
            #[cfg(windows)]
            grant_webview_media_permissions(&window);
            let _ = window;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running OpenPodcast");
}

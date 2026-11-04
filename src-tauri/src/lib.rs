mod aec;
mod capture;
mod eq;
mod db;
mod drive;
mod export;
mod models;
mod native_stt;
mod p2p;
mod paths;
mod settings;
mod transfer_queue;
mod whisper;

use capture::AudioRecorder;
use db::Database;
use models::*;
use parking_lot::Mutex;
use p2p::{PreviewBus, RecordBus, RosterBus, StudioHost, TransferBus};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{Emitter, Manager, State, WebviewWindowBuilder};
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
    /// Guest keeps this open so the host can admit them and keep the session alive.
    pub guest_conn: Mutex<Option<p2p::GuestSession>>,
    pub recorders: Mutex<HashMap<String, AudioRecorder>>,
    pub active_session: Mutex<Option<String>>,
    /// session_id → Recordings/... folder (created when Record starts)
    pub session_dirs: Mutex<HashMap<String, PathBuf>>,
    /// session_id → Drafts/*.json (lobby / never-recorded)
    pub session_drafts: Mutex<HashMap<String, PathBuf>>,
    /// Open append handles for streaming video tracks (avoids reopen+fsync per chunk).
    pub video_writers: Mutex<HashMap<String, std::fs::File>>,
    /// Live A/V preview frames from the peer session → UI.
    pub preview_tx: PreviewBus,
    /// Full roster (host + guests) so every client can render everyone.
    pub roster_tx: RosterBus,
    /// Host → guest RecordStart / RecordStop.
    pub record_tx: RecordBus,
    /// Guest → host HQ file transfer progress.
    pub transfer_tx: TransferBus,
    /// Live-preview AEC3 (render reference = remote PCM, capture = outbound mic).
    pub aec: Arc<aec::AecEngine>,
}

impl AppState {
    fn new() -> Result<Self, String> {
        let (preview_tx, _) = tokio::sync::broadcast::channel(64);
        let (roster_tx, _) = tokio::sync::broadcast::channel(32);
        let (record_tx, _) = tokio::sync::broadcast::channel(16);
        let (transfer_tx, _) = tokio::sync::broadcast::channel(64);
        let aec = aec::AecEngine::new()?;
        // Match persisted preference so first join already has the right mode.
        aec.set_enabled(settings::load_settings().echo_cancellation);
        Ok(Self {
            db: Database::open()?,
            host: Mutex::new(None),
            guest_conn: Mutex::new(None),
            recorders: Mutex::new(HashMap::new()),
            active_session: Mutex::new(None),
            session_dirs: Mutex::new(HashMap::new()),
            session_drafts: Mutex::new(HashMap::new()),
            video_writers: Mutex::new(HashMap::new()),
            preview_tx,
            roster_tx,
            record_tx,
            transfer_tx,
            aec,
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
fn save_app_settings(
    state: State<'_, Arc<AppState>>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    settings::save_settings(&settings)?;
    state.aec.set_enabled(settings.echo_cancellation);
    Ok(settings::load_settings())
}

/// Enable/disable native AEC3 without rewriting all app settings (lobby headphones).
#[tauri::command]
fn set_native_aec(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<bool, String> {
    state.aec.set_enabled(enabled);
    Ok(state.aec.enabled())
}

#[tauri::command]
fn get_native_aec(state: State<'_, Arc<AppState>>) -> bool {
    state.aec.enabled()
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
    let host_name = {
        let t = host_name.trim();
        if t.is_empty() {
            "Host".to_string()
        } else {
            t.to_string()
        }
    };

    // Reuse an existing host endpoint so React Strict Mode / lobby remounts
    // don't orphan the dialable endpoint the UI already shared as an invite.
    let reused = {
        let mut guard = state.host.lock();
        if let Some(host) = guard.as_mut() {
            host.set_host_name(host_name.clone());
            host.update_studio_settings(settings.clone());
            let ticket = host.encode_ticket()?;
            let peers = host.peer_list();
            Some((ticket, peers))
        } else {
            None
        }
    };
    if let Some((ticket, peers)) = reused {
        let session_id = state
            .active_session
            .lock()
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        *state.active_session.lock() = Some(session_id.clone());
        let sync = serde_json::json!({
            "session_id": session_id,
            "studio": settings,
            "host_name": host_name,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "offsets_ms": {},
        });
        let draft = write_draft_sync(&state, &session_id, &sync)?;
        return Ok(serde_json::json!({
            "session_id": session_id,
            "invite": ticket,
            "peers": peers,
            "draft_path": draft.to_string_lossy(),
            "reused": true,
        }));
    }

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

    let host = StudioHost::start(
        settings.clone(),
        host_name.clone(),
        state.preview_tx.clone(),
        state.roster_tx.clone(),
        state.transfer_tx.clone(),
        state.aec.clone(),
    )
    .await?;
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
        "reused": false,
    }))
}

#[tauri::command]
async fn get_invite(
    state: State<'_, Arc<AppState>>,
    host_name: Option<String>,
) -> Result<String, String> {
    let (endpoint, mut ticket) = {
        let mut guard = state.host.lock();
        let host = guard
            .as_mut()
            .ok_or_else(|| "No active studio".to_string())?;
        if let Some(name) = host_name {
            let t = name.trim();
            if !t.is_empty() {
                host.set_host_name(t.to_string());
            }
        }
        (host.endpoint().clone(), host.ticket.clone())
    };
    p2p::wait_until_dialable(&endpoint).await?;
    ticket.iroh_ticket = iroh_tickets::endpoint::EndpointTicket::new(endpoint.addr()).to_string();
    // Keep the in-memory ticket's dial info fresh for the next copy.
    if let Some(host) = state.host.lock().as_mut() {
        host.ticket.iroh_ticket = ticket.iroh_ticket.clone();
    }
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
async fn admit_guest(
    state: State<'_, Arc<AppState>>,
    participant_id: String,
) -> Result<(), String> {
    let (take, send) = {
        let guard = state.host.lock();
        let host = guard
            .as_ref()
            .ok_or_else(|| "No active studio".to_string())?;
        host.admit(&participant_id)?;
        (host.active_take(), host.guest_send(&participant_id))
    };
    // Mid-take admit: tell the guest to start HQ capture immediately.
    if let (Some((session_id, started_at_ms)), Some(send)) = (take, send) {
        let msg = p2p::PeerMessage::RecordStart {
            session_id,
            started_at_ms,
        };
        let mut stream = send.lock().await;
        if let Err(e) = p2p::write_frame(&mut stream, &msg).await {
            tracing::warn!("late RecordStart failed: {e}");
        }
    }
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
        "invite": invite,
        "joined_at": chrono::Utc::now().to_rfc3339(),
    });
    let draft = write_draft_sync(&state, &session_id, &sync)?;
    *state.active_session.lock() = Some(session_id.clone());

    // Stay connected so the host sees us in the waiting list and can Admit.
    let (stream, host_name) =
        p2p::join_studio(&ticket, display_name.clone(), participant_id.clone()).await?;
    *state.guest_conn.lock() = Some(stream.hold_open(
        state.preview_tx.clone(),
        state.roster_tx.clone(),
        state.record_tx.clone(),
        state.aec.clone(),
    ));

    Ok(serde_json::json!({
        "session_id": session_id,
        "participant_id": participant_id,
        "studio": ticket.studio,
        "host_name": if host_name.trim().is_empty() {
            ticket.host_name
        } else {
            host_name
        },
        "connected": true,
        "invite": invite,
        "draft_path": draft.to_string_lossy(),
    }))
}

#[tauri::command]
async fn send_preview_frame(
    state: State<'_, Arc<AppState>>,
    participant_id: String,
    jpeg_b64: Option<String>,
    pcm_b64: Option<String>,
) -> Result<(), String> {
    if jpeg_b64.is_none() && pcm_b64.is_none() {
        return Ok(());
    }

    // Cancel acoustic echo against the far-end PCM we already played as reference.
    let pcm_b64 = pcm_b64.map(|pcm| state.aec.process_capture_b64(&pcm));

    // Clone send handle — never take() the session (concurrent invokes).
    let guest_send = state
        .guest_conn
        .lock()
        .as_ref()
        .map(|s| s.send_handle());
    if let Some(send) = guest_send {
        return p2p::send_preview_on(send, participant_id, jpeg_b64, pcm_b64).await;
    }

    let targets = state
        .host
        .lock()
        .as_ref()
        .map(|h| h.guest_send_handles())
        .unwrap_or_default();
    p2p::fanout_preview(targets, participant_id, jpeg_b64, pcm_b64).await
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
    eq_bands: Option<Vec<EqBand>>,
    eq_bypass: Option<bool>,
) -> Result<String, String> {
    let dir = ensure_recording_dir(&state, &session_id)?;
    let path = dir.join(format!("{participant_id}-audio.wav"));
    let eq = Some(capture::EqCaptureConfig {
        bands: eq_bands.unwrap_or_default(),
        bypass: eq_bypass.unwrap_or(false),
    });
    let recorder = AudioRecorder::start(path.clone(), device_name, eq)?;
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

fn video_writer_key(session_id: &str, participant_id: &str) -> String {
    format!("{session_id}::{participant_id}")
}

#[tauri::command]
fn save_video_track(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
    data: Vec<u8>,
    extension: String,
) -> Result<String, String> {
    // Prefer the streaming path if a writer is open — don't clobber mid-take.
    {
        let mut writers = state.video_writers.lock();
        writers.remove(&video_writer_key(&session_id, &participant_id));
    }
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
    use std::io::Write;
    let path = video_track_path(&state, &session_id, &participant_id, &extension)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let key = video_writer_key(&session_id, &participant_id);
    let mut writers = state.video_writers.lock();
    if let Some(mut prev) = writers.insert(key, file) {
        let _ = prev.flush();
    }
    Ok(path.to_string_lossy().to_string())
}

/// Append one MediaRecorder timeslice to disk — keeps RAM flat for hour-long takes.
/// Runs off the async runtime and skips per-chunk fsync so the UI (teleprompter) stays smooth.
#[tauri::command]
async fn append_video_chunk(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
    data: Vec<u8>,
    extension: String,
) -> Result<u64, String> {
    if data.is_empty() {
        return Ok(0);
    }
    let len = data.len() as u64;
    let key = video_writer_key(&session_id, &participant_id);
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        {
            let mut writers = state.video_writers.lock();
            if let Some(file) = writers.get_mut(&key) {
                file.write_all(&data).map_err(|e| e.to_string())?;
                return Ok(len);
            }
        }
        // Fallback if begin_video_track wasn't called (or writer was closed).
        let path = video_track_path(&state, &session_id, &participant_id, &extension)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(&data).map_err(|e| e.to_string())?;
        // Keep the handle open for subsequent chunks.
        state.video_writers.lock().insert(key, file);
        Ok(len)
    })
    .await
    .map_err(|e| format!("append_video_chunk join: {e}"))?
}

/// Flush and close the streaming video writer after MediaRecorder stops.
#[tauri::command]
fn finish_video_track(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
) -> Result<(), String> {
    use std::io::Write;
    let key = video_writer_key(&session_id, &participant_id);
    if let Some(mut file) = state.video_writers.lock().remove(&key) {
        file.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
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
    presence_events: Option<Vec<PresenceEvent>>,
) -> Result<SessionRecord, String> {
    let dir = ensure_recording_dir(&state, &session_id)?;

    // Prefer explicit offsets; otherwise derive from first join events.
    let mut offsets = pad_offsets_ms;
    if offsets.is_empty() {
        if let Some(events) = &presence_events {
            for ev in events {
                if ev.kind == "join" {
                    let entry = offsets.entry(ev.participant_id.clone()).or_insert(ev.t_ms);
                    *entry = (*entry).min(ev.t_ms);
                }
            }
        }
    }
    for (pid, pad) in &offsets {
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
    sync["offsets_ms"] = serde_json::to_value(&offsets).unwrap_or_default();
    sync["participants"] = serde_json::to_value(&participants).unwrap_or_default();
    if let Some(events) = &presence_events {
        sync["presence_events"] = serde_json::to_value(events).unwrap_or_default();
    }
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
    *state.guest_conn.lock() = None;
    state.recorders.lock().clear();
    *state.active_session.lock() = None;
    Ok(())
}

#[tauri::command]
async fn begin_drive_oauth(app: tauri::AppHandle) -> Result<String, String> {
    drive::begin_oauth(app).await
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
fn save_google_client_secret(secret: String) -> Result<bool, String> {
    drive::set_oauth_client_secret(&secret)?;
    Ok(drive::has_oauth_client_secret())
}

#[tauri::command]
fn google_client_secret_configured() -> bool {
    drive::has_oauth_client_secret()
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
fn get_session_timeline(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<export::SessionTimeline, String> {
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    Ok(export::load_timeline(Path::new(&session.local_path)))
}

#[tauri::command]
fn export_nle_timeline(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    resolution: Option<String>,
    fps: Option<u32>,
) -> Result<String, String> {
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    let dir = PathBuf::from(&session.local_path);
    let path = export::export_nle_timeline(
        &dir,
        &session.name,
        resolution.as_deref().unwrap_or(&session.resolution),
        fps.unwrap_or(session.frame_rate.max(1)),
    )?;
    // Reveal the export folder so the user can import the .fcpxml into Resolve.
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&path)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(parent) = path.parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
async fn render_session(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    layout: String,
    mode: Option<String>,
    resolution: Option<String>,
    fps: Option<u32>,
) -> Result<String, String> {
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| "Session not found".to_string())?;
    let dir = PathBuf::from(&session.local_path);
    let mode = mode.unwrap_or_else(|| "video".into());
    let opts = export::RenderVideoOpts {
        resolution: resolution.unwrap_or_else(|| session.resolution.clone()),
        fps: fps.unwrap_or(session.frame_rate.max(1)),
    };
    let out_dir = dir.join("exports").join(format!(
        "render-{}-{}",
        mode,
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    // Run ffmpeg off the async runtime so render-progress events can flush to the UI.
    tauri::async_runtime::spawn_blocking(move || {
        let mut on_progress = |p: export::RenderProgress| {
            let _ = app.emit("render-progress", &p);
        };
        let mut cb: export::ProgressCallback<'_> = Some(&mut on_progress);
        let master = if mode == "audio" {
            export::render_final_audio(&dir, &out_dir, &mut cb)?
        } else {
            export::render_final_video(&dir, &layout, &out_dir, &opts, &mut cb)?
        };
        Ok(master.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("render task failed: {e}"))?
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

/// Host: tell guests to show the same pre-roll countdown UI.
#[tauri::command]
async fn signal_record_countdown(
    state: State<'_, Arc<AppState>>,
    seconds: u32,
) -> Result<(), String> {
    let handles = {
        let host = state.host.lock();
        let Some(host) = host.as_ref() else {
            return Err("Not hosting a studio".into());
        };
        host.guest_send_handles()
    };
    p2p::fanout_message(
        handles,
        p2p::PeerMessage::RecordCountdown { seconds },
    )
    .await;
    Ok(())
}

/// Host: open the session inbox and tell every guest to start local HQ recording.
#[tauri::command]
async fn signal_record_start(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    let dir = ensure_recording_dir(&state, &session_id)?;
    let started_at_ms = chrono::Utc::now().timestamp_millis();
    let handles = {
        let host = state.host.lock();
        let Some(host) = host.as_ref() else {
            return Err("Not hosting a studio".into());
        };
        host.set_track_inbox(Some(dir));
        host.set_active_take(Some((session_id.clone(), started_at_ms)));
        host.guest_send_handles()
    };
    p2p::fanout_message(
        handles,
        p2p::PeerMessage::RecordStart {
            session_id,
            started_at_ms,
        },
    )
    .await;
    Ok(())
}

/// Host: tell guests to stop and begin sending tracks.
#[tauri::command]
async fn signal_record_stop(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    let (admitted, handles) = {
        let host = state.host.lock();
        let Some(host) = host.as_ref() else {
            return Err("Not hosting a studio".into());
        };
        host.set_active_take(None);
        (host.admitted_guest_ids(), host.guest_send_handles())
    };
    p2p::fanout_message(handles, p2p::PeerMessage::RecordStop).await;
    Ok(admitted)
}

fn guest_track_paths(dir: &PathBuf, participant_id: &str) -> Result<Vec<PathBuf>, String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    entries.sort();
    Ok(entries
        .into_iter()
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            (name.contains("-audio.") || name.contains("-video."))
                && name.starts_with(&format!("{participant_id}-"))
        })
        .collect())
}

fn enqueue_track_return_inner(
    state: &AppState,
    local_session_id: String,
    host_session_id: String,
    participant_id: String,
    invite: String,
) -> Result<transfer_queue::PendingTransfer, String> {
    let dir = resolve_session_dir(state, &local_session_id)?;
    let paths = guest_track_paths(&dir, &participant_id)?;
    if paths.is_empty() {
        return Err("No local audio/video tracks found to queue".into());
    }
    let mut file_names = Vec::new();
    let mut file_sizes = std::collections::HashMap::new();
    for path in &paths {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("track.bin")
            .to_string();
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        file_sizes.insert(name.clone(), len);
        file_names.push(name);
    }
    let id = format!("{host_session_id}:{participant_id}");
    let item = transfer_queue::PendingTransfer {
        id: id.clone(),
        host_session_id,
        participant_id,
        invite,
        local_session_id,
        file_names,
        file_sizes,
        offsets: Default::default(),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        attempts: 0,
        last_error: None,
    };
    transfer_queue::enqueue(item)?;
    // Return the merged row (preserves prior resume offsets).
    Ok(transfer_queue::list_pending()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| "Failed to read transfer queue".to_string())?)
}

/// Guest: enqueue local HQ tracks for durable delivery (survives leaving the lobby).
#[tauri::command]
fn enqueue_track_return(
    state: State<'_, Arc<AppState>>,
    local_session_id: String,
    host_session_id: String,
    participant_id: String,
    invite: String,
) -> Result<transfer_queue::PendingTransfer, String> {
    enqueue_track_return_inner(
        &state,
        local_session_id,
        host_session_id,
        participant_id,
        invite,
    )
}

#[tauri::command]
fn list_pending_transfers() -> Vec<transfer_queue::PendingTransfer> {
    transfer_queue::list_pending()
}

async fn push_tracks_once(
    state: &Arc<AppState>,
    session_id: &str,
    participant_id: &str,
    invite: Option<&str>,
    pending_id: &str,
) -> Result<Vec<String>, String> {
    let dir = resolve_session_dir(state, session_id)?;
    let paths = guest_track_paths(&dir, participant_id)?;
    if paths.is_empty() {
        return Err("No local audio/video tracks found to send".into());
    }

    let pending = transfer_queue::list_pending()
        .into_iter()
        .find(|p| p.id == pending_id);
    let offsets = pending
        .as_ref()
        .map(|p| p.offsets.clone())
        .unwrap_or_default();

    let live = state
        .guest_conn
        .lock()
        .as_ref()
        .map(|s| s.send_handle());

    let (_keep_endpoint, _keep_conn, _keep_recv, send) = if let Some(send) = live {
        (None, None, None, send)
    } else {
        let invite = invite
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "Host studio isn’t reachable and no invite is saved. Ask the host to reopen the studio, then tap Resume sync.".to_string()
            })?;
        let ticket = p2p::decode_ticket(invite)?;
        let stream = p2p::join_for_transfer(
            &ticket,
            participant_id.to_string(),
            participant_id.to_string(),
        )
        .await
        .map_err(|e| {
            format!(
                "Couldn’t reconnect to the host ({e}). Keep their studio open, then tap Resume sync — upload continues from where it stopped."
            )
        })?;
        let p2p::PeerStream {
            _endpoint,
            _conn,
            send,
            recv,
        } = stream;
        (
            Some(_endpoint),
            Some(_conn),
            Some(recv),
            Arc::new(tokio::sync::Mutex::new(send)),
        )
    };

    let mut sizes: Vec<u64> = Vec::with_capacity(paths.len());
    for path in &paths {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        sizes.push(len);
    }
    let grand_total: u64 = sizes.iter().sum::<u64>().max(1);
    // Count already-finished files toward overall progress.
    let mut completed_before: u64 = 0;
    for (idx, path) in paths.iter().enumerate() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("track.bin");
        let file_size = sizes.get(idx).copied().unwrap_or(0);
        let resume = offsets.get(name).copied().unwrap_or(0);
        if resume >= file_size && file_size > 0 {
            completed_before = completed_before.saturating_add(file_size);
        }
    }

    let mut sent = Vec::new();
    for (idx, path) in paths.iter().enumerate() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("track.bin")
            .to_string();
        let resume = offsets.get(&name).copied().unwrap_or(0);
        let file_size = sizes.get(idx).copied().unwrap_or(0);
        if resume >= file_size && file_size > 0 {
            sent.push(name);
            continue;
        }
        match p2p::send_file_on(
            send.clone(),
            participant_id,
            path,
            resume,
            Some((pending_id.to_string(), name.clone())),
            Some((
                state.transfer_tx.clone(),
                completed_before,
                grand_total,
            )),
        )
        .await
        {
            Ok(()) => {
                completed_before = completed_before.saturating_add(file_size);
                sent.push(name);
            }
            Err(e) => {
                return Err(format!(
                    "Upload interrupted at {} ({e}). Your files are safe — tap Resume sync to continue from {}.",
                    name,
                    humansize_bytes(resume)
                ));
            }
        }
    }
    Ok(sent)
}

fn humansize_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let x = n as f64;
    if x >= GB {
        format!("{:.2} GB", x / GB)
    } else if x >= MB {
        format!("{:.1} MB", x / MB)
    } else if x >= KB {
        format!("{:.0} KB", x / KB)
    } else {
        format!("{n} B")
    }
}

/// Guest: push tracks now (live session if connected, else reconnect with purpose=transfer).
/// Retries a few times so brief host blips don't force a manual restart.
#[tauri::command]
async fn return_local_tracks(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    participant_id: String,
    invite: Option<String>,
    host_session_id: Option<String>,
) -> Result<Vec<String>, String> {
    let pending_id = host_session_id
        .as_ref()
        .map(|h| format!("{h}:{participant_id}"))
        .unwrap_or_else(|| format!("{session_id}:{participant_id}"));

    // Ensure a durable queue row exists (and refresh file sizes) before sending.
    if let Some(host_sid) = host_session_id.clone() {
        let invite_s = invite.clone().unwrap_or_default();
        let _ = enqueue_track_return_inner(
            &state,
            session_id.clone(),
            host_sid,
            participant_id.clone(),
            invite_s,
        );
    }

    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let _ = transfer_queue::mark_attempt(&pending_id);
        let _ = state.transfer_tx.send(p2p::TransferEvent {
            participant_id: participant_id.clone(),
            file_name: String::new(),
            status: if attempt == 1 {
                "sending".into()
            } else {
                format!("retry:{attempt}")
            },
            bytes_received: 0,
            total_bytes: 0,
            overall_bytes: 0,
            overall_total: 0,
        });

        match push_tracks_once(
            &state,
            &session_id,
            &participant_id,
            invite.as_deref(),
            &pending_id,
        )
        .await
        {
            Ok(sent) => {
                let _ = transfer_queue::remove(&pending_id);
                return Ok(sent);
            }
            Err(e) => {
                last_err = e.clone();
                let _ = transfer_queue::set_error(&pending_id, e);
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        700 * u64::from(attempt),
                    ))
                    .await;
                }
            }
        }
    }
    Err(format!(
        "{last_err} (tried {MAX_ATTEMPTS} times). Leave this page open or retry from Recordings anytime — resume picks up mid-file."
    ))
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
            set_native_aec,
            get_native_aec,
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
            finish_video_track,
            finalize_session,
            discard_session,
            leave_studio,
            begin_drive_oauth,
            complete_drive_oauth,
            save_google_client_secret,
            google_client_secret_configured,
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
            get_session_timeline,
            render_session,
            export_nle_timeline,
            list_session_files,
            get_session_media,
            save_session_thumbnail,
            get_session_thumbnail,
            reveal_session_folder,
            signal_record_countdown,
            signal_record_start,
            signal_record_stop,
            enqueue_track_return,
            list_pending_transfers,
            return_local_tracks,
            send_preview_frame,
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

            // Forward peer preview frames + roster to the webview.
            let handle = app.handle().clone();
            let state = app.state::<Arc<AppState>>().inner().clone();
            let mut rx = state.preview_tx.subscribe();
            let handle_preview = handle.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = handle_preview.emit("studio-preview", &ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            let mut roster_rx = state.roster_tx.subscribe();
            let handle_roster = handle.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match roster_rx.recv().await {
                        Ok(ev) => {
                            let _ = handle_roster.emit("studio-roster", &ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            let mut record_rx = state.record_tx.subscribe();
            let handle_record = handle.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match record_rx.recv().await {
                        Ok(ev) => {
                            let _ = handle_record.emit("studio-record", &ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            let mut transfer_rx = state.transfer_tx.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    match transfer_rx.recv().await {
                        Ok(ev) => {
                            let _ = handle.emit("studio-transfer", &ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running OpenPodcast");
}

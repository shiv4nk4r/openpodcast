use chrono::Local;
use std::path::{Path, PathBuf};

pub fn app_data_dir() -> Result<PathBuf, String> {
    let dir = dirs::data_dir()
        .ok_or_else(|| "Could not resolve data directory".to_string())?
        .join("OpenPodcast");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// User-chosen save location, or the app data directory.
pub fn save_root() -> Result<PathBuf, String> {
    let dir = match crate::settings::load_settings().recording_folder {
        Some(ref folder) if !folder.trim().is_empty() => PathBuf::from(folder.trim()),
        _ => app_data_dir()?,
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Finished / in-progress takes live here.
pub fn recordings_dir() -> Result<PathBuf, String> {
    let dir = save_root()?.join("Recordings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Lobby / never-recorded studio sync files (not media folders).
pub fn drafts_dir() -> Result<PathBuf, String> {
    let dir = save_root()?.join("Drafts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Legacy UUID sessions root (pre-Recordings layout) — still scanned for old takes.
pub fn legacy_sessions_dir() -> Result<PathBuf, String> {
    let dir = match crate::settings::load_settings().recording_folder {
        Some(ref folder) if !folder.trim().is_empty() => PathBuf::from(folder.trim()).join("sessions"),
        _ => app_data_dir()?.join("sessions"),
    };
    Ok(dir)
}

pub fn models_dir() -> Result<PathBuf, String> {
    let dir = app_data_dir()?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn db_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("openpodcast.db"))
}

pub fn settings_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("settings.json"))
}

/// Safe folder/file fragment from a title.
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        let c = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if ch.is_whitespace() || matches!(ch, '-' | '_' | '.') {
            '-'
        } else {
            continue;
        };
        if c == '-' {
            if prev_dash || out.is_empty() {
                continue;
            }
            prev_dash = true;
            out.push('-');
        } else {
            prev_dash = false;
            out.push(c);
        }
        if out.len() >= 48 {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "untitled".into()
    } else {
        trimmed
    }
}

fn short_id(session_id: &str) -> &str {
    let len = session_id.len().min(8);
    &session_id[..len]
}

/// e.g. `2026-11-02_141530_check-mic-1-2-3_a1b2c3d4`
pub fn structured_recording_name(title: &str, session_id: &str) -> String {
    let ts = Local::now().format("%Y-%m-%d_%H%M%S");
    format!("{ts}_{}_{}", slugify(title), short_id(session_id))
}

/// e.g. `2026-11-02_141530_lobby_a1b2c3d4.json`
pub fn structured_draft_filename(session_id: &str) -> String {
    let ts = Local::now().format("%Y-%m-%d_%H%M%S");
    format!("{ts}_lobby_{}.json", short_id(session_id))
}

fn sync_session_id(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("session_id")?.as_str().map(|s| s.to_string())
}

/// Find an on-disk recording folder for a session id (Recordings, then legacy).
pub fn find_recording_dir(session_id: &str) -> Option<PathBuf> {
    let roots = [
        recordings_dir().ok(),
        legacy_sessions_dir().ok(),
        // Older builds used recording_folder itself as the sessions root
        crate::settings::load_settings()
            .recording_folder
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from),
    ];

    for root in roots.into_iter().flatten() {
        if !root.exists() {
            continue;
        }
        // Legacy: direct UUID child
        let legacy = root.join(session_id);
        if legacy.is_dir() && legacy.join("sync.json").exists() {
            return Some(legacy);
        }
        // Structured: scan children for matching sync.json
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let sync = dir.join("sync.json");
                if sync.is_file() {
                    if let Some(id) = sync_session_id(&sync) {
                        if id == session_id {
                            return Some(dir);
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn find_draft_file(session_id: &str) -> Option<PathBuf> {
    let root = drafts_dir().ok()?;
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(id) = sync_session_id(&path) {
                if id == session_id {
                    return Some(path);
                }
            }
        }
    }
    None
}

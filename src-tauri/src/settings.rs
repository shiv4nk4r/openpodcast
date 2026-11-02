use crate::models::AppSettings;
use crate::paths::{app_data_dir, settings_path};
use keyring::Entry;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "com.openpodcast.app";
const AI_KEY: &str = "ai_api_key";

#[derive(Debug, Default, Serialize, Deserialize)]
struct DriveTokenFile {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

fn drive_tokens_path() -> Result<std::path::PathBuf, String> {
    Ok(app_data_dir()?.join("drive_tokens.json"))
}

fn read_drive_tokens() -> DriveTokenFile {
    let Ok(path) = drive_tokens_path() else {
        return DriveTokenFile::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_drive_tokens(tokens: &DriveTokenFile) -> Result<(), String> {
    let path = drive_tokens_path()?;
    let json = serde_json::to_string_pretty(tokens).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn drive_tokens_present() -> bool {
    let t = read_drive_tokens();
    t.refresh_token
        .as_ref()
        .is_some_and(|s| !s.is_empty())
        || t.access_token.as_ref().is_some_and(|s| !s.is_empty())
}

pub fn load_settings() -> AppSettings {
    let path = match settings_path() {
        Ok(p) => p,
        Err(_) => return AppSettings::default(),
    };
    let mut settings = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        AppSettings::default()
    };
    settings.drive_connected = drive_tokens_present();
    settings.ai_has_key = get_secret(AI_KEY).is_some();
    settings
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path()?;
    let mut to_save = settings.clone();
    // Don't persist derived flags as source of truth
    to_save.drive_connected = drive_tokens_present();
    to_save.ai_has_key = get_secret(AI_KEY).is_some();
    let json = serde_json::to_string_pretty(&to_save).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub fn set_secret(key: &str, value: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
    entry.set_password(value).map_err(|e| e.to_string())
}

pub fn get_secret(key: &str) -> Option<String> {
    Entry::new(SERVICE, key)
        .ok()
        .and_then(|e| e.get_password().ok())
}

pub fn delete_secret(key: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
    let _ = entry.delete_credential();
    Ok(())
}

pub fn set_drive_access_token(token: &str) -> Result<(), String> {
    let mut t = read_drive_tokens();
    t.access_token = Some(token.to_string());
    write_drive_tokens(&t)
}

pub fn get_drive_access_token() -> Option<String> {
    read_drive_tokens()
        .access_token
        .filter(|s| !s.is_empty())
}

pub fn set_drive_refresh_token(token: &str) -> Result<(), String> {
    let mut t = read_drive_tokens();
    t.refresh_token = Some(token.to_string());
    write_drive_tokens(&t)
}

pub fn get_drive_refresh_token() -> Option<String> {
    read_drive_tokens()
        .refresh_token
        .filter(|s| !s.is_empty())
}

pub fn clear_drive_tokens() -> Result<(), String> {
    let path = drive_tokens_path()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    // Also clear any legacy keychain copies.
    let _ = delete_secret("google_drive_refresh");
    let _ = delete_secret("google_drive_access");
    Ok(())
}

pub fn set_ai_api_key(key: &str) -> Result<(), String> {
    set_secret(AI_KEY, key)
}

pub fn get_ai_api_key() -> Option<String> {
    get_secret(AI_KEY)
}

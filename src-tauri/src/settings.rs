use crate::models::AppSettings;
use crate::paths::settings_path;
use keyring::Entry;

const SERVICE: &str = "com.openpodcast.app";
const DRIVE_TOKEN_KEY: &str = "google_drive_refresh";
const AI_KEY: &str = "ai_api_key";

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
    settings.drive_connected = get_secret(DRIVE_TOKEN_KEY).is_some();
    settings.ai_has_key = get_secret(AI_KEY).is_some();
    settings
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path()?;
    let mut to_save = settings.clone();
    // Don't persist derived flags as source of truth
    to_save.drive_connected = get_secret(DRIVE_TOKEN_KEY).is_some();
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

pub fn set_drive_refresh_token(token: &str) -> Result<(), String> {
    set_secret(DRIVE_TOKEN_KEY, token)
}

pub fn get_drive_refresh_token() -> Option<String> {
    get_secret(DRIVE_TOKEN_KEY)
}

pub fn clear_drive_tokens() -> Result<(), String> {
    delete_secret(DRIVE_TOKEN_KEY)?;
    delete_secret("google_drive_access")?;
    Ok(())
}

pub fn set_ai_api_key(key: &str) -> Result<(), String> {
    set_secret(AI_KEY, key)
}

pub fn get_ai_api_key() -> Option<String> {
    get_secret(AI_KEY)
}

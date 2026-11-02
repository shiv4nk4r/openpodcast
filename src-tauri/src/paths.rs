use std::path::PathBuf;

pub fn app_data_dir() -> Result<PathBuf, String> {
    let dir = dirs::data_dir()
        .ok_or_else(|| "Could not resolve data directory".to_string())?
        .join("OpenPodcast");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn sessions_dir() -> Result<PathBuf, String> {
    let dir = app_data_dir()?.join("sessions");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn session_dir(session_id: &str) -> Result<PathBuf, String> {
    let dir = sessions_dir()?.join(session_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
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

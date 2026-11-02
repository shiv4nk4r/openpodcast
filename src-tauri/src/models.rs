use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StudioMode {
    AudioOnly,
    AudioVideo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioSettings {
    pub name: String,
    pub mode: StudioMode,
    pub frame_rate: u32,
    pub resolution: String,
}

impl Default for StudioSettings {
    fn default() -> Self {
        Self {
            name: "Episode 1".into(),
            mode: StudioMode::AudioVideo,
            frame_rate: 30,
            resolution: "1080p".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub role: String,
    pub resolution: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub name: String,
    pub mode: StudioMode,
    pub frame_rate: u32,
    pub resolution: String,
    pub created_at: DateTime<Utc>,
    pub duration_secs: f64,
    pub participants: Vec<Participant>,
    pub has_video: bool,
    pub local_path: String,
    pub drive_uploaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub display_name: String,
    pub recording_folder: Option<String>,
    pub drive_connected: bool,
    pub ai_base_url: Option<String>,
    pub ai_has_key: bool,
    pub echo_cancellation: bool,
    #[serde(default)]
    pub preferred_camera_id: Option<String>,
    #[serde(default)]
    pub preferred_mic_id: Option<String>,
    #[serde(default)]
    pub preferred_speaker_id: Option<String>,
    /// "dark" | "light" — UI appearance preference
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "light".into()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            display_name: "Host".into(),
            recording_folder: None,
            drive_connected: false,
            ai_base_url: None,
            ai_has_key: false,
            echo_cancellation: true,
            preferred_camera_id: None,
            preferred_mic_id: None,
            preferred_speaker_id: None,
            theme: default_theme(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteTicket {
    pub version: u8,
    pub host_addrs: Vec<String>,
    pub port: u16,
    pub password: String,
    pub studio: StudioSettings,
    pub host_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionCue {
    pub index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub label: String,
    pub kind: String,
}

//! Google Drive desktop OAuth (PKCE) + resumable upload.
//! Client ID is a public desktop OAuth client — users connect their own Drive.
//! Set OPENPODCAST_GOOGLE_CLIENT_ID at build/runtime, or leave unset for folder-fallback mode.

use crate::settings::{
    clear_drive_tokens, get_drive_refresh_token, set_drive_refresh_token, set_secret, get_secret,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const DRIVE_UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";
const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

fn client_id() -> String {
    std::env::var("OPENPODCAST_GOOGLE_CLIENT_ID")
        .unwrap_or_else(|_| "YOUR_GOOGLE_DESKTOP_CLIENT_ID.apps.googleusercontent.com".into())
}

pub struct PendingOauth {
    pub verifier: String,
    pub redirect_port: u16,
}

static PENDING: Mutex<Option<PendingOauth>> = Mutex::new(None);

fn pkce_pair() -> (String, String) {
    let mut rng = rand::thread_rng();
    let verifier: String = (0..64)
        .map(|_| {
            let idx = rng.gen_range(0..66);
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"[idx] as char
        })
        .collect();
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    (verifier, challenge)
}

pub fn begin_oauth() -> Result<String, String> {
    let (verifier, challenge) = pkce_pair();
    // Fixed loopback port for desktop OAuth redirect
    let port: u16 = 17834;
    *PENDING.lock().map_err(|e| e.to_string())? = Some(PendingOauth {
        verifier,
        redirect_port: port,
    });
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&access_type=offline&prompt=consent",
        urlencoding::encode(&client_id()),
        urlencoding::encode(&redirect),
        urlencoding::encode(SCOPE),
        urlencoding::encode(&challenge),
    );
    Ok(url)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

pub async fn complete_oauth_with_code(code: String) -> Result<(), String> {
    let pending = PENDING
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or_else(|| "No pending OAuth".to_string())?;
    let redirect = format!("http://127.0.0.1:{}/callback", pending.redirect_port);
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id()),
            ("code", code),
            ("code_verifier", pending.verifier),
            ("grant_type", "authorization_code".into()),
            ("redirect_uri", redirect),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("OAuth token exchange failed: {body}"));
    }
    let tokens: TokenResponse = resp.json().await.map_err(|e| e.to_string())?;
    set_secret("google_drive_access", &tokens.access_token)?;
    if let Some(refresh) = tokens.refresh_token {
        set_drive_refresh_token(&refresh)?;
    }
    let _ = tokens.expires_in;
    Ok(())
}

pub fn disconnect() -> Result<(), String> {
    clear_drive_tokens()
}

pub fn is_connected() -> bool {
    get_drive_refresh_token().is_some() || get_secret("google_drive_access").is_some()
}

async fn access_token() -> Result<String, String> {
    if let Some(t) = get_secret("google_drive_access") {
        return Ok(t);
    }
    let refresh = get_drive_refresh_token().ok_or_else(|| "Drive not connected".to_string())?;
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id()),
            ("refresh_token", refresh),
            ("grant_type", "refresh_token".into()),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err("Failed to refresh Drive token".into());
    }
    let tokens: TokenResponse = resp.json().await.map_err(|e| e.to_string())?;
    set_secret("google_drive_access", &tokens.access_token)?;
    Ok(tokens.access_token)
}

async fn ensure_folder(access: &str, name: &str, parent: Option<&str>) -> Result<String, String> {
    let client = reqwest::Client::new();
    let q = if let Some(p) = parent {
        format!(
            "name = '{}' and '{}' in parents and mimeType = 'application/vnd.google-apps.folder' and trashed = false",
            name.replace('\'', "\\'"),
            p
        )
    } else {
        format!(
            "name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false and 'root' in parents",
            name.replace('\'', "\\'")
        )
    };
    let list: serde_json::Value = client
        .get(format!("{DRIVE_API}/files"))
        .bearer_auth(access)
        .query(&[("q", q.as_str()), ("fields", "files(id,name)")])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(id) = list["files"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|f| f["id"].as_str())
    {
        return Ok(id.to_string());
    }
    let mut meta = serde_json::json!({
        "name": name,
        "mimeType": "application/vnd.google-apps.folder"
    });
    if let Some(p) = parent {
        meta["parents"] = serde_json::json!([p]);
    }
    let created: serde_json::Value = client
        .post(format!("{DRIVE_API}/files"))
        .bearer_auth(access)
        .json(&meta)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    created["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Failed to create Drive folder".into())
}

pub async fn upload_file(
    path: &Path,
    session_name: &str,
    on_progress: impl Fn(f32),
) -> Result<String, String> {
    let access = access_token().await?;
    let root = ensure_folder(&access, "OpenPodcast", None).await?;
    let folder = ensure_folder(&access, session_name, Some(&root)).await?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("upload.bin")
        .to_string();
    let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
    let client = reqwest::Client::new();

    let meta = serde_json::json!({
        "name": file_name,
        "parents": [folder]
    });
    let start = client
        .post(format!("{DRIVE_UPLOAD}?uploadType=resumable"))
        .bearer_auth(&access)
        .header("X-Upload-Content-Type", "application/octet-stream")
        .header("X-Upload-Content-Length", bytes.len().to_string())
        .json(&meta)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let session_url = start
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| "Missing resumable upload URL".to_string())?
        .to_string();

    // Single PUT for simplicity; still resumable-session compatible
    on_progress(0.1);
    let put = client
        .put(&session_url)
        .header("Content-Length", bytes.len().to_string())
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !put.status().is_success() {
        let body = put.text().await.unwrap_or_default();
        return Err(format!("Upload failed: {body}"));
    }
    on_progress(1.0);
    let body: serde_json::Value = put.json().await.unwrap_or_default();
    Ok(body["id"].as_str().unwrap_or("").to_string())
}

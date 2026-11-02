//! Google Drive desktop OAuth (PKCE) + resumable upload.
//! Desktop OAuth client ID + secret ship with the app (Google treats Desktop
//! clients as public). Override with OPENPODCAST_GOOGLE_CLIENT_ID / _SECRET.

use crate::settings::{
    clear_drive_tokens, get_drive_access_token, get_drive_refresh_token, get_secret,
    load_settings, save_settings, set_drive_access_token, set_drive_refresh_token, set_secret,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const DRIVE_UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";
const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const CLIENT_SECRET_KEY: &str = "google_oauth_client_secret";

/// Public Desktop OAuth client ID. Override with OPENPODCAST_GOOGLE_CLIENT_ID.
const DEFAULT_GOOGLE_CLIENT_ID: &str =
    "358949045753-n0eojdnq1gsq2gndhhvld41ou5vncrcd.apps.googleusercontent.com";

/// Desktop OAuth client secret (distributed with the app). Override with OPENPODCAST_GOOGLE_CLIENT_SECRET.
const DEFAULT_GOOGLE_CLIENT_SECRET: &str = "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";

fn client_id() -> Result<String, String> {
    let id = std::env::var("OPENPODCAST_GOOGLE_CLIENT_ID")
        .unwrap_or_else(|_| DEFAULT_GOOGLE_CLIENT_ID.to_string());
    let id = id.trim().to_string();
    if id.is_empty() || id.starts_with("YOUR_GOOGLE") || !id.contains(".apps.googleusercontent.com")
    {
        return Err(
            "Google Drive is not configured. Create a Desktop OAuth client in Google Cloud Console."
                .into(),
        );
    }
    Ok(id)
}

fn client_secret() -> Result<String, String> {
    if let Ok(s) = std::env::var("OPENPODCAST_GOOGLE_CLIENT_SECRET") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    if let Some(s) = get_secret(CLIENT_SECRET_KEY) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    Ok(DEFAULT_GOOGLE_CLIENT_SECRET.to_string())
}

pub fn set_oauth_client_secret(secret: &str) -> Result<(), String> {
    let s = secret.trim();
    if s.is_empty() {
        return Err("Client secret cannot be empty".into());
    }
    set_secret(CLIENT_SECRET_KEY, s)
}

pub fn has_oauth_client_secret() -> bool {
    client_secret().is_ok()
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingOauth {
    pub verifier: String,
    pub redirect_port: u16,
}

static PENDING: Mutex<Option<PendingOauth>> = Mutex::new(None);

fn pending_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::paths::app_data_dir()?.join("drive_oauth_pending.json"))
}

fn store_pending(pending: &PendingOauth) -> Result<(), String> {
    *PENDING.lock().map_err(|e| e.to_string())? = Some(pending.clone());
    let path = pending_path()?;
    let json = serde_json::to_string(pending).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn load_pending() -> Option<PendingOauth> {
    if let Ok(guard) = PENDING.lock() {
        if let Some(p) = guard.clone() {
            return Some(p);
        }
    }
    let path = pending_path().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn clear_pending() {
    let _ = PENDING.lock().map(|mut g| g.take());
    if let Ok(path) = pending_path() {
        let _ = std::fs::remove_file(path);
    }
}

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

const OAUTH_PORT: u16 = 17834;

pub async fn begin_oauth(app: AppHandle) -> Result<String, String> {
    let _ = client_secret()?; // fail early with a clear message
    let client_id = client_id()?;
    let (verifier, challenge) = pkce_pair();

    let listener = TcpListener::bind(("127.0.0.1", OAUTH_PORT))
        .await
        .map_err(|e| {
            format!(
                "Cannot listen on 127.0.0.1:{OAUTH_PORT} ({e}). Quit any other OpenPodcast window, then try again."
            )
        })?;

    store_pending(&PendingOauth {
        verifier,
        redirect_port: OAUTH_PORT,
    })?;
    spawn_oauth_accept(app, listener);

    let redirect = format!("http://127.0.0.1:{OAUTH_PORT}/callback");
    let url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&access_type=offline&prompt=consent",
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect),
        urlencoding::encode(SCOPE),
        urlencoding::encode(&challenge),
    );
    Ok(url)
}

/// Accept a raw OAuth code or a full redirect URL containing `?code=`.
pub fn extract_oauth_code(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Paste the OAuth code (or the full redirect URL)".into());
    }
    if let Ok(u) = url::Url::parse(t) {
        for (k, v) in u.query_pairs() {
            if k == "code" && !v.is_empty() {
                return Ok(v.to_string());
            }
            if k == "error" {
                return Err(format!("Google OAuth error: {v}"));
            }
        }
    }
    if let Some(idx) = t.find("code=") {
        let rest = &t[idx + 5..];
        let code = rest
            .split('&')
            .next()
            .unwrap_or(rest)
            .trim()
            .trim_end_matches(|c: char| c == '"' || c == '\'');
        // Decode percent-encoding if the user pasted a raw query fragment.
        let code = urlencoding::decode(code)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| code.to_string());
        if !code.is_empty() {
            return Ok(code);
        }
    }
    if !t.contains(' ') && !t.contains("://") {
        return Ok(t.to_string());
    }
    Err("Could not find ?code= in that value".into())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn spawn_oauth_accept(app: AppHandle, listener: TcpListener) {
    tauri::async_runtime::spawn(async move {
        tracing::info!("Drive OAuth callback ready on 127.0.0.1:{OAUTH_PORT}");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5 * 60);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                let _ = app.emit(
                    "drive-oauth-done",
                    serde_json::json!({
                        "ok": false,
                        "error": "Timed out waiting for Google sign-in",
                    }),
                );
                return;
            }
            let accept = tokio::time::timeout(left, listener.accept()).await;
            let (mut socket, _) = match accept {
                Ok(Ok(pair)) => pair,
                Ok(Err(e)) => {
                    let _ = app.emit(
                        "drive-oauth-done",
                        serde_json::json!({ "ok": false, "error": e.to_string() }),
                    );
                    return;
                }
                Err(_) => {
                    let _ = app.emit(
                        "drive-oauth-done",
                        serde_json::json!({
                            "ok": false,
                            "error": "Timed out waiting for Google sign-in",
                        }),
                    );
                    return;
                }
            };

            let mut buf = vec![0u8; 16384];
            let n = match socket.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => continue,
            };
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");
            let path = first_line.split_whitespace().nth(1).unwrap_or("");
            if !path.contains("/callback") || !path.contains("code=") {
                let body = "Not a Google OAuth callback";
                let resp = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
                continue;
            }

            let result = match extract_oauth_code(&format!("http://127.0.0.1{path}")) {
                Ok(code) => complete_oauth_with_code(code).await,
                Err(e) => Err(e),
            };

            let (status, body) = match &result {
                Ok(()) => (
                    "200 OK",
                    "<!doctype html><html><body style=\"font-family:system-ui;padding:2rem\">\
                     <h1>Google Drive connected</h1>\
                     <p>You can close this tab and return to OpenPodcast.</p>\
                     </body></html>"
                        .to_string(),
                ),
                Err(e) => (
                    "400 Bad Request",
                    format!(
                        "<!doctype html><html><body style=\"font-family:system-ui;padding:2rem\">\
                         <h1>Could not finish Google Drive</h1>\
                         <p style=\"color:#b00020\"><code>{}</code></p>\
                         <p>You can also paste the full redirect URL into OpenPodcast → Settings.</p>\
                         </body></html>",
                        html_escape(e)
                    ),
                ),
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.shutdown().await;

            match result {
                Ok(()) => {
                    let mut s = load_settings();
                    s.drive_connected = true;
                    let _ = save_settings(&s);
                    let _ = app.emit("drive-oauth-done", serde_json::json!({ "ok": true }));
                }
                Err(e) => {
                    let _ = app.emit(
                        "drive-oauth-done",
                        serde_json::json!({ "ok": false, "error": e }),
                    );
                }
            }
            return;
        }
    });
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

pub async fn complete_oauth_with_code(code_or_url: String) -> Result<(), String> {
    let code = extract_oauth_code(&code_or_url)?;
    let pending = load_pending().ok_or_else(|| {
        "No pending Google sign-in. Click Connect Google Drive again, then paste the new redirect URL."
            .to_string()
    })?;
    let redirect = format!("http://127.0.0.1:{}/callback", pending.redirect_port);
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id()?),
            ("client_secret", client_secret()?),
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
    set_drive_access_token(&tokens.access_token)?;
    if let Some(refresh) = tokens.refresh_token {
        set_drive_refresh_token(&refresh)?;
    } else if get_drive_refresh_token().is_none() {
        eprintln!("Google Drive OAuth: no refresh_token in response; using access token only");
    }
    let _ = tokens.expires_in;
    // Clear pending only after success so paste-retry still works on failure.
    clear_pending();
    let mut s = load_settings();
    s.drive_connected = true;
    save_settings(&s)?;
    Ok(())
}

pub fn disconnect() -> Result<(), String> {
    clear_drive_tokens()
}

pub fn is_connected() -> bool {
    get_drive_refresh_token().is_some() || get_drive_access_token().is_some()
}

async fn access_token() -> Result<String, String> {
    if let Some(t) = get_drive_access_token() {
        return Ok(t);
    }
    let refresh = get_drive_refresh_token().ok_or_else(|| "Drive not connected".to_string())?;
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id()?),
            ("client_secret", client_secret()?),
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
    set_drive_access_token(&tokens.access_token)?;
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

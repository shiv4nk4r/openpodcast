//! Subtitle generation via the user's OpenAI-compatible AI provider
//! (`/audio/transcriptions`) or a local whisper.cpp binary when available.
//! Never invents fake "X speaking" text — callers get real cues or a clear error.

use crate::models::CaptionCue;
use crate::paths::models_dir;
use crate::settings::{get_ai_api_key, load_settings};
use std::path::{Path, PathBuf};
use std::process::Command;

const WHISPER_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";

pub fn model_path() -> Result<PathBuf, String> {
    Ok(models_dir()?.join("ggml-base.en.bin"))
}

pub fn model_ready() -> bool {
    model_path().map(|p| p.exists()).unwrap_or(false)
}

pub async fn download_model(on_progress: impl Fn(f32)) -> Result<PathBuf, String> {
    let path = model_path()?;
    if path.exists() {
        on_progress(1.0);
        return Ok(path);
    }
    let client = reqwest::Client::new();
    let resp = client
        .get(WHISPER_MODEL_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if total > 0 {
        on_progress(0.9);
    }
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    on_progress(1.0);
    Ok(path)
}

fn find_whisper_cli() -> Option<PathBuf> {
    which("whisper-cli")
        .or_else(|| which("whisper-cpp"))
        .or_else(|| which("main")) // older whisper.cpp binary name
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn ai_configured() -> bool {
    let settings = load_settings();
    settings
        .ai_base_url
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        && get_ai_api_key().is_some()
}

pub async fn transcribe_wav_ai_only(
    wav_path: &Path,
    speaker: &str,
) -> Result<Vec<CaptionCue>, String> {
    if !ai_configured() {
        return Err(
            "No AI provider configured. Add an OpenAI-compatible base URL + API key in Settings \
             (must support /audio/transcriptions), or use Generate on device."
                .into(),
        );
    }
    cloud_transcribe(wav_path, speaker).await
}

pub async fn transcribe_wav(
    wav_path: &Path,
    speaker: &str,
) -> Result<Vec<CaptionCue>, String> {
    // 1) Prefer the user's configured OpenAI-compatible provider
    if ai_configured() {
        return cloud_transcribe(wav_path, speaker).await;
    }

    // 2) Local whisper.cpp if model + CLI are present
    if let Some(cli) = find_whisper_cli() {
        if model_ready() {
            let model = model_path()?;
            let out_prefix = wav_path.with_extension("whisper");
            let status = Command::new(cli)
                .args([
                    "-m",
                    model.to_str().unwrap_or_default(),
                    "-f",
                    wav_path.to_str().unwrap_or_default(),
                    "-otxt",
                    "-of",
                    out_prefix.to_str().unwrap_or_default(),
                ])
                .status()
                .map_err(|e| e.to_string())?;
            if status.success() {
                let txt = out_prefix.with_extension("txt");
                if let Ok(content) = std::fs::read_to_string(txt) {
                    let cues = cues_from_plain(speaker, &content);
                    if !cues.is_empty() {
                        return Ok(cues);
                    }
                }
            }
            return Err(
                "Local Whisper ran but produced no text. Check the audio track or try AI / on-device captions."
                    .into(),
            );
        }
    }

    Err(
        "No AI provider configured. Use Generate on device (Apple/Windows Speech), add an \
         OpenAI-compatible API in Settings, or type subtitles manually."
            .into(),
    )
}

async fn cloud_transcribe(wav_path: &Path, speaker: &str) -> Result<Vec<CaptionCue>, String> {
    let settings = load_settings();
    let base = settings
        .ai_base_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "AI base URL missing. Set it in Settings.".to_string())?;
    let key =
        get_ai_api_key().ok_or_else(|| "AI API key missing. Set it in Settings.".to_string())?;

    let bytes = tokio::fs::read(wav_path).await.map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("Audio file is empty".into());
    }
    let file_name = wav_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio.wav");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;

    // Prefer verbose_json for timed segments; fall back to plain text.
    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .text("response_format", "verbose_json")
        .text("timestamp_granularities[]", "segment")
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(file_name.to_string())
                .mime_str("audio/wav")
                .map_err(|e| e.to_string())?,
        );

    let url = format!("{}/audio/transcriptions", base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(&key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            format!(
                "Could not reach AI provider at {url}: {e}. \
                 Use an OpenAI-compatible endpoint that supports /audio/transcriptions (e.g. OpenAI)."
            )
        })?;

    let status = resp.status();
    let body_text = resp.text().await.map_err(|e| e.to_string())?;

    if !status.is_success() {
        // Some local servers only support json/text — retry once with plain json
        if status.as_u16() == 400 || status.as_u16() == 422 {
            return cloud_transcribe_plain(wav_path, speaker, &base, &key, bytes, file_name).await;
        }
        return Err(format!(
            "AI transcription failed ({status}): {}",
            truncate(&body_text, 280)
        ));
    }

    let body: serde_json::Value =
        serde_json::from_str(&body_text).map_err(|e| format!("Invalid AI response: {e}"))?;

    if let Some(segments) = body["segments"].as_array() {
        let mut cues = Vec::new();
        for (i, seg) in segments.iter().enumerate() {
            let text = seg["text"].as_str().unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            let start_s = seg["start"].as_f64().unwrap_or(0.0);
            let end_s = seg["end"].as_f64().unwrap_or(start_s + 2.0);
            cues.push(CaptionCue {
                index: i + 1,
                start_ms: (start_s * 1000.0).round() as u64,
                end_ms: (end_s * 1000.0).round().max(start_s * 1000.0 + 200.0) as u64,
                speaker: speaker.to_string(),
                text: text.to_string(),
            });
        }
        if !cues.is_empty() {
            return Ok(cues);
        }
    }

    let text = body["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return Err("AI returned an empty transcript".into());
    }
    Ok(cues_from_plain(speaker, text))
}

async fn cloud_transcribe_plain(
    wav_path: &Path,
    speaker: &str,
    base: &str,
    key: &str,
    bytes: Vec<u8>,
    file_name: &str,
) -> Result<Vec<CaptionCue>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes)
                .file_name(file_name.to_string())
                .mime_str("audio/wav")
                .map_err(|e| e.to_string())?,
        );
    let url = format!("{}/audio/transcriptions", base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("AI request failed: {e}"))?;
    let status = resp.status();
    let body_text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "AI transcription failed ({status}): {}. \
             Your provider must support OpenAI-style /v1/audio/transcriptions (Whisper).",
            truncate(&body_text, 280)
        ));
    }
    let body: serde_json::Value =
        serde_json::from_str(&body_text).map_err(|e| format!("Invalid AI response: {e}"))?;
    let text = body["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        // Some servers return the string directly
        let plain = body_text.trim().trim_matches('"');
        if plain.is_empty() || plain.starts_with('{') {
            let _ = wav_path; // silence unused in some builds
            return Err("AI returned an empty transcript".into());
        }
        return Ok(cues_from_plain(speaker, plain));
    }
    Ok(cues_from_plain(speaker, text))
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(max).collect::<String>())
    }
}

pub(crate) fn cues_from_plain(speaker: &str, text: &str) -> Vec<CaptionCue> {
    let chunks: Vec<&str> = text
        .split(|c| c == '.' || c == '?' || c == '!')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if chunks.is_empty() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return vec![];
        }
        return vec![CaptionCue {
            index: 1,
            start_ms: 0,
            end_ms: 3500,
            speaker: speaker.to_string(),
            text: trimmed.to_string(),
        }];
    }
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, t)| CaptionCue {
            index: i + 1,
            start_ms: (i as u64) * 4000,
            end_ms: (i as u64) * 4000 + 3500,
            speaker: speaker.to_string(),
            text: t.to_string(),
        })
        .collect()
}

pub fn merge_cues(mut all: Vec<CaptionCue>) -> Vec<CaptionCue> {
    all.sort_by_key(|c| c.start_ms);
    for (i, c) in all.iter_mut().enumerate() {
        c.index = i + 1;
    }
    all
}

pub fn cues_to_srt(cues: &[CaptionCue]) -> String {
    let mut out = String::new();
    for c in cues {
        out.push_str(&format!(
            "{}\n{} --> {}\n[{}] {}\n\n",
            c.index,
            format_ts(c.start_ms),
            format_ts(c.end_ms),
            c.speaker,
            c.text
        ));
    }
    out
}

/// Best-effort parse of our own SRT (and simple variants).
pub fn parse_srt(raw: &str) -> Vec<CaptionCue> {
    let mut cues = Vec::new();
    let blocks = raw.replace("\r\n", "\n");
    for block in blocks.split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.len() < 2 {
            continue;
        }
        let (ts_line, text_start) = if lines[0].chars().all(|c| c.is_ascii_digit()) && lines.len() >= 3
        {
            (lines[1], 2)
        } else {
            (lines[0], 1)
        };
        let Some((start_ms, end_ms)) = parse_srt_range(ts_line) else {
            continue;
        };
        let mut text = lines[text_start..].join(" ").trim().to_string();
        let mut speaker = "Host".to_string();
        if let Some(rest) = text.strip_prefix('[') {
            if let Some((sp, body)) = rest.split_once(']') {
                speaker = sp.trim().to_string();
                text = body.trim().to_string();
            }
        }
        if text.is_empty() {
            continue;
        }
        cues.push(CaptionCue {
            index: cues.len() + 1,
            start_ms,
            end_ms,
            speaker,
            text,
        });
    }
    cues
}

fn parse_srt_range(line: &str) -> Option<(u64, u64)> {
    let (a, b) = line.split_once("-->")?;
    Some((parse_srt_ts(a.trim())?, parse_srt_ts(b.trim())?))
}

fn parse_srt_ts(s: &str) -> Option<u64> {
    // HH:MM:SS,mmm or HH:MM:SS.mmm
    let s = s.replace(',', ".");
    let (hms, ms_part) = s.split_once('.')?;
    let mut parts = hms.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let sec: u64 = parts.next()?.parse().ok()?;
    let millis: u64 = ms_part
        .chars()
        .take(3)
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    Some(h * 3_600_000 + m * 60_000 + sec * 1000 + millis)
}

fn format_ts(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1000;
    let millis = ms % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

pub async fn generate_show_notes(transcript: &str) -> Result<String, String> {
    let settings = load_settings();
    let base = settings
        .ai_base_url
        .ok_or_else(|| "Connect an AI provider in Settings".to_string())?;
    let key = get_ai_api_key().ok_or_else(|| "Missing AI API key".to_string())?;
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [
            {"role": "system", "content": "Write concise podcast show notes with a title, 3-5 bullets, and timestamps if present."},
            {"role": "user", "content": transcript}
        ]
    });
    let resp = client
        .post(url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err("AI request failed".into());
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

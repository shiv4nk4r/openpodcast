//! On-device speech-to-text for captions.
//! - macOS: Apple Speech framework helper (SFSpeechRecognizer)
//! - Windows: System.Speech via PowerShell
//! Audio longer than ~50s is chunked in Rust (platform STT limits).

use crate::models::CaptionCue;
use crate::whisper::{cues_from_plain, merge_cues};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHUNK_SECS: u32 = 50;
const CHUNK_OVERLAP_SECS: u32 = 1;

#[derive(Debug, Deserialize)]
struct NativeSeg {
    text: String,
    start: f64,
    end: f64,
}

#[derive(Debug, Deserialize)]
struct NativeOut {
    #[allow(dead_code)]
    text: String,
    #[serde(default)]
    segments: Vec<NativeSeg>,
}

pub fn available() -> bool {
    cfg!(target_os = "macos") || cfg!(target_os = "windows")
}

pub fn engine_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Apple Speech"
    }
    #[cfg(target_os = "windows")]
    {
        "Windows Speech"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Native STT"
    }
}

pub async fn transcribe_wav(wav_path: &Path, speaker: &str) -> Result<Vec<CaptionCue>, String> {
    let wav_path = wav_path.to_path_buf();
    let speaker = speaker.to_string();
    tokio::task::spawn_blocking(move || transcribe_wav_blocking(&wav_path, &speaker))
        .await
        .map_err(|e| e.to_string())?
}

fn transcribe_wav_blocking(wav_path: &Path, speaker: &str) -> Result<Vec<CaptionCue>, String> {
    if !wav_path.exists() {
        return Err(format!("Audio not found: {}", wav_path.display()));
    }

    // Apple/Windows STT are unreliable on multi-channel / high-rate WAVs from
    // interface mics (e.g. 3ch @ 48k). Always feed mono 16 kHz PCM.
    let prepared = prepare_wav_mono_16k(wav_path)?;
    let prepared_path = prepared.as_path();

    let duration_secs = wav_duration_secs(prepared_path)?;
    if duration_secs <= 0.05 {
        let _ = cleanup_temp(prepared_path, wav_path);
        return Err("Audio track is empty".into());
    }

    let result = if duration_secs <= f64::from(CHUNK_SECS) + 0.5 {
        run_native_file(prepared_path, speaker, 0.0)
    } else {
        let chunks = split_wav_chunks(prepared_path)?;
        let mut all = Vec::new();
        let mut errors = Vec::new();
        for (offset_secs, chunk_path) in &chunks {
            match run_native_file(chunk_path, speaker, *offset_secs) {
                Ok(cues) => all.extend(cues),
                Err(e) => errors.push(format!("@{offset_secs:.0}s: {e}")),
            }
            let _ = std::fs::remove_file(chunk_path);
        }
        if all.is_empty() {
            Err(if errors.is_empty() {
                format!(
                    "{} produced no captions. Speak clearly near the mic, or type subtitles manually.",
                    engine_label()
                )
            } else {
                errors.join("\n")
            })
        } else {
            Ok(merge_cues(all))
        }
    };

    let _ = cleanup_temp(prepared_path, wav_path);
    result
}

fn cleanup_temp(prepared: &Path, original: &Path) -> Result<(), ()> {
    if prepared != original {
        let _ = std::fs::remove_file(prepared);
    }
    Ok(())
}

/// Downmix to mono and resample to 16 kHz 16-bit PCM for native STT engines.
fn prepare_wav_mono_16k(path: &Path) -> Result<PathBuf, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if samples.is_empty() {
        return Err("Audio track is empty".into());
    }

    let ch = spec.channels.max(1) as usize;
    let sr = spec.sample_rate.max(1);
    let target_sr = 16_000u32;

    // Already mono 16k — reuse original
    if ch == 1 && sr == target_sr && spec.sample_format == hound::SampleFormat::Int && spec.bits_per_sample == 16
    {
        return Ok(path.to_path_buf());
    }

    // Downmix to mono
    let mono: Vec<f32> = if ch == 1 {
        samples.iter().map(|s| *s as f32).collect()
    } else {
        samples
            .chunks_exact(ch)
            .map(|frame| {
                let sum: f32 = frame.iter().map(|s| *s as f32).sum();
                sum / ch as f32
            })
            .collect()
    };

    // Linear resample to 16 kHz
    let out_len = ((mono.len() as u64) * u64::from(target_sr) / u64::from(sr)) as usize;
    let out_len = out_len.max(1);
    let mut resampled = Vec::with_capacity(out_len);
    if sr == target_sr {
        resampled.extend(mono.iter().map(|s| s.round().clamp(-32768.0, 32767.0) as i16));
    } else {
        for i in 0..out_len {
            let src = i as f64 * f64::from(sr) / f64::from(target_sr);
            let i0 = src.floor() as usize;
            let i1 = (i0 + 1).min(mono.len().saturating_sub(1));
            let frac = (src - i0 as f64) as f32;
            let v = mono[i0] * (1.0 - frac) + mono[i1] * frac;
            resampled.push(v.round().clamp(-32768.0, 32767.0) as i16);
        }
    }

    let out_dir = std::env::temp_dir().join("openpodcast_stt");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let out_path = out_dir.join(format!(
        "{}-mono16k.wav",
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("track")
    ));
    let out_spec = hound::WavSpec {
        channels: 1,
        sample_rate: target_sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    {
        let mut w = hound::WavWriter::create(&out_path, out_spec).map_err(|e| e.to_string())?;
        for s in resampled {
            w.write_sample(s).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    Ok(out_path)
}

fn run_native_file(
    wav_path: &Path,
    speaker: &str,
    offset_secs: f64,
) -> Result<Vec<CaptionCue>, String> {
    let json = {
        #[cfg(target_os = "macos")]
        {
            run_macos(wav_path)?
        }
        #[cfg(target_os = "windows")]
        {
            run_windows(wav_path)?
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = wav_path;
            return Err("Native on-device captions are only available on macOS and Windows".into());
        }
    };

    let parsed: NativeOut = serde_json::from_str(json.trim()).map_err(|e| {
        format!(
            "{} returned invalid JSON ({e}): {}",
            engine_label(),
            truncate(json.trim(), 200)
        )
    })?;

    if !parsed.segments.is_empty() {
        let cues: Vec<CaptionCue> = parsed
            .segments
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .enumerate()
            .map(|(i, s)| {
                let start_ms = ((s.start + offset_secs) * 1000.0).round().max(0.0) as u64;
                let end_ms = ((s.end + offset_secs) * 1000.0)
                    .round()
                    .max(start_ms as f64 + 200.0) as u64;
                CaptionCue {
                    index: i + 1,
                    start_ms,
                    end_ms,
                    speaker: speaker.to_string(),
                    text: s.text.trim().to_string(),
                }
            })
            .collect();
        if !cues.is_empty() {
            return Ok(cues);
        }
    }

    if parsed.text.trim().is_empty() {
        return Err(format!("{} recognized no speech", engine_label()));
    }

    // Fallback: sentence-split plain text, shift by offset
    let mut cues = cues_from_plain(speaker, parsed.text.trim());
    for c in &mut cues {
        c.start_ms += (offset_secs * 1000.0).round() as u64;
        c.end_ms += (offset_secs * 1000.0).round() as u64;
    }
    Ok(cues)
}

#[cfg(target_os = "macos")]
fn run_macos(wav_path: &Path) -> Result<String, String> {
    let helper = helper_macos().ok_or_else(|| {
        "macOS speech helper missing. Rebuild the app (swiftc required) or use Generate with AI."
            .to_string()
    })?;
    let output = Command::new(&helper)
        .arg(wav_path)
        .output()
        .map_err(|e| format!("Failed to launch Apple Speech helper: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!(
                "no error text (exit {}). If this is the first run, allow Speech Recognition for OpenPodcast in System Settings → Privacy & Security → Speech Recognition.",
                output.status.code().unwrap_or(-1)
            )
        };
        return Err(format!(
            "Apple Speech failed (exit {}): {detail}",
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(target_os = "macos")]
fn helper_macos() -> Option<PathBuf> {
    // Prefer app-data helper (rebuilds when Swift source changes)
    if let Ok(bin) = compile_macos_helper() {
        return Some(bin);
    }
    // Build-time helper from cargo OUT_DIR
    if let Some(p) = option_env!("NATIVE_STT_MACOS_HELPER") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    // Next to the running executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("macos_transcribe");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn compile_macos_helper() -> Result<PathBuf, String> {
    let out_dir = crate::paths::models_dir()?.join("native_stt");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let bin = out_dir.join("macos_transcribe");
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native_stt/macos_transcribe.swift");
    if !src.exists() {
        return Err("macos_transcribe.swift not found".into());
    }
    // Rebuild when source is newer than the binary
    let needs_build = match (std::fs::metadata(&bin), std::fs::metadata(&src)) {
        (Ok(b), Ok(s)) => {
            let bt = b.modified().ok();
            let st = s.modified().ok();
            match (bt, st) {
                (Some(bt), Some(st)) => st > bt,
                _ => true,
            }
        }
        _ => true,
    };
    if needs_build {
        let status = Command::new("swiftc")
            .args([
                "-O",
                "-framework",
                "Speech",
                "-framework",
                "AVFoundation",
                "-o",
            ])
            .arg(&bin)
            .arg(&src)
            .status()
            .map_err(|e| format!("swiftc unavailable: {e}"))?;
        if !status.success() {
            return Err("swiftc failed to build Apple Speech helper".into());
        }
    }
    // Also place next to the running binary for easier TCC prompting
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("macos_transcribe");
            let _ = std::fs::copy(&bin, &sibling);
        }
    }
    Ok(bin)
}

#[cfg(target_os = "windows")]
fn run_windows(wav_path: &Path) -> Result<String, String> {
    let ps = which_powershell().ok_or_else(|| "PowerShell not found on PATH".to_string())?;
    let script = include_str!("../native_stt/windows_transcribe.ps1");
    let dir = std::env::temp_dir().join("openpodcast_native_stt");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script_path = dir.join("windows_transcribe.ps1");
    std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
    let output = Command::new(ps)
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .arg(wav_path)
        .output()
        .map_err(|e| format!("Failed to launch Windows Speech: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let err = if err.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).to_string()
        } else {
            err.to_string()
        };
        return Err(format!("Windows Speech failed: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(target_os = "windows")]
fn which_powershell() -> Option<PathBuf> {
    for name in ["pwsh", "powershell"] {
        if let Some(p) = which(name) {
            return Some(p);
        }
    }
    // Common install paths
    let candidates = [
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        r"C:\Program Files\PowerShell\7\pwsh.exe",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        let exe = dir.join(format!("{name}.exe"));
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

fn wav_duration_secs(path: &Path) -> Result<f64, String> {
    let reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let len = reader.len() as f64;
    let ch = spec.channels.max(1) as f64;
    let sr = spec.sample_rate.max(1) as f64;
    Ok(len / ch / sr)
}

fn split_wav_chunks(path: &Path) -> Result<Vec<(f64, PathBuf)>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let ch = spec.channels as usize;
    let sr = spec.sample_rate as usize;
    let frame = ch; // samples per time sample across channels
    let chunk_frames = sr * CHUNK_SECS as usize;
    let overlap_frames = sr * CHUNK_OVERLAP_SECS as usize;
    let total_frames = samples.len() / frame;

    let out_dir = std::env::temp_dir().join("openpodcast_stt_chunks");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    let mut start_frame = 0usize;
    let mut idx = 0usize;
    while start_frame < total_frames {
        let end_frame = (start_frame + chunk_frames).min(total_frames);
        let start_sample = start_frame * frame;
        let end_sample = end_frame * frame;
        let slice = &samples[start_sample..end_sample];
        let chunk_path = out_dir.join(format!("chunk-{idx}.wav"));
        {
            let mut w = hound::WavWriter::create(&chunk_path, spec).map_err(|e| e.to_string())?;
            for s in slice {
                w.write_sample(*s).map_err(|e| e.to_string())?;
            }
            w.finalize().map_err(|e| e.to_string())?;
        }
        let offset = start_frame as f64 / sr as f64;
        out.push((offset, chunk_path));
        idx += 1;
        if end_frame >= total_frames {
            break;
        }
        let next = end_frame.saturating_sub(overlap_frames);
        start_frame = if next > start_frame { next } else { end_frame };
    }
    Ok(out)
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(max).collect::<String>())
    }
}

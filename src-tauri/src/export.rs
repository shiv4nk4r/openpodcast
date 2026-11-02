use crate::models::CaptionCue;
use crate::whisper::cues_to_srt;
use std::path::{Path, PathBuf};

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ExportRequest {
    pub session_id: String,
    pub layout: String,
    pub include_captions: bool,
    pub trim_start_ms: u64,
    pub trim_end_ms: Option<u64>,
}

/// Export session: copy/compose tracks into an exports folder + optional SRT.
/// Full video remux of multiple H.264 tracks without ffmpeg is limited; we
/// produce a deliverable package (audio mix WAV + SRT + sync.json + media copies)
/// and, when a single video track exists, copy it as the master MP4.
pub fn export_session(
    session_dir: &Path,
    req: &ExportRequest,
    cues: &[CaptionCue],
) -> Result<PathBuf, String> {
    let export_dir = session_dir.join("exports").join(format!(
        "export-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::create_dir_all(&export_dir).map_err(|e| e.to_string())?;

    // Copy media files
    for entry in std::fs::read_dir(session_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "wav" | "mp4" | "webm" | "json") {
                    let dest = export_dir.join(entry.file_name());
                    std::fs::copy(&path, &dest).map_err(|e| e.to_string())?;
                }
            }
        }
    }

    if req.include_captions && !cues.is_empty() {
        let srt = cues_to_srt(cues);
        std::fs::write(export_dir.join("captions.srt"), srt).map_err(|e| e.to_string())?;
    }

    let meta = serde_json::json!({
        "layout": req.layout,
        "trim_start_ms": req.trim_start_ms,
        "trim_end_ms": req.trim_end_ms,
        "exported_at": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(
        export_dir.join("export.json"),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    // Prefer first mp4/webm as master if present
    let master = std::fs::read_dir(&export_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "mp4" || e == "webm")
                .unwrap_or(false)
        });
    if let Some(m) = master {
        let dest = export_dir.join("master.mp4");
        if m.file_name().and_then(|n| n.to_str()) != Some("master.mp4") {
            let _ = std::fs::copy(&m, &dest);
        }
    }

    Ok(export_dir)
}

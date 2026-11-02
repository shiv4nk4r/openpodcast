use crate::models::{CaptionCue, Participant, PresenceEvent};
use crate::whisper::cues_to_srt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ExportRequest {
    pub session_id: String,
    pub layout: String,
    pub include_captions: bool,
    pub trim_start_ms: u64,
    pub trim_end_ms: Option<u64>,
}

#[derive(Clone, Debug)]
struct PresenceInterval {
    participant_id: String,
    start_ms: u64,
    end_ms: Option<u64>,
}

#[derive(Clone, Debug)]
struct TrackMedia {
    participant_id: String,
    #[allow(dead_code)]
    name: String,
    video: Option<PathBuf>,
    audio: Option<PathBuf>,
    offset_ms: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct SessionTimeline {
    pub duration_secs: f64,
    pub offsets_ms: HashMap<String, u64>,
    pub presence_events: Vec<PresenceEvent>,
    pub participants: Vec<Participant>,
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

    // Prefer a full composed render when ffmpeg is available.
    if which_ffmpeg().is_some() {
        match render_final_video(session_dir, &req.layout, &export_dir) {
            Ok(master) => {
                let meta = serde_json::json!({
                    "layout": req.layout,
                    "trim_start_ms": req.trim_start_ms,
                    "trim_end_ms": req.trim_end_ms,
                    "exported_at": chrono::Utc::now().to_rfc3339(),
                    "master": master.file_name().and_then(|n| n.to_str()),
                });
                std::fs::write(
                    export_dir.join("export.json"),
                    serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                if req.include_captions && !cues.is_empty() {
                    let srt = cues_to_srt(cues);
                    std::fs::write(export_dir.join("captions.srt"), srt)
                        .map_err(|e| e.to_string())?;
                }
                return Ok(export_dir);
            }
            Err(e) => {
                tracing::warn!("ffmpeg render failed, falling back to copy package: {e}");
            }
        }
    }

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

pub fn load_timeline(session_dir: &Path) -> SessionTimeline {
    let sync_path = session_dir.join("sync.json");
    let sync: serde_json::Value = std::fs::read_to_string(&sync_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}));
    SessionTimeline {
        duration_secs: sync
            .get("duration_secs")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        offsets_ms: sync
            .get("offsets_ms")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        presence_events: sync
            .get("presence_events")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        participants: sync
            .get("participants")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
    }
}

/// Compose a final podcast MP4 with the chosen layout and presence windows.
pub fn render_final_video(
    session_dir: &Path,
    layout: &str,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    let ffmpeg = which_ffmpeg().ok_or_else(|| {
        "Bundled ffmpeg was not found. Run `npm run fetch-ffmpeg` (or rebuild the app) so the renderer sidecar is installed."
            .to_string()
    })?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;

    let timeline = load_timeline(session_dir);
    let duration_secs = timeline.duration_secs.max(0.5);
    let duration_ms = (duration_secs * 1000.0).round() as u64;
    let intervals = intervals_from_events(&timeline.presence_events, duration_ms);
    let tracks = discover_tracks(session_dir, &timeline);

    if tracks.is_empty() {
        return Err("No audio/video tracks found to render".into());
    }

    let layout = normalize_layout(layout);
    let out = out_dir.join(format!("final-{layout}.mp4"));

    // Single-track fast path
    if tracks.len() == 1 {
        let t = &tracks[0];
        let mut args = vec!["-y".into()];
        if let Some(v) = &t.video {
            if t.offset_ms > 0 {
                args.push("-itsoffset".into());
                args.push(format!("{:.3}", t.offset_ms as f64 / 1000.0));
            }
            args.push("-i".into());
            args.push(v.to_string_lossy().into());
        } else {
            args.push("-f".into());
            args.push("lavfi".into());
            args.push("-i".into());
            args.push(format!(
                "color=c=0x111827:s=1920x1080:d={duration_secs:.3}:r=30"
            ));
        }
        if let Some(a) = &t.audio {
            args.push("-i".into());
            args.push(a.to_string_lossy().into());
            // Downmix odd layouts (e.g. 3.0 from some USB mics) to stereo AAC.
            args.push("-filter:a".into());
            args.push("aresample=48000:ocl=stereo".into());
        }
        args.push("-t".into());
        args.push(format!("{duration_secs:.3}"));
        args.push("-c:v".into());
        args.push("libx264".into());
        args.push("-pix_fmt".into());
        args.push("yuv420p".into());
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-b:a".into());
        args.push("192k".into());
        args.push("-shortest".into());
        args.push(out.to_string_lossy().into());
        run_ffmpeg(&ffmpeg, &args)?;
        return Ok(out);
    }

    let present_ids: Vec<String> = {
        let mut ids: Vec<String> = tracks.iter().map(|t| t.participant_id.clone()).collect();
        // Prefer people who actually appear in presence; keep all tracks otherwise.
        if !intervals.is_empty() {
            let with_presence: Vec<String> = ids
                .iter()
                .filter(|id| intervals.iter().any(|iv| &iv.participant_id == *id))
                .cloned()
                .collect();
            if !with_presence.is_empty() {
                ids = with_presence;
            }
        }
        ids
    };
    let mut tracks: Vec<TrackMedia> = tracks
        .into_iter()
        .filter(|t| present_ids.contains(&t.participant_id))
        .collect();
    if layout == "dynamic" {
        // Loudest overall track sits in the large tile for the rendered file.
        // Preview still switches live; presence gates still hide absent people.
        tracks.sort_by(|a, b| {
            avg_wav_rms(b.audio.as_deref())
                .partial_cmp(&avg_wav_rms(a.audio.as_deref()))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    } else if layout == "normal" {
        // Prefer Host as the large tile when metadata says so.
        tracks.sort_by_key(|t| {
            let hostish = timeline
                .participants
                .iter()
                .find(|p| p.id == t.participant_id)
                .map(|p| p.role.to_lowercase().contains("host"))
                .unwrap_or(false);
            if hostish {
                0
            } else {
                1
            }
        });
    }
    let n = tracks.len();
    if n == 0 {
        return Err("No present tracks to render".into());
    }

    let (w, h) = (1920u32, 1080u32);
    let mut args: Vec<String> = vec!["-y".into()];
    // Base canvas
    args.push("-f".into());
    args.push("lavfi".into());
    args.push("-i".into());
    args.push(format!(
        "color=c=0x0b1220:s={w}x{h}:d={duration_secs:.3}:r=30,format=yuv420p"
    ));

    let mut video_input_idx = Vec::new();
    let mut audio_input_idx = Vec::new();
    let mut next_in = 1usize; // 0 is canvas

    for t in &tracks {
        // Only feed containers that actually expose a video stream — chunked /
        // cam-off MediaRecorder files are often audio-only or unreadable as :v.
        let video_idx = if let Some(v) = &t.video {
            if media_has_stream(&ffmpeg, v, StreamKind::Video) {
                if t.offset_ms > 0 {
                    args.push("-itsoffset".into());
                    args.push(format!("{:.3}", t.offset_ms as f64 / 1000.0));
                }
                args.push("-i".into());
                args.push(v.to_string_lossy().into());
                let idx = next_in;
                next_in += 1;
                Some(idx)
            } else {
                tracing::warn!(
                    "skipping video without a video stream: {}",
                    v.display()
                );
                None
            }
        } else {
            None
        };
        video_input_idx.push(video_idx);

        if let Some(a) = &t.audio {
            if media_has_stream(&ffmpeg, a, StreamKind::Audio) {
                args.push("-i".into());
                args.push(a.to_string_lossy().into());
                audio_input_idx.push(Some(next_in));
                next_in += 1;
            } else {
                audio_input_idx.push(None);
            }
        } else if let Some(v) = &t.video {
            // No separate WAV — use audio from the media file when present.
            if media_has_stream(&ffmpeg, v, StreamKind::Audio) {
                if let Some(vin) = video_idx {
                    audio_input_idx.push(Some(vin)); // same input, :a in filter
                } else {
                    // Audio-only "video" file (cam off) — still mix the sound.
                    args.push("-i".into());
                    args.push(v.to_string_lossy().into());
                    audio_input_idx.push(Some(next_in));
                    next_in += 1;
                }
            } else {
                audio_input_idx.push(None);
            }
        } else {
            audio_input_idx.push(None);
        }
    }

    let cells = layout_cells(layout, n, w, h);
    let mut fc: Vec<String> = Vec::new();
    let mut last_label = "0:v".to_string();
    let mut vscale_i = 0usize;

    for (i, t) in tracks.iter().enumerate() {
        let Some(vin) = video_input_idx[i] else {
            continue;
        };
        let cell = &cells[i.min(cells.len() - 1)];
        let enable = enable_expr_for(&intervals, &t.participant_id, duration_ms);
        let scaled = format!("v{i}");
        fc.push(format!(
            "[{vin}:v]scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2:color=0x0b1220,setsar=1,fps=30,format=yuv420p[{scaled}]",
            cell.w, cell.h, cell.w, cell.h, scaled = scaled
        ));
        let out_label = format!("ov{vscale_i}");
        fc.push(format!(
            "[{base}][{scaled}]overlay={x}:{y}:eof_action=pass:enable='{enable}'[{out}]",
            base = last_label,
            scaled = scaled,
            x = cell.x,
            y = cell.y,
            enable = enable,
            out = out_label
        ));
        last_label = out_label;
        vscale_i += 1;
    }

    // Audio mix with presence gates — downmix any channel count to stereo.
    let mut amix_inputs = Vec::new();
    let mut a_i = 0usize;
    for (i, t) in tracks.iter().enumerate() {
        let enable = enable_expr_for(&intervals, &t.participant_id, duration_ms);
        if let Some(ain) = audio_input_idx[i] {
            let lab = format!("a{a_i}");
            // aresample ocl=stereo downmixes 1.0 / 3.0 / 5.1 etc. before volume gate
            fc.push(format!(
                "[{ain}:a]aresample=48000:ocl=stereo,aformat=sample_fmts=fltp,volume='{enable}'[{lab}]",
                ain = ain,
                enable = enable,
                lab = lab
            ));
            amix_inputs.push(lab);
            a_i += 1;
        }
    }

    if !amix_inputs.is_empty() {
        let inputs = amix_inputs
            .iter()
            .map(|l| format!("[{l}]"))
            .collect::<String>();
        fc.push(format!(
            "{inputs}amix=inputs={n}:duration=longest:dropout_transition=0,alimiter=limit=0.95[aout]",
            inputs = inputs,
            n = amix_inputs.len()
        ));
    }
    fc.push(format!("[{last}]null[vout]", last = last_label));
    let filter = fc.join(";");
    let has_audio = !amix_inputs.is_empty();

    args.push("-filter_complex".into());
    args.push(filter);
    args.push("-map".into());
    args.push("[vout]".into());
    if has_audio {
        args.push("-map".into());
        args.push("[aout]".into());
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-b:a".into());
        args.push("192k".into());
    }
    args.push("-t".into());
    args.push(format!("{duration_secs:.3}"));
    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push("-preset".into());
    args.push("veryfast".into());
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push(out.to_string_lossy().into());

    run_ffmpeg(&ffmpeg, &args)?;
    Ok(out)
}

fn normalize_layout(layout: &str) -> &str {
    match layout {
        "dynamic" | "speaker" | "spotlight" => "dynamic",
        "equal" | "grid" => "equal",
        "normal" | "side" | "auto" => "normal",
        other => other,
    }
}

struct Cell {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

fn layout_cells(layout: &str, n: usize, w: u32, h: u32) -> Vec<Cell> {
    let n = n.max(1);
    let gap = 12u32;
    match layout {
        "equal" => {
            let (cols, rows) = match n {
                1 => (1, 1),
                2 => (2, 1),
                3 | 4 => (2, 2),
                _ => (3, 2),
            };
            let cw = (w - gap * (cols as u32 + 1)) / cols as u32;
            let ch = (h - gap * (rows as u32 + 1)) / rows as u32;
            (0..n)
                .map(|i| {
                    let col = (i % cols) as u32;
                    let row = (i / cols) as u32;
                    Cell {
                        x: gap + col * (cw + gap),
                        y: gap + row * (ch + gap),
                        w: cw,
                        h: ch,
                    }
                })
                .collect()
        }
        "dynamic" => {
            // Speaker large left/top; others stacked on the side (active speaker
            // order is left to callers — first track is treated as primary).
            if n == 1 {
                return vec![Cell {
                    x: gap,
                    y: gap,
                    w: w - gap * 2,
                    h: h - gap * 2,
                }];
            }
            let side_w = (w as f32 * 0.28) as u32;
            let main_w = w - side_w - gap * 3;
            let mut cells = vec![Cell {
                x: gap,
                y: gap,
                w: main_w,
                h: h - gap * 2,
            }];
            let side_n = (n - 1).max(1) as u32;
            let side_h = (h - gap * (side_n + 1)) / side_n;
            for i in 0..(n - 1) {
                cells.push(Cell {
                    x: gap * 2 + main_w,
                    y: gap + i as u32 * (side_h + gap),
                    w: side_w,
                    h: side_h,
                });
            }
            cells
        }
        _ => {
            // normal: host (first) large, others strip along bottom
            if n == 1 {
                return vec![Cell {
                    x: gap,
                    y: gap,
                    w: w - gap * 2,
                    h: h - gap * 2,
                }];
            }
            let strip_h = (h as f32 * 0.28) as u32;
            let main_h = h - strip_h - gap * 3;
            let mut cells = vec![Cell {
                x: gap,
                y: gap,
                w: w - gap * 2,
                h: main_h,
            }];
            let side_n = (n - 1).max(1) as u32;
            let strip_w = (w - gap * (side_n + 1)) / side_n;
            for i in 0..(n - 1) {
                cells.push(Cell {
                    x: gap + i as u32 * (strip_w + gap),
                    y: gap * 2 + main_h,
                    w: strip_w,
                    h: strip_h,
                });
            }
            cells
        }
    }
}

fn enable_expr_for(intervals: &[PresenceInterval], pid: &str, duration_ms: u64) -> String {
    let mine: Vec<&PresenceInterval> = intervals
        .iter()
        .filter(|iv| iv.participant_id == pid)
        .collect();
    if mine.is_empty() {
        // No presence data → always visible
        return "1".into();
    }
    let parts: Vec<String> = mine
        .iter()
        .map(|iv| {
            let start = iv.start_ms as f64 / 1000.0;
            let end = iv
                .end_ms
                .unwrap_or(duration_ms)
                .max(iv.start_ms) as f64
                / 1000.0;
            format!("between(t\\,{start:.3}\\,{end:.3})")
        })
        .collect();
    parts.join("+")
}

fn intervals_from_events(events: &[PresenceEvent], duration_ms: u64) -> Vec<PresenceInterval> {
    let mut by_id: HashMap<String, Vec<&PresenceEvent>> = HashMap::new();
    for ev in events {
        by_id
            .entry(ev.participant_id.clone())
            .or_default()
            .push(ev);
    }
    let mut out = Vec::new();
    for (pid, mut list) in by_id {
        list.sort_by_key(|e| e.t_ms);
        let mut open: Option<u64> = None;
        for ev in list {
            if ev.kind == "join" {
                if open.is_none() {
                    open = Some(ev.t_ms);
                }
            } else if ev.kind == "leave" {
                if let Some(s) = open.take() {
                    out.push(PresenceInterval {
                        participant_id: pid.clone(),
                        start_ms: s,
                        end_ms: Some(ev.t_ms.max(s)),
                    });
                }
            }
        }
        if let Some(s) = open {
            out.push(PresenceInterval {
                participant_id: pid,
                start_ms: s,
                end_ms: if duration_ms > 0 {
                    Some(duration_ms)
                } else {
                    None
                },
            });
        }
    }
    out
}

fn discover_tracks(session_dir: &Path, timeline: &SessionTimeline) -> Vec<TrackMedia> {
    let mut by_id: HashMap<String, TrackMedia> = HashMap::new();
    let name_by: HashMap<String, String> = timeline
        .participants
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();

    if let Ok(entries) = std::fs::read_dir(session_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let (pid, kind) = if let Some(rest) = name.strip_suffix("-video.mp4") {
                (rest.to_string(), "video")
            } else if let Some(rest) = name.strip_suffix("-video.webm") {
                (rest.to_string(), "video")
            } else if let Some(rest) = name.strip_suffix("-audio.wav") {
                (rest.to_string(), "audio")
            } else {
                continue;
            };
            let entry = by_id.entry(pid.clone()).or_insert_with(|| TrackMedia {
                participant_id: pid.clone(),
                name: name_by.get(&pid).cloned().unwrap_or_else(|| pid.clone()),
                video: None,
                audio: None,
                offset_ms: *timeline.offsets_ms.get(&pid).unwrap_or(&0),
            });
            if kind == "video" {
                entry.video = Some(path);
            } else {
                entry.audio = Some(path);
            }
        }
    }

    // Stable order: participants list first, then leftovers
    let mut ordered = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in &timeline.participants {
        if let Some(t) = by_id.remove(&p.id) {
            seen.insert(p.id.clone());
            ordered.push(t);
        }
    }
    let mut rest: Vec<_> = by_id.into_values().collect();
    rest.sort_by(|a, b| a.participant_id.cmp(&b.participant_id));
    for t in rest {
        if !seen.contains(&t.participant_id) {
            ordered.push(t);
        }
    }
    ordered
}

fn avg_wav_rms(path: Option<&Path>) -> f32 {
    let Some(path) = path else { return 0.0 };
    let Ok(mut reader) = hound::WavReader::open(path) else {
        return 0.0;
    };
    let spec = reader.spec();
    let max = match spec.sample_format {
        hound::SampleFormat::Float => 1.0f32,
        hound::SampleFormat::Int => match spec.bits_per_sample {
            16 => i16::MAX as f32,
            32 => i32::MAX as f32,
            _ => i16::MAX as f32,
        },
    };
    let mut sum = 0.0f64;
    let mut n = 0u64;
    // Sample sparsely for speed on long takes.
    let step = 8usize;
    let mut i = 0usize;
    for sample in reader.samples::<i32>().flatten() {
        if i % step == 0 {
            let v = sample as f32 / max;
            sum += (v as f64) * (v as f64);
            n += 1;
        }
        i += 1;
        if n > 200_000 {
            break;
        }
    }
    if n == 0 {
        0.0
    } else {
        (sum / n as f64).sqrt() as f32
    }
}

#[derive(Clone, Copy)]
enum StreamKind {
    Video,
    Audio,
}

/// Probe with ffmpeg itself (sidecar often has no ffprobe).
fn media_has_stream(ffmpeg: &Path, path: &Path, kind: StreamKind) -> bool {
    if !path.is_file() {
        return false;
    }
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) < 32 {
        return false;
    }
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(path)
        .output();
    let Ok(output) = output else {
        return false;
    };
    // ffmpeg writes the probe dump to stderr and exits non-zero without an output sink.
    let probe = String::from_utf8_lossy(&output.stderr);
    match kind {
        StreamKind::Video => probe.lines().any(|l| {
            let l = l.trim();
            l.contains("Video:") || l.contains(": Video:")
        }),
        StreamKind::Audio => probe.lines().any(|l| {
            let l = l.trim();
            l.contains("Audio:") || l.contains(": Audio:")
        }),
    }
}

fn which_ffmpeg() -> Option<PathBuf> {
    // 1) Bundled Tauri sidecar (next to the app binary, or target/debug during `tauri dev`).
    for p in bundled_ffmpeg_candidates() {
        if p.is_file() {
            return Some(p);
        }
    }

    // 2) PATH (macOS `which`, Windows `where`).
    let finder = if cfg!(windows) { "where" } else { "which" };
    if let Ok(output) = Command::new(finder).arg("ffmpeg").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !path.is_empty() {
                let p = PathBuf::from(path);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }

    // 3) Common install locations.
    for candidate in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
        r"C:\ffmpeg\bin\ffmpeg.exe",
        r"C:\ProgramData\chocolatey\bin\ffmpeg.exe",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn bundled_ffmpeg_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let triple = option_env!("TARGET_TRIPLE").unwrap_or("unknown-unknown-unknown");
    let names = [
        "ffmpeg".to_string(),
        "ffmpeg.exe".to_string(),
        format!("ffmpeg-{triple}"),
        format!("ffmpeg-{triple}.exe"),
    ];

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in &names {
                out.push(dir.join(name));
            }
            // macOS .app: also check Contents/MacOS (same as exe dir) and Resources
            if cfg!(target_os = "macos") {
                if let Some(contents) = dir.parent() {
                    out.push(contents.join("Resources").join("ffmpeg"));
                    out.push(contents.join(format!("Resources/ffmpeg-{triple}")));
                }
            }
        }
    }

    // `tauri dev` / cargo: binaries checked into src-tauri/binaries
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries");
    for name in &names {
        out.push(manifest.join(name));
    }
    out
}

fn run_ffmpeg(ffmpeg: &Path, args: &[String]) -> Result<(), String> {
    let output = Command::new(ffmpeg)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(1200)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(format!("ffmpeg failed: {tail}"));
    }
    Ok(())
}

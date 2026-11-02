use crate::models::{CaptionCue, Participant, PresenceEvent};
use crate::whisper::cues_to_srt;
use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// Live encode progress pushed to the UI while rendering.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenderProgress {
    pub pct: f64,
    pub out_time_secs: f64,
    pub duration_secs: f64,
    pub eta_secs: Option<f64>,
    pub phase: String,
}

pub type ProgressCallback<'a> = Option<&'a mut dyn FnMut(RenderProgress)>;

fn emit_progress(cb: &mut ProgressCallback<'_>, progress: RenderProgress) {
    if let Some(f) = cb.as_mut() {
        f(progress);
    }
}

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
        match render_final_video(session_dir, &req.layout, &export_dir, &mut None) {
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
    on_progress: &mut ProgressCallback<'_>,
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
            args.push(
                "aresample=48000:ocl=stereo,aformat=sample_fmts=fltp:channel_layouts=stereo"
                    .into(),
            );
        }
        args.push("-t".into());
        args.push(format!("{duration_secs:.3}"));
        args.push("-c:v".into());
        args.push("libx264".into());
        args.push("-pix_fmt".into());
        args.push("yuv420p".into());
        if t.audio.is_some() {
            args.push("-c:a".into());
            args.push("aac".into());
            args.push("-b:a".into());
            args.push("192k".into());
            args.push("-ac".into());
            args.push("2".into());
        }
        args.push("-shortest".into());
        args.push(out.to_string_lossy().into());
        run_ffmpeg(&ffmpeg, &args, duration_secs, on_progress)?;
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
    let corner_r = 28u32;
    let mut args: Vec<String> = vec!["-y".into()];
    // Soft studio backdrop (matches in-app stage feel).
    args.push("-f".into());
    args.push("lavfi".into());
    args.push("-i".into());
    args.push(format!(
        "color=c=0x10141e:s={w}x{h}:d={duration_secs:.3}:r=30,format=rgba"
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
    // Keep alpha on the running composition so rounded tiles composite cleanly.
    let mut last_label = "base0".to_string();
    fc.push("[0:v]format=rgba[base0]".into());
    let mut vscale_i = 0usize;

    for (i, t) in tracks.iter().enumerate() {
        let Some(vin) = video_input_idx[i] else {
            continue;
        };
        let cell = &cells[i.min(cells.len() - 1)];
        let enable = enable_expr_for(&intervals, &t.participant_id, duration_ms);
        let radius = corner_r.min(cell.w / 4).min(cell.h / 4).max(12);
        let fitted = format!("vf{i}");
        let rounded = format!("vr{i}");
        let shadow = format!("vs{i}");
        let named = format!("vn{i}");
        // Cover-fit into 16:9 tile, then soft rounded alpha (studio chrome).
        fc.push(format!(
            "[{vin}:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1,fps=30,format=rgba[{fitted}]",
            vin = vin,
            w = cell.w,
            h = cell.h,
            fitted = fitted
        ));
        fc.push(round_tile_alpha(&fitted, &rounded, radius));
        // Drop shadow under the tile
        fc.push(format!(
            "color=c=black@0.45:s={cw}x{ch}:d={dur:.3},format=rgba,{alpha},fps=30[{shadow}]",
            cw = cell.w,
            ch = cell.h,
            dur = duration_secs,
            alpha = round_alpha_geq(radius),
            shadow = shadow
        ));
        let label = escape_drawtext(&t.name);
        fc.push(format!(
            "[{rounded}]drawtext=text='{label}':fontsize={fs}:fontcolor=white:borderw=0:shadowcolor=black@0.65:shadowx=1:shadowy=1:x=22:y=h-42[{named}]",
            rounded = rounded,
            label = label,
            named = named,
            fs = if cell.w > 700 { 30 } else { 22 }
        ));

        let after_shadow = format!("ovs{vscale_i}");
        let out_label = format!("ov{vscale_i}");
        fc.push(format!(
            "[{base}][{shadow}]overlay={x}:{y}:eof_action=pass:format=auto:enable='{enable}'[{mid}]",
            base = last_label,
            shadow = shadow,
            x = cell.x + 5,
            y = cell.y + 8,
            enable = enable,
            mid = after_shadow
        ));
        fc.push(format!(
            "[{mid}][{named}]overlay={x}:{y}:eof_action=pass:format=auto:enable='{enable}'[{out}]",
            mid = after_shadow,
            named = named,
            x = cell.x,
            y = cell.y,
            enable = enable,
            out = out_label
        ));
        last_label = out_label;
        vscale_i += 1;
    }

    // Audio mix — eval=frame is required or time-based volume stays silent.
    let mut amix_inputs = Vec::new();
    let mut a_i = 0usize;
    for (i, t) in tracks.iter().enumerate() {
        let enable = enable_expr_for(&intervals, &t.participant_id, duration_ms);
        if let Some(ain) = audio_input_idx[i] {
            let lab = format!("a{a_i}");
            fc.push(format!(
                "[{ain}:a]aresample=48000:ocl=stereo,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=eval=frame:volume='{enable}'[{lab}]",
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
        if amix_inputs.len() == 1 {
            fc.push(format!(
                "{inputs}anull,alimiter=limit=0.95[aout]",
                inputs = inputs
            ));
        } else {
            fc.push(format!(
                "{inputs}amix=inputs={n}:duration=longest:dropout_transition=0:normalize=0,alimiter=limit=0.95[aout]",
                inputs = inputs,
                n = amix_inputs.len()
            ));
        }
    }
    fc.push(format!(
        "[{last}]format=yuv420p[vout]",
        last = last_label
    ));
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
        args.push("-ac".into());
        args.push("2".into());
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

    run_ffmpeg(&ffmpeg, &args, duration_secs, on_progress)?;
    Ok(out)
}

/// Mix participant WAVs into a single stereo AAC podcast (no video).
pub fn render_final_audio(
    session_dir: &Path,
    out_dir: &Path,
    on_progress: &mut ProgressCallback<'_>,
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
        return Err("No audio tracks found to render".into());
    }

    let out = out_dir.join("final-audio.m4a");
    let mut args: Vec<String> = vec!["-y".into()];
    let mut input_idxs: Vec<(usize, String)> = Vec::new(); // (ffmpeg input index, participant_id)

    for t in &tracks {
        let path = t
            .audio
            .as_ref()
            .or(t.video.as_ref())
            .filter(|p| media_has_stream(&ffmpeg, p, StreamKind::Audio));
        let Some(path) = path else { continue };
        let idx = input_idxs.len();
        args.push("-i".into());
        args.push(path.to_string_lossy().into());
        input_idxs.push((idx, t.participant_id.clone()));
    }

    if input_idxs.is_empty() {
        return Err("No playable audio streams found to render".into());
    }

    let mut fc: Vec<String> = Vec::new();
    let mut amix_inputs = Vec::new();
    for (i, (ain, pid)) in input_idxs.iter().enumerate() {
        let enable = enable_expr_for(&intervals, pid, duration_ms);
        let lab = format!("a{i}");
        fc.push(format!(
            "[{ain}:a]aresample=48000:ocl=stereo,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=eval=frame:volume='{enable}'[{lab}]",
            ain = ain,
            enable = enable,
            lab = lab
        ));
        amix_inputs.push(lab);
    }

    let mix = if amix_inputs.len() == 1 {
        format!("[{}]anull,alimiter=limit=0.95[aout]", amix_inputs[0])
    } else {
        let inputs = amix_inputs
            .iter()
            .map(|l| format!("[{l}]"))
            .collect::<String>();
        format!(
            "{inputs}amix=inputs={n}:duration=longest:dropout_transition=0:normalize=0,alimiter=limit=0.95[aout]",
            inputs = inputs,
            n = amix_inputs.len()
        )
    };
    fc.push(mix);

    args.push("-filter_complex".into());
    args.push(fc.join(";"));
    args.push("-map".into());
    args.push("[aout]".into());
    args.push("-t".into());
    args.push(format!("{duration_secs:.3}"));
    args.push("-c:a".into());
    args.push("aac".into());
    args.push("-b:a".into());
    args.push("192k".into());
    args.push("-ac".into());
    args.push("2".into());
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push(out.to_string_lossy().into());

    run_ffmpeg(&ffmpeg, &args, duration_secs, on_progress)?;
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

/// Keep tiles 16:9 (studio stage), never stretch into tall squares.
fn fit_16x9(max_w: u32, max_h: u32) -> (u32, u32) {
    let max_w = max_w.max(160);
    let max_h = max_h.max(90);
    let by_w_h = (max_w as f32 * 9.0 / 16.0) as u32;
    if by_w_h <= max_h {
        (max_w, by_w_h.max(90))
    } else {
        let by_h_w = (max_h as f32 * 16.0 / 9.0) as u32;
        (by_h_w.max(160), max_h)
    }
}

fn layout_cells(layout: &str, n: usize, w: u32, h: u32) -> Vec<Cell> {
    let n = n.max(1);
    let gap = 28u32;
    match layout {
        "equal" => {
            let (cols, rows) = match n {
                1 => (1, 1),
                2 => (2, 1),
                3 | 4 => (2, 2),
                _ => (3, 2),
            };
            let slot_w = (w - gap * (cols as u32 + 1)) / cols as u32;
            let slot_h = (h - gap * (rows as u32 + 1)) / rows as u32;
            let (tw, th) = fit_16x9(slot_w, slot_h);
            (0..n)
                .map(|i| {
                    let col = (i % cols) as u32;
                    let row = (i / cols) as u32;
                    let slot_x = gap + col * (slot_w + gap);
                    let slot_y = gap + row * (slot_h + gap);
                    Cell {
                        x: slot_x + slot_w.saturating_sub(tw) / 2,
                        y: slot_y + slot_h.saturating_sub(th) / 2,
                        w: tw,
                        h: th,
                    }
                })
                .collect()
        }
        "dynamic" => {
            if n == 1 {
                let (tw, th) = fit_16x9(w - gap * 2, h - gap * 2);
                return vec![Cell {
                    x: (w - tw) / 2,
                    y: (h - th) / 2,
                    w: tw,
                    h: th,
                }];
            }
            let side_slot_w = (w as f32 * 0.26) as u32;
            let main_slot_w = w - side_slot_w - gap * 3;
            let (main_w, main_h) = fit_16x9(main_slot_w, h - gap * 2);
            let mut cells = vec![Cell {
                x: gap + main_slot_w.saturating_sub(main_w) / 2,
                y: (h - main_h) / 2,
                w: main_w,
                h: main_h,
            }];
            let side_n = (n - 1).max(1) as u32;
            let side_slot_h = (h - gap * (side_n + 1)) / side_n;
            let (side_w, side_h) = fit_16x9(side_slot_w, side_slot_h);
            for i in 0..(n - 1) {
                let slot_y = gap + i as u32 * (side_slot_h + gap);
                cells.push(Cell {
                    x: gap * 2 + main_slot_w + side_slot_w.saturating_sub(side_w) / 2,
                    y: slot_y + side_slot_h.saturating_sub(side_h) / 2,
                    w: side_w,
                    h: side_h,
                });
            }
            cells
        }
        _ => {
            // normal: host large, guests in a 16:9 strip along the bottom
            if n == 1 {
                let (tw, th) = fit_16x9(w - gap * 2, h - gap * 2);
                return vec![Cell {
                    x: (w - tw) / 2,
                    y: (h - th) / 2,
                    w: tw,
                    h: th,
                }];
            }
            let strip_budget = (h as f32 * 0.30) as u32;
            let main_budget = h - strip_budget - gap * 3;
            let (main_w, main_h) = fit_16x9(w - gap * 2, main_budget);
            let mut cells = vec![Cell {
                x: (w - main_w) / 2,
                y: gap + main_budget.saturating_sub(main_h) / 2,
                w: main_w,
                h: main_h,
            }];
            let side_n = (n - 1).max(1) as u32;
            let strip_slot_w = (w - gap * (side_n + 1)) / side_n;
            let (sw, sh) = fit_16x9(strip_slot_w, strip_budget);
            let strip_y = gap * 2 + main_budget + strip_budget.saturating_sub(sh) / 2;
            for i in 0..(n - 1) {
                let slot_x = gap + i as u32 * (strip_slot_w + gap);
                cells.push(Cell {
                    x: slot_x + strip_slot_w.saturating_sub(sw) / 2,
                    y: strip_y,
                    w: sw,
                    h: sh,
                });
            }
            cells
        }
    }
}

fn round_alpha_geq(radius: u32) -> String {
    let r = radius.max(8);
    // Transparent outside a rounded-rect; opaque inside.
    format!(
        "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(gt(abs(W/2-X),W/2-{r})*gt(abs(H/2-Y),H/2-{r}),if(lte(hypot(abs(W/2-X)-(W/2-{r}),abs(H/2-Y)-(H/2-{r})),{r}),255,0),255)'"
    )
}

fn round_tile_alpha(input: &str, output: &str, radius: u32) -> String {
    format!(
        "[{input}]format=rgba,{alpha}[{output}]",
        input = input,
        alpha = round_alpha_geq(radius),
        output = output
    )
}

fn escape_drawtext(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\\\".into(),
            ':' => "\\:".into(),
            '\'' => "\u{2019}".into(), // typographic apostrophe avoids quote breakage
            '%' => "%%".into(),
            c if c.is_control() => "".into(),
            c => c.to_string(),
        })
        .collect()
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

fn started_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn parse_ffmpeg_clock(s: &str) -> Option<f64> {
    // HH:MM:SS.microseconds
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

struct FfmpegProgressState {
    out_time_secs: f64,
    speed: f64,
    done: bool,
}

fn parse_ffmpeg_progress_text(text: &str, state: &mut FfmpegProgressState) {
    // ffmpeg appends key=value blocks; keep the latest values in the file.
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("out_time_ms=") {
            if let Ok(ms) = v.parse::<f64>() {
                state.out_time_secs = (ms / 1000.0).max(0.0);
            }
        } else if let Some(v) = line.strip_prefix("out_time=") {
            if let Some(secs) = parse_ffmpeg_clock(v) {
                state.out_time_secs = secs.max(0.0);
            }
        } else if let Some(v) = line.strip_prefix("speed=") {
            let cleaned = v.trim().trim_end_matches('x');
            if let Ok(s) = cleaned.parse::<f64>() {
                if s.is_finite() && s > 0.0 {
                    state.speed = s;
                }
            }
        } else if line == "progress=end" {
            state.done = true;
        }
    }
}

fn progress_from_state(
    state: &FfmpegProgressState,
    duration_secs: f64,
    started: Instant,
) -> RenderProgress {
    let remaining = (duration_secs - state.out_time_secs).max(0.0);
    let eta_secs = if state.done {
        Some(0.0)
    } else if state.speed > 0.05 {
        Some(remaining / state.speed)
    } else {
        let elapsed = started.elapsed().as_secs_f64();
        if state.out_time_secs > 0.25 && elapsed > 0.25 {
            let rate = state.out_time_secs / elapsed;
            if rate > 0.01 {
                Some(remaining / rate)
            } else {
                None
            }
        } else {
            None
        }
    };
    let pct = if state.done {
        100.0
    } else if duration_secs > 0.0 {
        (state.out_time_secs / duration_secs * 100.0).clamp(0.0, 99.5)
    } else {
        0.0
    };
    RenderProgress {
        pct,
        out_time_secs: state.out_time_secs,
        duration_secs,
        eta_secs,
        phase: if state.done {
            "done".into()
        } else {
            "encoding".into()
        },
    }
}

fn run_ffmpeg(
    ffmpeg: &Path,
    args: &[String],
    duration_secs: f64,
    on_progress: &mut ProgressCallback<'_>,
) -> Result<(), String> {
    // pipe:1 is fully buffered when not a TTY, so progress only arrives at exit.
    // Writing to a file lets us poll live updates while ffmpeg runs.
    let progress_path = std::env::temp_dir().join(format!(
        "openpodcast-ffmpeg-progress-{}-{}.txt",
        std::process::id(),
        started_nanos()
    ));
    let _ = std::fs::remove_file(&progress_path);

    let mut full_args: Vec<String> = Vec::with_capacity(args.len() + 5);
    full_args.push("-nostats".into());
    full_args.push("-stats_period".into());
    full_args.push("0.5".into());
    full_args.push("-progress".into());
    full_args.push(progress_path.to_string_lossy().into());
    full_args.extend(args.iter().cloned());

    let mut child = Command::new(ffmpeg)
        .args(&full_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "ffmpeg stderr unavailable".to_string())?;

    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = std::io::Read::read_to_string(&mut reader, &mut buf);
        buf
    });

    emit_progress(
        on_progress,
        RenderProgress {
            pct: 0.0,
            out_time_secs: 0.0,
            duration_secs,
            eta_secs: None,
            phase: "encoding".into(),
        },
    );

    let started = Instant::now();
    let mut state = FfmpegProgressState {
        out_time_secs: 0.0,
        speed: 0.0,
        done: false,
    };
    let mut last_emitted_ms = -1_i64;

    loop {
        if let Ok(text) = std::fs::read_to_string(&progress_path) {
            parse_ffmpeg_progress_text(&text, &mut state);
            let tick_ms = (state.out_time_secs * 10.0).floor() as i64;
            if tick_ms != last_emitted_ms || state.done {
                last_emitted_ms = tick_ms;
                emit_progress(
                    on_progress,
                    progress_from_state(&state, duration_secs, started),
                );
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                // Final read in case the last progress block landed as the process exited.
                if let Ok(text) = std::fs::read_to_string(&progress_path) {
                    parse_ffmpeg_progress_text(&text, &mut state);
                }
                let _ = std::fs::remove_file(&progress_path);
                let stderr_text = stderr_thread
                    .join()
                    .unwrap_or_else(|_| String::from("(stderr unavailable)"));
                if !status.success() {
                    let tail: String = stderr_text
                        .chars()
                        .rev()
                        .take(1200)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    return Err(format!("ffmpeg failed: {tail}"));
                }
                state.done = true;
                emit_progress(
                    on_progress,
                    progress_from_state(&state, duration_secs, started),
                );
                return Ok(());
            }
            Ok(None) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = std::fs::remove_file(&progress_path);
                return Err(format!("ffmpeg wait failed: {e}"));
            }
        }
    }
}

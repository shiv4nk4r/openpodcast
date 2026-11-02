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

/// Output quality for composed video renders.
#[derive(Debug, Clone)]
pub struct RenderVideoOpts {
    pub resolution: String,
    pub fps: u32,
}

impl Default for RenderVideoOpts {
    fn default() -> Self {
        Self {
            resolution: "1080p".into(),
            fps: 30,
        }
    }
}

impl RenderVideoOpts {
    pub fn size(&self) -> (u32, u32) {
        match self.resolution.as_str() {
            "720p" => (1280, 720),
            "1440p" => (2560, 1440),
            "2160p" | "4K" | "4k" => (3840, 2160),
            _ => (1920, 1080),
        }
    }

    pub fn fps(&self) -> u32 {
        match self.fps {
            24 | 25 | 30 | 50 | 60 => self.fps,
            _ => 30,
        }
    }
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
    name: String,
    role: String,
    video: Option<PathBuf>,
    audio: Option<PathBuf>,
    offset_ms: u64,
}

/// Light studio stage tokens — match default `[data-theme="light"]` preview.
struct StudioChrome {
    /// Page / canvas background (`--page`)
    bg: &'static str,
    /// Soft tile shadow opacity
    shadow_alpha: f64,
    /// Empty / cam-off tile fill (`--surface-3`)
    empty_bg: &'static str,
    /// Empty tile label (`--ink-muted`)
    empty_fg: &'static str,
    /// Name chip fill / text (`--chip` / `--chip-text`)
    chip_bg: &'static str,
    chip_fg: &'static str,
    pad: u32,
    gap: u32,
    radius: u32,
    chip_pad_x: u32,
    chip_pad_y: u32,
    chip_inset: u32,
    chip_fontsize: u32,
    fontfile: Option<PathBuf>,
}

impl StudioChrome {
    fn for_size(w: u32) -> Self {
        let scale = w as f64 / 1920.0;
        let px = |v: f64| -> u32 { (v * scale).round().max(1.0) as u32 };
        Self {
            // `--page` light default
            bg: "0xf1f4f9",
            shadow_alpha: 0.18,
            empty_bg: "0xe9edf5",
            empty_fg: "0x5b6578",
            chip_bg: "0xffffff@0.86",
            chip_fg: "0x10141c",
            pad: px(20.0),
            gap: px(12.0),
            radius: px(18.0).clamp(10, 40),
            chip_pad_x: px(10.0).max(6),
            chip_pad_y: px(5.0).max(3),
            chip_inset: px(12.0).max(8),
            chip_fontsize: px(14.0).clamp(11, 36),
            fontfile: manrope_font_path(),
        }
    }

    fn font_arg(&self) -> String {
        match &self.fontfile {
            Some(p) => format!(":fontfile={}", escape_drawtext_path(p)),
            None => String::new(),
        }
    }
}

fn manrope_font_path() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Manrope-SemiBold.ttf"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Manrope-SemiBold.otf"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn escape_drawtext_path(p: &Path) -> String {
    // ffmpeg drawtext fontfile: escape \ and :
    p.to_string_lossy()
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".into(),
            ':' => "\\:".into(),
            '\'' => "\\'".into(),
            c => c.to_string(),
        })
        .collect()
}

fn track_label(t: &TrackMedia) -> String {
    let name = t.name.trim();
    let role = t.role.trim();
    if role.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {role}")
    }
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
        match render_final_video(
            session_dir,
            &req.layout,
            &export_dir,
            &RenderVideoOpts::default(),
            &mut None,
        ) {
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
    opts: &RenderVideoOpts,
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
    let (out_w, out_h) = opts.size();
    let fps = opts.fps();

    if tracks.is_empty() {
        return Err("No audio/video tracks found to render".into());
    }

    let layout = normalize_layout(layout);
    let out = out_dir.join(format!("final-{layout}.mp4"));
    let chrome = StudioChrome::for_size(out_w);

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

    let (w, h) = (out_w, out_h);
    let mut args: Vec<String> = vec!["-y".into()];
    // Studio stage backdrop — matches dark preview `--page`.
    args.push("-f".into());
    args.push("lavfi".into());
    args.push("-i".into());
    args.push(format!(
        "color=c={bg}:s={w}x{h}:d={duration_secs:.3}:r={fps},format=rgba",
        bg = chrome.bg
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

    let cells = layout_cells(layout, n, w, h, chrome.pad, chrome.gap);
    let mut fc: Vec<String> = Vec::new();
    // Keep alpha on the running composition so rounded tiles composite cleanly.
    let mut last_label = "base0".to_string();
    fc.push("[0:v]format=rgba[base0]".into());
    let mut vscale_i = 0usize;
    let shadow_blur = ((14.0 * (w as f64 / 1920.0)).round() as i32).clamp(6, 28);
    let shadow_oy = ((10.0 * (w as f64 / 1920.0)).round() as i32).max(4);

    for (i, t) in tracks.iter().enumerate() {
        let cell = &cells[i.min(cells.len() - 1)];
        let enable = enable_expr_for(&intervals, &t.participant_id, duration_ms);
        let radius = chrome
            .radius
            .min(cell.w / 4)
            .min(cell.h / 4)
            .max(8);
        let fitted = format!("vf{i}");
        let rounded = format!("vr{i}");
        let shadow = format!("vs{i}");
        let named = format!("vn{i}");

        if let Some(vin) = video_input_idx[i] {
            // Cover-fit into 16:9 tile (matches `object-fit: cover`).
            fc.push(format!(
                "[{vin}:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1,fps={fps},format=rgba[{fitted}]",
                vin = vin,
                w = cell.w,
                h = cell.h,
                fps = fps,
                fitted = fitted
            ));
        } else {
            // Cam-off / audio-only plate — matches preview `.tile .empty`.
            let empty_name = escape_drawtext(&t.name);
            fc.push(format!(
                "color=c={empty_bg}:s={cw}x{ch}:d={dur:.3}:r={fps},format=rgba,drawtext=text='{empty_name}':fontsize={fs}:fontcolor={empty_fg}{font}:x=(w-text_w)/2:y=(h-text_h)/2[{fitted}]",
                empty_bg = chrome.empty_bg,
                cw = cell.w,
                ch = cell.h,
                dur = duration_secs,
                fps = fps,
                empty_name = empty_name,
                empty_fg = chrome.empty_fg,
                fs = (chrome.chip_fontsize as f64 * 1.6).round() as u32,
                font = chrome.font_arg(),
                fitted = fitted
            ));
        }
        fc.push(round_tile_alpha(&fitted, &rounded, radius));

        // Soft drop shadow approximating CSS `--shadow` (blurred, low opacity).
        fc.push(format!(
            "color=c=black@{alpha}:s={cw}x{ch}:d={dur:.3},format=rgba,{round_a},boxblur={blur}:{blur},fps={fps}[{shadow}]",
            alpha = chrome.shadow_alpha,
            cw = cell.w,
            ch = cell.h,
            dur = duration_secs,
            round_a = round_alpha_geq(radius),
            blur = shadow_blur,
            fps = fps,
            shadow = shadow
        ));

        // Name chip — matches light `.name-tag` (`--chip` / Manrope).
        let label = escape_drawtext(&track_label(t));
        let chip_x = chrome.chip_inset;
        let chip_bottom = chrome.chip_inset;
        fc.push(format!(
            "[{rounded}]drawtext=text='{label}':fontsize={fs}:fontcolor={chip_fg}{font}:box=1:boxcolor={chip_bg}:boxborderw={pad}:x={x}:y=h-text_h-{bottom}-{pad}[{named}]",
            rounded = rounded,
            label = label,
            fs = chrome.chip_fontsize,
            chip_fg = chrome.chip_fg,
            chip_bg = chrome.chip_bg,
            font = chrome.font_arg(),
            pad = chrome.chip_pad_y.max(chrome.chip_pad_x / 2),
            x = chip_x,
            bottom = chip_bottom,
            named = named
        ));

        let after_shadow = format!("ovs{vscale_i}");
        let out_label = format!("ov{vscale_i}");
        fc.push(format!(
            "[{base}][{shadow}]overlay={x}:{y}:eof_action=pass:format=auto:enable='{enable}'[{mid}]",
            base = last_label,
            shadow = shadow,
            x = cell.x as i32,
            y = cell.y as i32 + shadow_oy,
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
    args.push("-r".into());
    args.push(fps.to_string());
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

/// Build an NLE package Resolve can import: Final Cut Pro 7 XML (`.xml`) is the
/// most reliable path in DaVinci Resolve for multi-track ISO timelines.
/// Also writes a companion FCPXML 1.8 for Final Cut / Premiere.
pub fn export_nle_timeline(
    session_dir: &Path,
    project_name: &str,
    resolution: &str,
    fps: u32,
) -> Result<PathBuf, String> {
    let timeline = load_timeline(session_dir);
    let tracks = discover_tracks(session_dir, &timeline);
    if tracks.is_empty() {
        return Err("No media tracks found to export".into());
    }

    let opts = RenderVideoOpts {
        resolution: resolution.to_string(),
        fps,
    };
    let (width, height) = opts.size();
    let fps = opts.fps();
    let duration_secs = timeline.duration_secs.max(0.5);
    let frames = ((duration_secs * fps as f64).round() as i64).max(1);

    let out_dir = session_dir.join("exports").join(format!(
        "nle-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let media_dir = out_dir.join("media");
    std::fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;

    let mut stem = sanitize_filename(project_name);
    if stem.is_empty() {
        stem = "OpenPodcast".into();
    }
    stem = stem.chars().take(80).collect();

    #[derive(Clone)]
    struct NleClip {
        name: String,
        local_path: PathBuf,
        offset_frames: i64,
        is_video: bool,
    }

    // Resolve often refuses browser WebM / exotic containers. Transcode video
    // to H.264 MP4 and keep WAV audio as-is.
    let ffmpeg = which_ffmpeg();
    let mut clips: Vec<NleClip> = Vec::new();
    for (ti, t) in tracks.iter().enumerate() {
        let off_frames = ((t.offset_ms as f64 / 1000.0) * fps as f64).round() as i64;
        if let Some(v) = &t.video {
            let dest = media_dir.join(format!(
                "{:02}-{}-video.mp4",
                ti + 1,
                sanitize_filename(&t.name)
            ));
            prepare_resolve_video(ffmpeg.as_deref(), v, &dest, fps)?;
            clips.push(NleClip {
                name: format!("{} video", t.name),
                local_path: dest,
                offset_frames: off_frames,
                is_video: true,
            });
        }
        if let Some(a) = &t.audio {
            let dest = media_dir.join(format!(
                "{:02}-{}-audio.wav",
                ti + 1,
                sanitize_filename(&t.name)
            ));
            link_or_copy(a, &dest)?;
            clips.push(NleClip {
                name: format!("{} audio", t.name),
                local_path: dest,
                offset_frames: off_frames,
                is_video: false,
            });
        }
    }

    if clips.is_empty() {
        return Err("No video or audio files to put on the NLE timeline".into());
    }

    let project = xml_escape(project_name);
    let ntsc = if matches!(fps, 24 | 30 | 60) {
        "TRUE"
    } else {
        "FALSE"
    };
    let rate_xml = format!(
        "<rate><timebase>{fps}</timebase><ntsc>{ntsc}</ntsc></rate>"
    );

    let mut video_tracks = String::new();
    let mut audio_tracks = String::new();
    let mut file_id = 1u32;
    let mut clip_id = 1u32;

    for clip in &clips {
        let pathurl = file_url_localhost(&clip.local_path)?;
        let name = xml_escape(&clip.name);
        let start = clip.offset_frames.max(0);
        let end = start + frames;
        let fid = format!("file-{file_id}");
        let cid = format!("clipitem-{clip_id}");
        file_id += 1;
        clip_id += 1;

        if clip.is_video {
            video_tracks.push_str(&format!(
                r#"        <track>
          <clipitem id="{cid}">
            <name>{name}</name>
            <enabled>TRUE</enabled>
            <duration>{frames}</duration>
            {rate}
            <start>{start}</start>
            <end>{end}</end>
            <in>0</in>
            <out>{frames}</out>
            <file id="{fid}">
              <name>{name}</name>
              <pathurl>{pathurl}</pathurl>
              <rate>{rate_inner}</rate>
              <duration>{frames}</duration>
              <media>
                <video>
                  <samplecharacteristics>
                    <rate>{rate_inner}</rate>
                    <width>{width}</width>
                    <height>{height}</height>
                    <pixelaspectratio>square</pixelaspectratio>
                    <fielddominance>none</fielddominance>
                  </samplecharacteristics>
                </video>
              </media>
            </file>
          </clipitem>
          <enabled>TRUE</enabled>
          <locked>FALSE</locked>
        </track>
"#,
                cid = cid,
                name = name,
                frames = frames,
                rate = rate_xml,
                rate_inner = format!(
                    "<timebase>{fps}</timebase><ntsc>{ntsc}</ntsc>"
                ),
                start = start,
                end = end,
                fid = fid,
                pathurl = pathurl,
                width = width,
                height = height,
            ));
        } else {
            audio_tracks.push_str(&format!(
                r#"        <track>
          <clipitem id="{cid}">
            <name>{name}</name>
            <enabled>TRUE</enabled>
            <duration>{frames}</duration>
            {rate}
            <start>{start}</start>
            <end>{end}</end>
            <in>0</in>
            <out>{frames}</out>
            <file id="{fid}">
              <name>{name}</name>
              <pathurl>{pathurl}</pathurl>
              <rate>{rate_inner}</rate>
              <duration>{frames}</duration>
              <media>
                <audio>
                  <samplecharacteristics>
                    <depth>16</depth>
                    <samplerate>48000</samplerate>
                  </samplecharacteristics>
                  <channelcount>2</channelcount>
                </audio>
              </media>
            </file>
            <sourcetrack>
              <mediatype>audio</mediatype>
              <trackindex>1</trackindex>
            </sourcetrack>
          </clipitem>
          <enabled>TRUE</enabled>
          <locked>FALSE</locked>
        </track>
"#,
                cid = cid,
                name = name,
                frames = frames,
                rate = rate_xml,
                rate_inner = format!(
                    "<timebase>{fps}</timebase><ntsc>{ntsc}</ntsc>"
                ),
                start = start,
                end = end,
                fid = fid,
                pathurl = pathurl,
            ));
        }
    }

    // Resolve is most reliable with Final Cut Pro 7 / XMEML for multi-track ISO.
    let xmeml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="5">
  <sequence id="sequence-1">
    <name>{project}</name>
    <duration>{frames}</duration>
    {rate}
    <timecode>
      {rate}
      <string>00:00:00:00</string>
      <frame>0</frame>
      <displayformat>NDF</displayformat>
    </timecode>
    <media>
      <video>
        <format>
          <samplecharacteristics>
            {rate}
            <width>{width}</width>
            <height>{height}</height>
            <pixelaspectratio>square</pixelaspectratio>
            <fielddominance>none</fielddominance>
          </samplecharacteristics>
        </format>
{video_tracks}      </video>
      <audio>
{audio_tracks}      </audio>
    </media>
  </sequence>
</xmeml>
"#,
        project = project,
        frames = frames,
        rate = rate_xml,
        width = width,
        height = height,
        video_tracks = video_tracks,
        audio_tracks = audio_tracks,
    );

    let xml_path = out_dir.join(format!("{stem}.xml"));
    std::fs::write(&xml_path, xmeml).map_err(|e| e.to_string())?;

    // Companion FCPXML 1.8 (gap + lanes) for Final Cut / some Premiere workflows.
    let _ = write_fcpxml_companion(
        &out_dir.join(format!("{stem}.fcpxml")),
        project_name,
        &clips
            .iter()
            .map(|c| {
                (
                    c.name.clone(),
                    c.local_path.clone(),
                    c.offset_frames,
                    c.is_video,
                )
            })
            .collect::<Vec<_>>(),
        width,
        height,
        fps,
        frames,
    );

    // OTIO is the most reliable Resolve Free interchange for multi-track ISO.
    let otio_path = out_dir.join(format!("{stem}.otio"));
    let otio_clips: Vec<(String, PathBuf, i64, bool)> = clips
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.local_path.clone(),
                c.offset_frames,
                c.is_video,
            )
        })
        .collect();
    write_otio(&otio_path, project_name, &otio_clips, fps, frames)?;

    let readme = format!(
        "OpenPodcast → DaVinci Resolve\n\
         ============================\n\n\
         IMPORTANT: Do NOT use File → Open on these files.\n\
         You must IMPORT a timeline while a project is already open.\n\n\
         Best file to import (Resolve Free + Studio):\n\
           {otio}\n\n\
         Steps that work\n\
         1. Open Resolve → create or open a project\n\
         2. Switch to the Edit page\n\
         3. In the Media Pool (left), right-click empty space →\n\
            Timelines → Import…\n\
            (or File → Import → Timeline… / Shift-Cmd-I)\n\
         4. Choose the .otio file (fallback: the .xml file)\n\
         5. Confirm the import dialog\n\
         6. Double-click the new timeline in the Media Pool\n\n\
         Media is already H.264 MP4 + WAV in media/\n\
         If clips are offline: right-click → Relink → select media/\n\n\
         Manual fallback (always works)\n\
         1. File → Import → Media… and select everything in media/\n\
         2. Select clips in the Media Pool → right-click →\n\
            Create New Timeline Using Selected Clips\n\
         3. Stack / sync tracks by hand (offsets are in clip names)\n\n\
         Adobe Premiere Pro\n\
         Prefer: {xml}\n\
         1. Open/create a Premiere project\n\
         2. File → Import… (Cmd/Ctrl-I)\n\
         3. Choose the .xml file (Final Cut Pro 7 XML)\n\
         4. Premiere builds a sequence with stacked ISO tracks\n\
         5. If media is offline, relink to the media/ folder\n\n\
         Notes\n\
         - .fcpxml is mainly for Final Cut; Premiere prefers .xml\n\
         - Recent Premiere versions may also import .otio\n\
         - Media is H.264 MP4 + WAV so Premiere can play it\n",
        otio = otio_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("timeline.otio"),
        xml = xml_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("timeline.xml"),
    );
    let _ = std::fs::write(out_dir.join("README-open-in-resolve.txt"), readme);

    // Premiere Pro is the primary target — return the FCP7 .xml path.
    Ok(xml_path)
}

fn prepare_resolve_video(
    ffmpeg: Option<&Path>,
    src: &Path,
    dest: &Path,
    fps: u32,
) -> Result<(), String> {
    let Some(ffmpeg) = ffmpeg else {
        // No ffmpeg — copy as-is and hope Resolve accepts the container.
        return link_or_copy(src, dest);
    };
    // Prefer a Resolve-friendly H.264 MP4. Copy streams when possible.
    let try_copy = Command::new(ffmpeg)
        .args([
            "-y",
            "-i",
            &src.to_string_lossy(),
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            &dest.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("ffmpeg spawn failed: {e}"))?;
    if try_copy.status.success() && dest.is_file() {
        return Ok(());
    }
    let _ = std::fs::remove_file(dest);
    let reencode = Command::new(ffmpeg)
        .args([
            "-y",
            "-i",
            &src.to_string_lossy(),
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-preset",
            "veryfast",
            "-r",
            &fps.to_string(),
            "-an",
            "-movflags",
            "+faststart",
            &dest.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("ffmpeg spawn failed: {e}"))?;
    if reencode.status.success() && dest.is_file() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&reencode.stderr);
    let tail: String = err.chars().rev().take(400).collect::<String>().chars().rev().collect();
    Err(format!("could not prepare video for Resolve: {tail}"))
}

fn write_otio(
    path: &Path,
    project_name: &str,
    clips: &[(String, PathBuf, i64, bool)],
    fps: u32,
    frames: i64,
) -> Result<(), String> {
    let rate = fps as f64;
    let mut video_tracks = Vec::new();
    let mut audio_tracks = Vec::new();

    for (name, local, offset_frames, is_video) in clips {
        let target_url = file_url_localhost(local)?;
        let mut children = Vec::new();
        let off = (*offset_frames).max(0);
        if off > 0 {
            children.push(serde_json::json!({
                "OTIO_SCHEMA": "Gap.1",
                "name": "sync offset",
                "source_range": {
                    "OTIO_SCHEMA": "TimeRange.1",
                    "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": 0.0 },
                    "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": off as f64 }
                }
            }));
        }
        children.push(serde_json::json!({
            "OTIO_SCHEMA": "Clip.1",
            "name": name,
            "source_range": {
                "OTIO_SCHEMA": "TimeRange.1",
                "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": 0.0 },
                "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": frames as f64 }
            },
            "media_reference": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "target_url": target_url,
                "available_range": {
                    "OTIO_SCHEMA": "TimeRange.1",
                    "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": 0.0 },
                    "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": frames as f64 }
                }
            }
        }));

        let track = serde_json::json!({
            "OTIO_SCHEMA": "Track.1",
            "name": name,
            "kind": if *is_video { "Video" } else { "Audio" },
            "children": children
        });
        if *is_video {
            video_tracks.push(track);
        } else {
            audio_tracks.push(track);
        }
    }

    let mut all_tracks = video_tracks;
    all_tracks.extend(audio_tracks);

    let doc = serde_json::json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": project_name,
        "global_start_time": {
            "OTIO_SCHEMA": "RationalTime.1",
            "rate": rate,
            "value": 0.0
        },
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": all_tracks
        }
    });

    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

fn write_fcpxml_companion(
    path: &Path,
    project_name: &str,
    clips: &[(String, PathBuf, i64, bool)],
    width: u32,
    height: u32,
    fps: u32,
    frames: i64,
) -> Result<(), String> {
    let frame_dur = fcpx_frame_duration(fps);
    let seq_dur = format!("{frames}/{fps}s");
    let mut resources = format!(
        r#"    <format id="r1" name="FFVideoFormat{height}p{fps}" frameDuration="{frame_dur}" width="{width}" height="{height}"/>
"#
    );
    let mut spine_kids = String::new();
    let mut next = 2u32;
    let mut v_lane = 1i32;
    let mut a_lane = -1i32;

    for (name, local, offset_frames, is_video) in clips {
        let id = format!("r{next}");
        next += 1;
        let src = file_url_localhost(local)?;
        let ename = xml_escape(name);
        if *is_video {
            resources.push_str(&format!(
                r#"    <asset id="{id}" name="{ename}" start="0s" duration="{seq_dur}" hasVideo="1" hasAudio="0" format="r1">
      <media-rep kind="original-media" src="{src}"/>
    </asset>
"#
            ));
            let lane = v_lane;
            v_lane += 1;
            spine_kids.push_str(&format!(
                r#"            <asset-clip ref="{id}" name="{ename}" lane="{lane}" offset="{off}/{fps}s" duration="{seq_dur}" start="0s"/>
"#,
                id = id,
                ename = ename,
                lane = lane,
                off = (*offset_frames).max(0),
                fps = fps,
                seq_dur = seq_dur,
            ));
        } else {
            resources.push_str(&format!(
                r#"    <asset id="{id}" name="{ename}" start="0s" duration="{seq_dur}" hasVideo="0" hasAudio="1" audioSources="1" audioChannels="2" audioRate="48000">
      <media-rep kind="original-media" src="{src}"/>
    </asset>
"#
            ));
            let lane = a_lane;
            a_lane -= 1;
            spine_kids.push_str(&format!(
                r#"            <asset-clip ref="{id}" name="{ename}" lane="{lane}" offset="{off}/{fps}s" duration="{seq_dur}" start="0s"/>
"#,
                id = id,
                ename = ename,
                lane = lane,
                off = (*offset_frames).max(0),
                fps = fps,
                seq_dur = seq_dur,
            ));
        }
    }

    let project = xml_escape(project_name);
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.8">
  <resources>
{resources}  </resources>
  <library>
    <event name="OpenPodcast">
      <project name="{project}">
        <sequence format="r1" duration="{seq_dur}" tcStart="0s" tcFormat="NDF" audioLayout="stereo" audioRate="48k">
          <spine>
            <gap name="ISO" offset="0s" duration="{seq_dur}">
{spine_kids}            </gap>
          </spine>
        </sequence>
      </project>
    </event>
  </library>
</fcpxml>
"#
    );
    std::fs::write(path, xml).map_err(|e| e.to_string())
}

fn link_or_copy(src: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        if std::fs::hard_link(src, dest).is_ok() {
            return Ok(());
        }
    }
    std::fs::copy(src, dest)
        .map(|_| ())
        .map_err(|e| format!("failed to copy {}: {e}", src.display()))
}

fn fcpx_frame_duration(fps: u32) -> String {
    match fps {
        24 => "100/2400s".into(),
        25 => "1/25s".into(),
        30 => "100/3000s".into(),
        50 => "1/50s".into(),
        60 => "100/6000s".into(),
        n => format!("1/{n}s"),
    }
}

fn file_url_localhost(path: &Path) -> Result<String, String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let abs = abs.canonicalize().unwrap_or(abs);
    // Resolve prefers file://localhost/… on macOS/Windows interchange.
    let url = url::Url::from_file_path(&abs)
        .map_err(|_| format!("invalid media path: {}", path.display()))?;
    let s = url.to_string();
    if s.starts_with("file:///") {
        Ok(format!("file://localhost/{}", s.trim_start_matches("file:///")))
    } else {
        Ok(s)
    }
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&apos;".into(),
            c => c.to_string(),
        })
        .collect()
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
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

fn layout_cells(layout: &str, n: usize, w: u32, h: u32, pad: u32, gap: u32) -> Vec<Cell> {
    let n = n.max(1);
    let pad = pad.max(8);
    let gap = gap.max(8);
    let inner_w = w.saturating_sub(pad * 2).max(320);
    let inner_h = h.saturating_sub(pad * 2).max(180);
    let ox = pad;
    let oy = pad;

    match layout {
        "equal" => {
            // Mirror CSS `auto-fit minmax(220px, 1fr)` at export width.
            let min_col = ((220.0 * (w as f64 / 1280.0)).round() as u32).clamp(160, 360);
            let cols = ((inner_w + gap) / (min_col + gap)).clamp(1, n as u32);
            let rows = ((n as u32) + cols - 1) / cols;
            let slot_w = (inner_w - gap * cols.saturating_sub(1)) / cols;
            let slot_h = (inner_h - gap * rows.saturating_sub(1)) / rows.max(1);
            let (tw, th) = fit_16x9(slot_w, slot_h);
            let grid_w = cols * tw + gap * cols.saturating_sub(1);
            let grid_h = rows * th + gap * rows.saturating_sub(1);
            let grid_x = ox + inner_w.saturating_sub(grid_w) / 2;
            let grid_y = oy + inner_h.saturating_sub(grid_h) / 2;
            (0..n)
                .map(|i| {
                    let col = (i as u32) % cols;
                    let row = (i as u32) / cols;
                    Cell {
                        x: grid_x + col * (tw + gap),
                        y: grid_y + row * (th + gap),
                        w: tw,
                        h: th,
                    }
                })
                .collect()
        }
        "dynamic" => {
            if n == 1 {
                let (tw, th) = fit_16x9(inner_w, inner_h);
                return vec![Cell {
                    x: ox + (inner_w - tw) / 2,
                    y: oy + (inner_h - th) / 2,
                    w: tw,
                    h: th,
                }];
            }
            // CSS: `1fr 0.34fr` → side ≈ 0.34/1.34 of content width.
            let side_slot_w = ((inner_w as f64) * (0.34 / 1.34)).round() as u32;
            let main_slot_w = inner_w.saturating_sub(side_slot_w + gap);
            let (main_w, main_h) = fit_16x9(main_slot_w, inner_h);
            let mut cells = vec![Cell {
                x: ox + main_slot_w.saturating_sub(main_w) / 2,
                y: oy + (inner_h - main_h) / 2,
                w: main_w,
                h: main_h,
            }];
            let side_n = (n - 1).max(1) as u32;
            let side_slot_h = (inner_h - gap * side_n.saturating_sub(1)) / side_n;
            let (side_w, side_h) = fit_16x9(side_slot_w, side_slot_h);
            let side_x = ox + main_slot_w + gap + side_slot_w.saturating_sub(side_w) / 2;
            for i in 0..(n - 1) {
                let slot_y = oy + i as u32 * (side_slot_h + gap);
                cells.push(Cell {
                    x: side_x,
                    y: slot_y + side_slot_h.saturating_sub(side_h) / 2,
                    w: side_w,
                    h: side_h,
                });
            }
            cells
        }
        _ => {
            // normal: host full-width 16:9 on top; guests in a bottom strip
            // (matches flex host `1 1 100%` + guests `1 1 28%`).
            if n == 1 {
                let (tw, th) = fit_16x9(inner_w, inner_h);
                return vec![Cell {
                    x: ox + (inner_w - tw) / 2,
                    y: oy + (inner_h - th) / 2,
                    w: tw,
                    h: th,
                }];
            }
            let side_n = (n - 1).max(1) as u32;
            // Guests want ~28% flex basis; size strip from that 16:9 height.
            let guest_target_w = ((inner_w as f64) * 0.28).round() as u32;
            let (_, guest_target_h) = fit_16x9(guest_target_w, inner_h / 2);
            let strip_h = guest_target_h.min(inner_h / 3).max(inner_h / 5);
            let main_budget = inner_h.saturating_sub(strip_h + gap);
            let (main_w, main_h) = fit_16x9(inner_w, main_budget);
            let mut cells = vec![Cell {
                x: ox + (inner_w - main_w) / 2,
                y: oy + main_budget.saturating_sub(main_h) / 2,
                w: main_w,
                h: main_h,
            }];
            let strip_slot_w = (inner_w - gap * side_n.saturating_sub(1)) / side_n;
            let (sw, sh) = fit_16x9(strip_slot_w, strip_h);
            let strip_total_w = side_n * sw + gap * side_n.saturating_sub(1);
            let strip_x0 = ox + inner_w.saturating_sub(strip_total_w) / 2;
            let strip_y = oy + main_budget + gap + strip_h.saturating_sub(sh) / 2;
            for i in 0..(n - 1) {
                cells.push(Cell {
                    x: strip_x0 + i as u32 * (sw + gap),
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
    let meta_by: HashMap<String, (&str, &str)> = timeline
        .participants
        .iter()
        .map(|p| (p.id.clone(), (p.name.as_str(), p.role.as_str())))
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
            let entry = by_id.entry(pid.clone()).or_insert_with(|| {
                let (pname, prole) = meta_by
                    .get(&pid)
                    .map(|(n, r)| ((*n).to_string(), (*r).to_string()))
                    .unwrap_or_else(|| (pid.clone(), String::new()));
                TrackMedia {
                    participant_id: pid.clone(),
                    name: pname,
                    role: prole,
                    video: None,
                    audio: None,
                    offset_ms: *timeline.offsets_ms.get(&pid).unwrap_or(&0),
                }
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

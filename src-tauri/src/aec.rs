//! Native WebRTC AEC3 for the live P2P preview path.
//!
//! Far-end (what peers send us, which we play) is fed as the AEC *render* reference.
//! Near-end mic PCM is cleaned with `process_capture` before we send it out.
//!
//! Runs at 16 kHz to match the preview wire format. Frame size is 10 ms (160 samples).
//! Requires `meson` + `ninja` on the build machine (bundled webrtc-audio-processing).

use base64::Engine;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use webrtc_audio_processing::config::{
    EchoCanceller, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel,
};
use webrtc_audio_processing::{Config, Processor};

const SAMPLE_RATE: u32 = 16_000;
/// Matches useStudioPreview jitter target (~120ms) + a little slack for AudioContext.
const DEFAULT_STREAM_DELAY_MS: u16 = 140;

pub struct AecEngine {
    /// Single lock covers processor + pending buffers (render+capture must stay ordered).
    inner: Mutex<AecInner>,
    enabled: AtomicBool,
    stream_delay_ms: AtomicU16,
    frame_len: usize,
}

struct AecInner {
    processor: Processor,
    render_pending: Vec<f32>,
    capture_pending: Vec<f32>,
}

impl AecEngine {
    pub fn new() -> Result<Arc<Self>, String> {
        let processor = Processor::new(SAMPLE_RATE).map_err(|e| e.to_string())?;
        let frame_len = processor.num_samples_per_frame();
        let engine = Arc::new(Self {
            inner: Mutex::new(AecInner {
                processor,
                render_pending: Vec::with_capacity(frame_len * 4),
                capture_pending: Vec::with_capacity(frame_len * 4),
            }),
            enabled: AtomicBool::new(true),
            stream_delay_ms: AtomicU16::new(DEFAULT_STREAM_DELAY_MS),
            frame_len,
        });
        engine.apply_config(true, DEFAULT_STREAM_DELAY_MS);
        Ok(engine)
    }

    pub fn set_enabled(&self, enabled: bool) {
        let was = self.enabled.swap(enabled, Ordering::Relaxed);
        if was != enabled {
            self.apply_config(enabled, self.stream_delay_ms.load(Ordering::Relaxed));
            let mut inner = self.inner.lock();
            if enabled {
                inner.processor.reinitialize();
            }
            inner.render_pending.clear();
            inner.capture_pending.clear();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_stream_delay_ms(&self, delay_ms: u16) {
        self.stream_delay_ms.store(delay_ms, Ordering::Relaxed);
        if self.enabled() {
            self.apply_config(true, delay_ms);
        }
    }

    fn apply_config(&self, enabled: bool, delay_ms: u16) {
        self.inner.lock().processor.set_config(Config {
            echo_canceller: enabled.then_some(EchoCanceller::Full {
                stream_delay_ms: Some(delay_ms),
            }),
            noise_suppression: enabled.then_some(NoiseSuppression {
                level: NoiseSuppressionLevel::Moderate,
                analyze_linear_aec_output: false,
            }),
            high_pass_filter: enabled.then_some(HighPassFilter {
                apply_in_full_band: true,
            }),
            ..Config::default()
        });
    }

    /// Far-end reference: PCM we are about to play (or just received for playback).
    pub fn feed_render_pcm16(&self, pcm: &[i16]) {
        if !self.enabled() || pcm.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        inner
            .render_pending
            .extend(pcm.iter().map(|s| *s as f32 / 32768.0));
        while inner.render_pending.len() >= self.frame_len {
            let frame: Vec<f32> = inner.render_pending.drain(..self.frame_len).collect();
            let _ = inner.processor.analyze_render_frame([&frame[..]]);
        }
    }

    /// Near-end capture: returns echo-cancelled PCM (same length as input).
    pub fn process_capture_pcm16(&self, pcm: &[i16]) -> Vec<i16> {
        if !self.enabled() || pcm.is_empty() {
            return pcm.to_vec();
        }
        let mut inner = self.inner.lock();
        inner
            .capture_pending
            .extend(pcm.iter().map(|s| *s as f32 / 32768.0));

        let mut out_f = Vec::with_capacity(pcm.len());
        while inner.capture_pending.len() >= self.frame_len {
            let mut frame: Vec<f32> = inner.capture_pending.drain(..self.frame_len).collect();
            if inner
                .processor
                .process_capture_frame(&mut [&mut frame[..]])
                .is_ok()
            {
                out_f.extend_from_slice(&frame);
            } else {
                out_f.extend_from_slice(&frame);
            }
        }

        if out_f.is_empty() {
            return pcm.to_vec();
        }

        let mut out: Vec<i16> = out_f
            .into_iter()
            .map(|s| {
                (s.clamp(-1.0, 1.0) * 32767.0)
                    .round()
                    .clamp(i16::MIN as f32, i16::MAX as f32) as i16
            })
            .collect();

        if out.len() < pcm.len() {
            out.extend_from_slice(&pcm[out.len()..]);
        } else if out.len() > pcm.len() {
            out.truncate(pcm.len());
        }
        out
    }

    pub fn feed_render_b64(&self, pcm_b64: &str) {
        if let Some(samples) = decode_pcm16_b64(pcm_b64) {
            self.feed_render_pcm16(&samples);
        }
    }

    /// Echo-cancel outbound mic PCM (base64). Returns original on empty/disabled.
    pub fn process_capture_b64(&self, pcm_b64: &str) -> String {
        let Some(samples) = decode_pcm16_b64(pcm_b64) else {
            return pcm_b64.to_string();
        };
        let cleaned = self.process_capture_pcm16(&samples);
        encode_pcm16_b64(&cleaned)
    }
}

fn decode_pcm16_b64(b64: &str) -> Option<Vec<i16>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .ok()?;
    if bytes.len() < 2 {
        return None;
    }
    let n = bytes.len() / 2;
    let mut out = Vec::with_capacity(n);
    for chunk in bytes.chunks_exact(2) {
        out.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Some(out)
}

fn encode_pcm16_b64(samples: &[i16]) -> String {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

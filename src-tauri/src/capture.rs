use crate::eq::EqChain;
use crate::models::{DeviceInfo, EqBand};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

/// Laptop / USB mics often peak well below full scale. Boost into a usable
/// range, then soft-limit so we don't hard-clip on louder speech.
const INPUT_GAIN: f32 = 3.0; // ~9.5 dB

fn amplify_f32(sample: f32) -> i16 {
    // Soft clip via tanh keeps peaks musical instead of brickwall-distorted
    let shaped = (sample * INPUT_GAIN).tanh();
    (shaped * i16::MAX as f32)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

pub struct AudioRecorder {
    stop: Arc<AtomicBool>,
    level: Arc<Mutex<f32>>,
    path: PathBuf,
    join: Option<std::thread::JoinHandle<()>>,
}

pub struct EqCaptureConfig {
    pub bands: Vec<EqBand>,
    pub bypass: bool,
}

impl AudioRecorder {
    pub fn list_input_devices() -> Result<Vec<DeviceInfo>, String> {
        let host = cpal::default_host();
        let mut devices = Vec::new();
        if let Ok(iter) = host.input_devices() {
            for (i, d) in iter.enumerate() {
                let label = d.name().unwrap_or_else(|_| format!("Input {i}"));
                devices.push(DeviceInfo {
                    id: label.clone(),
                    label,
                    kind: "audioinput".into(),
                });
            }
        }
        Ok(devices)
    }

    pub fn start(
        path: PathBuf,
        device_name: Option<String>,
        eq: Option<EqCaptureConfig>,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(Mutex::new(0.0f32));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let stop_c = stop.clone();
        let level_c = level.clone();
        let path_c = path.clone();
        let eq_cfg = eq.unwrap_or(EqCaptureConfig {
            bands: Vec::new(),
            bypass: true,
        });

        let join = std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let host = cpal::default_host();
                let device = if let Some(name) = device_name {
                    host.input_devices()
                        .map_err(|e| e.to_string())?
                        .find(|d| d.name().ok().as_ref() == Some(&name))
                        .ok_or_else(|| format!("Audio device not found: {name}"))?
                } else {
                    host.default_input_device()
                        .ok_or_else(|| "No default input device".to_string())?
                };

                let config = device
                    .default_input_config()
                    .map_err(|e| e.to_string())?;
                let sample_rate = config.sample_rate().0;
                let channels = config.channels();
                let sample_format = config.sample_format();

                let spec = WavSpec {
                    channels,
                    sample_rate,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let writer = WavWriter::create(&path_c, spec).map_err(|e| e.to_string())?;
                let writer = Arc::new(Mutex::new(Some(writer)));
                let writer_cb = writer.clone();
                let stop_cb = stop_c.clone();
                let level_cb = level_c.clone();

                let eq_chain = Arc::new(Mutex::new(EqChain::new(
                    &eq_cfg.bands,
                    eq_cfg.bypass,
                    sample_rate as f32,
                    channels as usize,
                )));

                let stream = match sample_format {
                    cpal::SampleFormat::F32 => {
                        let eq_cb = eq_chain.clone();
                        device
                            .build_input_stream(
                                &config.into(),
                                move |data: &[f32], _| {
                                    if stop_cb.load(Ordering::Relaxed) {
                                        return;
                                    }
                                    let mut peak = 0.0f32;
                                    let mut eq = eq_cb.lock();
                                    if let Some(w) = writer_cb.lock().as_mut() {
                                        let ch = channels as usize;
                                        for (i, &s) in data.iter().enumerate() {
                                            let processed =
                                                eq.process_sample_f32(s, i % ch.max(1));
                                            peak = peak.max((processed * INPUT_GAIN).abs().min(1.0));
                                            let _ = w.write_sample(amplify_f32(processed));
                                        }
                                    }
                                    *level_cb.lock() = peak;
                                },
                                |e| eprintln!("audio stream error: {e}"),
                                None,
                            )
                            .map_err(|e| e.to_string())?
                    }
                    cpal::SampleFormat::I16 => {
                        let eq_cb = eq_chain.clone();
                        device
                            .build_input_stream(
                                &config.into(),
                                move |data: &[i16], _| {
                                    if stop_cb.load(Ordering::Relaxed) {
                                        return;
                                    }
                                    let mut peak = 0.0f32;
                                    let mut eq = eq_cb.lock();
                                    if let Some(w) = writer_cb.lock().as_mut() {
                                        let ch = channels as usize;
                                        for (i, &s) in data.iter().enumerate() {
                                            let f = s as f32 / i16::MAX as f32;
                                            let processed =
                                                eq.process_sample_f32(f, i % ch.max(1));
                                            let out = amplify_f32(processed);
                                            peak = peak
                                                .max((out as f32).abs() / i16::MAX as f32);
                                            let _ = w.write_sample(out);
                                        }
                                    }
                                    *level_cb.lock() = peak;
                                },
                                |e| eprintln!("audio stream error: {e}"),
                                None,
                            )
                            .map_err(|e| e.to_string())?
                    }
                    other => return Err(format!("Unsupported sample format: {other:?}")),
                };

                stream.play().map_err(|e| e.to_string())?;
                let _ = ready_tx.send(Ok(()));

                while !stop_c.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                drop(stream);
                if let Some(w) = writer.lock().take() {
                    let _ = w.finalize();
                }
                Ok(())
            })();

            if let Err(e) = &result {
                let _ = ready_tx.send(Err(e.clone()));
            }
        });

        ready_rx
            .recv()
            .map_err(|_| "Audio thread ended early".to_string())??;

        Ok(Self {
            stop,
            level,
            path,
            join: Some(join),
        })
    }

    pub fn level(&self) -> f32 {
        *self.level.lock()
    }

    pub fn stop(mut self) -> PathBuf {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = j.join();
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(std::time::Duration::from_secs(8));
        }
        self.path
    }
}

/// Pad the start of a WAV with silence so guest tracks align to host clock.
pub fn pad_wav_start(path: &PathBuf, pad_ms: u64) -> Result<(), String> {
    if pad_ms == 0 {
        return Ok(());
    }
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let pad_samples =
        (spec.sample_rate as u64 * pad_ms / 1000) as usize * spec.channels as usize;
    let tmp = path.with_extension("padded.wav");
    {
        let mut writer = WavWriter::create(&tmp, spec).map_err(|e| e.to_string())?;
        for _ in 0..pad_samples {
            writer.write_sample(0i16).map_err(|e| e.to_string())?;
        }
        for s in samples {
            writer.write_sample(s).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

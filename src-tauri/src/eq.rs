//! RBJ cookbook biquad cascade for HQ WAV capture (mirrors Web Audio voice EQ).

use crate::models::EqBand;

#[derive(Clone, Copy, Default)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Default)]
struct BiquadState {
    coeffs: BiquadCoeffs,
    z1: f32,
    z2: f32,
}

impl BiquadState {
    fn process(&mut self, x: f32) -> f32 {
        let c = self.coeffs;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }
}

fn design(band: &EqBand, sample_rate: f32) -> BiquadCoeffs {
    if !band.enabled {
        return BiquadCoeffs {
            b0: 1.0,
            ..Default::default()
        };
    }
    let freq = band.frequency.clamp(20.0, sample_rate * 0.49);
    let q = band.q.clamp(0.1, 18.0);
    let a = 10f32.powf(band.gain / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
    let cos = w0.cos();
    let sin = w0.sin();
    let alpha = sin / (2.0 * q);

    let (mut b0, mut b1, mut b2, mut a0, mut a1, mut a2) = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    match band.band_type.as_str() {
        "lowpass" => {
            b0 = (1.0 - cos) / 2.0;
            b1 = 1.0 - cos;
            b2 = (1.0 - cos) / 2.0;
            a0 = 1.0 + alpha;
            a1 = -2.0 * cos;
            a2 = 1.0 - alpha;
        }
        "highpass" => {
            b0 = (1.0 + cos) / 2.0;
            b1 = -(1.0 + cos);
            b2 = (1.0 + cos) / 2.0;
            a0 = 1.0 + alpha;
            a1 = -2.0 * cos;
            a2 = 1.0 - alpha;
        }
        "peaking" => {
            b0 = 1.0 + alpha * a;
            b1 = -2.0 * cos;
            b2 = 1.0 - alpha * a;
            a0 = 1.0 + alpha / a;
            a1 = -2.0 * cos;
            a2 = 1.0 - alpha / a;
        }
        "lowshelf" => {
            let two = 2.0 * a.sqrt() * alpha;
            b0 = a * (a + 1.0 - (a - 1.0) * cos + two);
            b1 = 2.0 * a * (a - 1.0 - (a + 1.0) * cos);
            b2 = a * (a + 1.0 - (a - 1.0) * cos - two);
            a0 = a + 1.0 + (a - 1.0) * cos + two;
            a1 = -2.0 * (a - 1.0 + (a + 1.0) * cos);
            a2 = a + 1.0 + (a - 1.0) * cos - two;
        }
        "highshelf" => {
            let two = 2.0 * a.sqrt() * alpha;
            b0 = a * (a + 1.0 + (a - 1.0) * cos + two);
            b1 = -2.0 * a * (a - 1.0 + (a + 1.0) * cos);
            b2 = a * (a + 1.0 + (a - 1.0) * cos - two);
            a0 = a + 1.0 - (a - 1.0) * cos + two;
            a1 = 2.0 * (a - 1.0 - (a + 1.0) * cos);
            a2 = a + 1.0 - (a - 1.0) * cos - two;
        }
        "notch" => {
            b0 = 1.0;
            b1 = -2.0 * cos;
            b2 = 1.0;
            a0 = 1.0 + alpha;
            a1 = -2.0 * cos;
            a2 = 1.0 - alpha;
        }
        _ => {
            return BiquadCoeffs {
                b0: 1.0,
                ..Default::default()
            };
        }
    }

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// Multi-band EQ applied per channel in the HQ WAV path.
#[derive(Clone, Default)]
pub struct EqChain {
    bypass: bool,
    /// One cascade per channel (mono = 1).
    channels: Vec<Vec<BiquadState>>,
}

impl EqChain {
    pub fn new(bands: &[EqBand], bypass: bool, sample_rate: f32, channel_count: usize) -> Self {
        let ch = channel_count.max(1);
        let cascade: Vec<BiquadState> = if bypass || bands.is_empty() {
            Vec::new()
        } else {
            bands
                .iter()
                .map(|b| BiquadState {
                    coeffs: design(b, sample_rate),
                    z1: 0.0,
                    z2: 0.0,
                })
                .collect()
        };
        Self {
            bypass: bypass || cascade.is_empty(),
            channels: (0..ch).map(|_| cascade.clone()).collect(),
        }
    }

    pub fn process_interleaved_f32(&mut self, frame: &mut [f32]) {
        if self.bypass {
            return;
        }
        let ch = self.channels.len();
        if ch == 0 {
            return;
        }
        for (i, s) in frame.iter_mut().enumerate() {
            let c = i % ch;
            let mut x = *s;
            for biquad in &mut self.channels[c] {
                x = biquad.process(x);
            }
            *s = x;
        }
    }

    pub fn process_sample_f32(&mut self, sample: f32, channel: usize) -> f32 {
        if self.bypass {
            return sample;
        }
        let Some(cascade) = self.channels.get_mut(channel) else {
            return sample;
        };
        let mut x = sample;
        for biquad in cascade {
            x = biquad.process(x);
        }
        // Soft knee near full-scale so EQ boosts don't brickwall before INPUT_GAIN.
        if x.abs() > 0.85 {
            let sign = if x >= 0.0 { 1.0 } else { -1.0 };
            x = sign * (0.85 + 0.15 * (x.abs() - 0.85).tanh());
        }
        x
    }
}

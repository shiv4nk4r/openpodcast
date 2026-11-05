/** Shared DSP helpers for live mic / P2P audio. */

/** One-pole low-pass (anti-alias) before downsampling to 16 kHz. */
export function lowpassInPlace(samples: Float32Array, cutoffHz: number, sampleRate: number) {
  if (samples.length === 0) return;
  const rc = 1 / (2 * Math.PI * cutoffHz);
  const dt = 1 / sampleRate;
  const alpha = dt / (rc + dt);
  let prev = samples[0];
  for (let i = 0; i < samples.length; i++) {
    prev = prev + alpha * (samples[i] - prev);
    samples[i] = prev;
  }
}

/** Hermite cubic resample — smoother than linear for 48k↔16k voice. */
export function resampleCubic(
  input: Float32Array,
  fromRate: number,
  toRate: number,
): Float32Array {
  if (fromRate === toRate || input.length === 0) return input;
  const outLen = Math.max(1, Math.round((input.length * toRate) / fromRate));
  const out = new Float32Array(outLen);
  const scale = (input.length - 1) / Math.max(1, outLen - 1);
  const n = input.length;
  for (let i = 0; i < outLen; i++) {
    const src = i * scale;
    const i1 = Math.floor(src);
    const t = src - i1;
    const i0 = Math.max(0, i1 - 1);
    const i2 = Math.min(n - 1, i1 + 1);
    const i3 = Math.min(n - 1, i1 + 2);
    const y0 = input[i0];
    const y1 = input[i1];
    const y2 = input[i2];
    const y3 = input[i3];
    const c0 = y1;
    const c1 = 0.5 * (y2 - y0);
    const c2 = y0 - 2.5 * y1 + 2 * y2 - 0.5 * y3;
    const c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    out[i] = ((c3 * t + c2) * t + c1) * t + c0;
  }
  return out;
}

/** Soft clip that stays transparent until ~0.85, then rounds off peaks. */
export function softClip(sample: number): number {
  const a = Math.abs(sample);
  if (a <= 0.85) return sample;
  const sign = sample < 0 ? -1 : 1;
  return sign * (0.85 + 0.15 * Math.tanh((a - 0.85) * 2.2));
}

/**
 * Apply a short equal-power fade across a boundary so packet joins / underrun
 * recoveries don’t click.
 */
export function crossfadeInPlace(
  samples: Float32Array,
  fromValue: number,
  fadeSamples: number,
) {
  const n = Math.min(fadeSamples, samples.length);
  if (n <= 1) return;
  for (let i = 0; i < n; i++) {
    const t = i / (n - 1);
    // Equal-power-ish blend from held value into new audio.
    const a = Math.cos(t * Math.PI * 0.5);
    const b = Math.sin(t * Math.PI * 0.5);
    samples[i] = fromValue * a + samples[i] * b;
  }
}

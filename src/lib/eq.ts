/** ReaEQ-style parametric voice EQ — shared types, builtins, and DSP helpers. */

export type EqBandType =
  | "highpass"
  | "lowshelf"
  | "peaking"
  | "highshelf"
  | "lowpass"
  | "notch";

export interface EqBand {
  id: string;
  type: EqBandType;
  frequency: number;
  gain: number;
  q: number;
  enabled: boolean;
}

export interface EqProfile {
  id: string;
  name: string;
  bands: EqBand[];
  bypass: boolean;
  /** Built-in presets are read-only (Save As to customize). */
  builtin?: boolean;
}

export const EQ_BAND_TYPES: { id: EqBandType; label: string }[] = [
  { id: "highpass", label: "HP" },
  { id: "lowshelf", label: "LS" },
  { id: "peaking", label: "Peak" },
  { id: "highshelf", label: "HS" },
  { id: "lowpass", label: "LP" },
  { id: "notch", label: "Notch" },
];

export const SIMPLE_LOW_HZ = 120;
export const SIMPLE_MID_HZ = 1000;
export const SIMPLE_HIGH_HZ = 6000;

function band(
  id: string,
  type: EqBandType,
  frequency: number,
  gain: number,
  q: number,
  enabled = true,
): EqBand {
  return { id, type, frequency, gain, q, enabled };
}

/** Flat / identity — three simple bands at 0 dB. */
export function makeFlatBands(): EqBand[] {
  return [
    band("simple-low", "lowshelf", SIMPLE_LOW_HZ, 0, 0.7),
    band("simple-mid", "peaking", SIMPLE_MID_HZ, 0, 1.0),
    band("simple-high", "highshelf", SIMPLE_HIGH_HZ, 0, 0.7),
  ];
}

/** Fixed Web Audio EQ slots so profile switches never remount the MediaStream. */
export const MAX_EQ_SLOTS = 8;

/**
 * Voice profiles sit on top of the always-on polish chain (rumble HPF, mild
 * de-ess, speech leveler, soft peak limit). Keep these musical and under ~±2.5 dB.
 */
export const BUILTIN_EQ_PROFILES: EqProfile[] = [
  {
    id: "flat",
    name: "Flat",
    builtin: true,
    bypass: false,
    // Subtle polish even on “Flat” — always-on chain does most of the work.
    bands: makeFlatBands(),
  },
  {
    // Default podcast voice: clearer midrange, less mud.
    id: "broadcast",
    name: "Broadcast",
    builtin: true,
    bypass: false,
    bands: [
      band("bc-mud", "peaking", 240, -2.0, 0.85),
      band("bc-box", "peaking", 550, -0.8, 1.1),
      band("bc-presence", "peaking", 2800, 2.0, 0.9),
      band("bc-air", "highshelf", 9500, 1.0, 0.7),
    ],
  },
  {
    // Rounder chest; softer top for bright condensers.
    id: "warm",
    name: "Warm",
    builtin: true,
    bypass: false,
    bands: [
      band("wm-body", "lowshelf", 150, 2.0, 0.7),
      band("wm-mud", "peaking", 300, -1.0, 1.0),
      band("wm-harsh", "peaking", 4200, -1.5, 1.1),
      band("wm-air", "highshelf", 9000, -0.8, 0.7),
    ],
  },
  {
    // Clarity for dull / distant mics.
    id: "bright",
    name: "Bright",
    builtin: true,
    bypass: false,
    bands: [
      band("br-mud", "peaking", 250, -1.5, 0.9),
      band("br-presence", "peaking", 3600, 2.2, 0.95),
      band("br-sib", "peaking", 7200, -1.2, 1.6),
      band("br-air", "highshelf", 10500, 1.2, 0.7),
    ],
  },
  {
    // Extra low cut for desk / HVAC / handling noise.
    id: "cut-rumble",
    name: "Cut rumble",
    builtin: true,
    bypass: false,
    bands: [
      band("cr-hp", "highpass", 110, 0, 0.707),
      band("cr-low", "lowshelf", 150, -3.0, 0.7),
      band("cr-mud", "peaking", 220, -1.2, 1.0),
      band("cr-presence", "peaking", 3000, 0.6, 1.0),
    ],
  },
  {
    // Deeper chest / baritone weight.
    id: "deep",
    name: "Deep",
    builtin: true,
    bypass: false,
    bands: [
      band("dp-chest", "lowshelf", 135, 2.2, 0.7),
      band("dp-mud", "peaking", 320, -1.0, 1.0),
      band("dp-presence", "peaking", 2200, 0.6, 1.0),
      band("dp-air", "highshelf", 9000, -0.8, 0.7),
    ],
  },
  {
    // Lighter / higher voices — less boom, open top.
    id: "high",
    name: "High",
    builtin: true,
    bypass: false,
    bands: [
      band("hi-hp", "highpass", 100, 0, 0.707),
      band("hi-low", "lowshelf", 170, -2.5, 0.7),
      band("hi-presence", "peaking", 3500, 1.6, 1.0),
      band("hi-air", "highshelf", 10500, 1.2, 0.7),
    ],
  },
];

/** Pad / trim to a fixed filter count so the live graph never remounts. */
export function padBandsToSlots(bands: EqBand[]): EqBand[] {
  const slots: EqBand[] = bands.slice(0, MAX_EQ_SLOTS).map((b) => ({ ...b }));
  while (slots.length < MAX_EQ_SLOTS) {
    slots.push({
      id: `slot-${slots.length}`,
      type: "peaking",
      frequency: 1000,
      gain: 0,
      q: 1,
      enabled: false,
    });
  }
  return slots;
}

/**
 * Makeup gain (linear) so cut-heavy profiles don’t sound quieter than Flat.
 * Kept modest — limiter after EQ catches residual peaks.
 */
export function makeupGainForProfile(profile: EqProfile): number {
  if (profile.bypass) return 1;
  let cut = 0;
  let boost = 0;
  for (const b of profile.bands) {
    if (!b.enabled) continue;
    if (b.type === "highpass" || b.type === "lowpass" || b.type === "notch") {
      cut += 1.5;
      continue;
    }
    if (b.gain < 0) cut += -b.gain;
    else boost += b.gain;
  }
  // Recover cut energy + a little headroom for the speech leveler.
  const db = Math.min(6, Math.max(1.0, cut * 0.4 - boost * 0.12 + 1.0));
  return Math.pow(10, db / 20);
}

/** Best out-of-the-box podcast sound for new installs. */
export const DEFAULT_EQ_PROFILE_ID = "broadcast";

export function newBandId(): string {
  return crypto.randomUUID();
}

export function cloneProfile(p: EqProfile, overrides?: Partial<EqProfile>): EqProfile {
  return {
    ...p,
    ...overrides,
    bands: (overrides?.bands ?? p.bands).map((b) => ({ ...b })),
  };
}

export function resolveEqProfile(
  activeId: string | null | undefined,
  saved: EqProfile[] | null | undefined,
): EqProfile {
  const id = activeId || DEFAULT_EQ_PROFILE_ID;
  const fromSaved = (saved ?? []).find((p) => p.id === id);
  if (fromSaved) return cloneProfile(fromSaved);
  const builtin = BUILTIN_EQ_PROFILES.find((p) => p.id === id);
  if (builtin) return cloneProfile(builtin);
  return cloneProfile(BUILTIN_EQ_PROFILES[0]);
}

export function allEqProfiles(saved: EqProfile[] | null | undefined): EqProfile[] {
  return [...BUILTIN_EQ_PROFILES, ...(saved ?? [])];
}

/** Ensure Low/Mid/High trio exists for Simple mode edits. */
export function ensureSimpleTrio(bands: EqBand[]): EqBand[] {
  const next = bands.map((b) => ({ ...b }));
  const hasLow = next.some((b) => b.type === "lowshelf");
  const hasMid = next.some((b) => b.type === "peaking");
  const hasHigh = next.some((b) => b.type === "highshelf");
  if (!hasLow) {
    next.unshift(band(newBandId(), "lowshelf", SIMPLE_LOW_HZ, 0, 0.7));
  }
  if (!hasMid) {
    const mid = band(newBandId(), "peaking", SIMPLE_MID_HZ, 0, 1.0);
    const insertAt = Math.min(1, next.length);
    next.splice(insertAt, 0, mid);
  }
  if (!hasHigh) {
    next.push(band(newBandId(), "highshelf", SIMPLE_HIGH_HZ, 0, 0.7));
  }
  return next;
}

export function readSimpleGains(bands: EqBand[]): {
  low: number;
  mid: number;
  high: number;
} {
  const trio = ensureSimpleTrio(bands);
  const low = trio.find((b) => b.type === "lowshelf")?.gain ?? 0;
  const mid =
    trio.find(
      (b) =>
        b.type === "peaking" &&
        b.frequency >= 400 &&
        b.frequency <= 4000,
    )?.gain ??
    trio.find((b) => b.type === "peaking")?.gain ??
    0;
  const high = trio.find((b) => b.type === "highshelf")?.gain ?? 0;
  return { low, mid, high };
}

export function applySimpleGains(
  bands: EqBand[],
  gains: { low: number; mid: number; high: number },
): EqBand[] {
  const next = ensureSimpleTrio(bands);
  let midDone = false;
  return next.map((b) => {
    if (b.type === "lowshelf") return { ...b, gain: gains.low, frequency: SIMPLE_LOW_HZ };
    if (b.type === "highshelf")
      return { ...b, gain: gains.high, frequency: SIMPLE_HIGH_HZ };
    if (
      !midDone &&
      b.type === "peaking" &&
      b.frequency >= 400 &&
      b.frequency <= 4000
    ) {
      midDone = true;
      return { ...b, gain: gains.mid, frequency: SIMPLE_MID_HZ };
    }
    if (!midDone && b.type === "peaking") {
      midDone = true;
      return { ...b, gain: gains.mid, frequency: SIMPLE_MID_HZ };
    }
    return b;
  });
}

export function defaultPeakingBand(frequency = 1000): EqBand {
  return band(newBandId(), "peaking", frequency, 0, 1.0);
}

/** Map Web Audio BiquadFilterType ↔ our types. */
export function toBiquadType(t: EqBandType): BiquadFilterType {
  return t;
}

export function applyBandToFilter(filter: BiquadFilterNode, band: EqBand, bypass: boolean) {
  const now = filter.context.currentTime;
  // Snap params (avoid setTarget + type swaps, which can ring/click mid-stream).
  if (bypass || !band.enabled) {
    filter.type = "allpass";
    filter.frequency.setValueAtTime(1000, now);
    filter.Q.setValueAtTime(1, now);
    filter.gain.setValueAtTime(0, now);
    return;
  }
  filter.type = toBiquadType(band.type);
  filter.frequency.setValueAtTime(
    Math.max(20, Math.min(20000, band.frequency)),
    now,
  );
  filter.Q.setValueAtTime(Math.max(0.1, Math.min(18, band.q)), now);
  filter.gain.setValueAtTime(band.gain, now);
}

// —— Frequency response (for SVG curve) ————————————————————————————————

type Complex = { re: number; im: number };

function cMul(a: Complex, b: Complex): Complex {
  return { re: a.re * b.re - a.im * b.im, im: a.re * b.im + a.im * b.re };
}

function cDiv(a: Complex, b: Complex): Complex {
  const d = b.re * b.re + b.im * b.im;
  return {
    re: (a.re * b.re + a.im * b.im) / d,
    im: (a.im * b.re - a.re * b.im) / d,
  };
}

/** RBJ cookbook coeffs → complex H(z) at normalized ω. */
function bandTransfer(band: EqBand, sampleRate: number, freqHz: number): Complex {
  if (!band.enabled) return { re: 1, im: 0 };
  const A = Math.pow(10, band.gain / 40);
  const w0 = (2 * Math.PI * band.frequency) / sampleRate;
  const cos = Math.cos(w0);
  const sin = Math.sin(w0);
  const alpha = sin / (2 * Math.max(0.1, band.q));
  let b0 = 1,
    b1 = 0,
    b2 = 0,
    a0 = 1,
    a1 = 0,
    a2 = 0;

  switch (band.type) {
    case "lowpass":
      b0 = (1 - cos) / 2;
      b1 = 1 - cos;
      b2 = (1 - cos) / 2;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
    case "highpass":
      b0 = (1 + cos) / 2;
      b1 = -(1 + cos);
      b2 = (1 + cos) / 2;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
    case "peaking": {
      b0 = 1 + alpha * A;
      b1 = -2 * cos;
      b2 = 1 - alpha * A;
      a0 = 1 + alpha / A;
      a1 = -2 * cos;
      a2 = 1 - alpha / A;
      break;
    }
    case "lowshelf": {
      const twoSqrtAAlpha = 2 * Math.sqrt(A) * alpha;
      b0 = A * (A + 1 - (A - 1) * cos + twoSqrtAAlpha);
      b1 = 2 * A * (A - 1 - (A + 1) * cos);
      b2 = A * (A + 1 - (A - 1) * cos - twoSqrtAAlpha);
      a0 = A + 1 + (A - 1) * cos + twoSqrtAAlpha;
      a1 = -2 * (A - 1 + (A + 1) * cos);
      a2 = A + 1 + (A - 1) * cos - twoSqrtAAlpha;
      break;
    }
    case "highshelf": {
      const twoSqrtAAlpha = 2 * Math.sqrt(A) * alpha;
      b0 = A * (A + 1 + (A - 1) * cos + twoSqrtAAlpha);
      b1 = -2 * A * (A - 1 + (A + 1) * cos);
      b2 = A * (A + 1 + (A - 1) * cos - twoSqrtAAlpha);
      a0 = A + 1 - (A - 1) * cos + twoSqrtAAlpha;
      a1 = 2 * (A - 1 - (A + 1) * cos);
      a2 = A + 1 - (A - 1) * cos - twoSqrtAAlpha;
      break;
    }
    case "notch":
      b0 = 1;
      b1 = -2 * cos;
      b2 = 1;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
  }

  b0 /= a0;
  b1 /= a0;
  b2 /= a0;
  a1 /= a0;
  a2 /= a0;

  const w = (2 * Math.PI * freqHz) / sampleRate;
  const z1: Complex = { re: Math.cos(w), im: -Math.sin(w) };
  const z2 = cMul(z1, z1);
  const num = {
    re: b0 + b1 * z1.re + b2 * z2.re,
    im: b1 * z1.im + b2 * z2.im,
  };
  const den = {
    re: 1 + a1 * z1.re + a2 * z2.re,
    im: a1 * z1.im + a2 * z2.im,
  };
  return cDiv(num, den);
}

export function eqMagnitudeDb(
  bands: EqBand[],
  freqHz: number,
  sampleRate = 48000,
  bypass = false,
): number {
  if (bypass) return 0;
  let h: Complex = { re: 1, im: 0 };
  for (const b of bands) {
    h = cMul(h, bandTransfer(b, sampleRate, freqHz));
  }
  const mag = Math.sqrt(h.re * h.re + h.im * h.im);
  return 20 * Math.log10(Math.max(1e-9, mag));
}

export function buildResponseCurve(
  bands: EqBand[],
  opts?: { points?: number; bypass?: boolean; sampleRate?: number },
): { f: number; db: number }[] {
  const n = opts?.points ?? 128;
  const bypass = opts?.bypass ?? false;
  const sampleRate = opts?.sampleRate ?? 48000;
  const out: { f: number; db: number }[] = [];
  const fMin = 20;
  const fMax = 20000;
  for (let i = 0; i < n; i++) {
    const t = i / (n - 1);
    const f = fMin * Math.pow(fMax / fMin, t);
    out.push({ f, db: eqMagnitudeDb(bands, f, sampleRate, bypass) });
  }
  return out;
}

export function freqToX(f: number, width: number): number {
  const t = Math.log(f / 20) / Math.log(20000 / 20);
  return t * width;
}

export function xToFreq(x: number, width: number): number {
  const t = Math.max(0, Math.min(1, x / width));
  return 20 * Math.pow(20000 / 20, t);
}

export function dbToY(db: number, height: number, range = 18): number {
  const clamped = Math.max(-range, Math.min(range, db));
  return ((range - clamped) / (2 * range)) * height;
}

export function yToDb(y: number, height: number, range = 18): number {
  return range - (y / height) * (2 * range);
}

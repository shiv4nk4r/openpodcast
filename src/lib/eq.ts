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

/**
 * Studio-grade voice presets — gentle broadcast curves, not “consumer boost”
 * stacks. Tuned for spoken word over USB / XLR mics (close-mic podcast style).
 */
export const BUILTIN_EQ_PROFILES: EqProfile[] = [
  {
    id: "flat",
    name: "Flat",
    builtin: true,
    bypass: false,
    bands: makeFlatBands(),
  },
  {
    // Classic podcast / talk-radio chain: rumble out, mud down, presence + air.
    id: "broadcast",
    name: "Broadcast",
    builtin: true,
    bypass: false,
    bands: [
      band("bc-hp", "highpass", 80, 0, 0.707),
      band("bc-mud", "peaking", 220, -3.0, 0.9),
      band("bc-body", "peaking", 180, 1.0, 1.1),
      band("bc-presence", "peaking", 3200, 2.5, 1.0),
      band("bc-air", "highshelf", 10000, 1.5, 0.7),
    ],
  },
  {
    // Rounder chest tone; softens spit/harshness for bright condenser mics.
    id: "warm",
    name: "Warm",
    builtin: true,
    bypass: false,
    bands: [
      band("wm-hp", "highpass", 70, 0, 0.707),
      band("wm-body", "lowshelf", 160, 2.5, 0.7),
      band("wm-mud", "peaking", 280, -1.5, 1.0),
      band("wm-harsh", "peaking", 4500, -2.0, 1.2),
      band("wm-air", "highshelf", 9000, -1.5, 0.7),
    ],
  },
  {
    // Clarity for dull dynamics / distant mics — presence without sizzle.
    id: "bright",
    name: "Bright",
    builtin: true,
    bypass: false,
    bands: [
      band("br-hp", "highpass", 85, 0, 0.707),
      band("br-mud", "peaking", 250, -2.5, 0.9),
      band("br-presence", "peaking", 4000, 2.5, 1.0),
      band("br-air", "highshelf", 11000, 2.0, 0.7),
      band("br-sib", "peaking", 7000, -1.0, 2.0),
    ],
  },
  {
    // Desk / HVAC / handling noise — steeper low end, leave speech alone.
    id: "cut-rumble",
    name: "Cut rumble",
    builtin: true,
    bypass: false,
    bands: [
      band("cr-hp", "highpass", 110, 0, 0.707),
      band("cr-low", "lowshelf", 140, -5.0, 0.7),
      band("cr-mud", "peaking", 200, -2.0, 1.0),
      band("cr-presence", "peaking", 3000, 0.5, 1.0),
    ],
  },
  {
    // Deeper chest / baritone weight without turning into mud.
    id: "deep",
    name: "Deep",
    builtin: true,
    bypass: false,
    bands: [
      band("dp-hp", "highpass", 60, 0, 0.707),
      band("dp-chest", "lowshelf", 140, 4.0, 0.7),
      band("dp-body", "peaking", 200, 2.0, 1.0),
      band("dp-mud", "peaking", 320, -1.5, 1.0),
      band("dp-air", "highshelf", 9000, -1.5, 0.7),
    ],
  },
  {
    // Higher / lighter voices — cut boom, open the top for clarity.
    id: "high",
    name: "High",
    builtin: true,
    bypass: false,
    bands: [
      band("hi-hp", "highpass", 120, 0, 0.707),
      band("hi-low", "lowshelf", 160, -4.5, 0.7),
      band("hi-mud", "peaking", 250, -2.5, 0.9),
      band("hi-presence", "peaking", 3800, 2.0, 1.0),
      band("hi-air", "highshelf", 11000, 2.5, 0.7),
    ],
  },
];

export const DEFAULT_EQ_PROFILE_ID = "flat";

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
  if (bypass || !band.enabled) {
    filter.type = "peaking";
    filter.frequency.setTargetAtTime(1000, now, 0.015);
    filter.Q.setTargetAtTime(1, now, 0.015);
    filter.gain.setTargetAtTime(0, now, 0.015);
    return;
  }
  filter.type = toBiquadType(band.type);
  filter.frequency.setTargetAtTime(
    Math.max(20, Math.min(20000, band.frequency)),
    now,
    0.015,
  );
  filter.Q.setTargetAtTime(Math.max(0.1, Math.min(18, band.q)), now, 0.015);
  // highpass/lowpass/notch ignore gain in Web Audio; still set for consistency
  filter.gain.setTargetAtTime(band.gain, now, 0.015);
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

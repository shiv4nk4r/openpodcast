import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";

type PreviewEvent = {
  participant_id: string;
  jpeg_b64?: string | null;
  pcm_b64?: string | null;
};

const TARGET_W = 640;
const TARGET_H = 360;
const JPEG_QUALITY = 0.72;
const VIDEO_FPS = 15;
/** Wire format for mic PCM. */
const PCM_RATE = 16000;
const AUDIO_FLUSH_MS = 30;
/** ~120ms jitter target at 16kHz (before upsample). */
const JITTER_TARGET_16K = Math.round(PCM_RATE * 0.12);
const JITTER_MAX_16K = Math.round(PCM_RATE * 0.45);

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function bytesToB64(bytes: Uint8Array): string {
  let s = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    s += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(s);
}

function concatBytes(chunks: Uint8Array[]): Uint8Array {
  let n = 0;
  for (const c of chunks) n += c.length;
  const out = new Uint8Array(n);
  let o = 0;
  for (const c of chunks) {
    out.set(c, o);
    o += c.length;
  }
  return out;
}

function encodePcm16k(input: Float32Array, inputRate: number): Uint8Array {
  if (input.length === 0) return new Uint8Array();
  const outLen = Math.max(1, Math.round((input.length * PCM_RATE) / inputRate));
  const out = new Int16Array(outLen);
  const scale = (input.length - 1) / Math.max(1, outLen - 1);
  for (let i = 0; i < outLen; i++) {
    const src = i * scale;
    const i0 = Math.floor(src);
    const i1 = Math.min(input.length - 1, i0 + 1);
    const t = src - i0;
    const s = input[i0] * (1 - t) + input[i1] * t;
    const clipped = Math.max(-1, Math.min(1, s));
    out[i] = clipped < 0 ? clipped * 0x8000 : clipped * 0x7fff;
  }
  return new Uint8Array(out.buffer);
}

function resampleLinear(
  input: Float32Array,
  fromRate: number,
  toRate: number,
): Float32Array {
  if (fromRate === toRate || input.length === 0) return input;
  const outLen = Math.max(1, Math.round((input.length * toRate) / fromRate));
  const out = new Float32Array(outLen);
  const scale = (input.length - 1) / Math.max(1, outLen - 1);
  for (let i = 0; i < outLen; i++) {
    const src = i * scale;
    const i0 = Math.floor(src);
    const i1 = Math.min(input.length - 1, i0 + 1);
    const t = src - i0;
    out[i] = input[i0] * (1 - t) + input[i1] * t;
  }
  return out;
}

/** Ring stores samples at the AudioContext's native rate. */
class PcmRing {
  private buf: Float32Array;
  private r = 0;
  private w = 0;
  private n = 0;
  started = false;
  private target: number;

  constructor(capacity: number, target: number) {
    this.buf = new Float32Array(capacity);
    this.target = target;
  }

  get length() {
    return this.n;
  }

  push(samples: Float32Array) {
    for (let i = 0; i < samples.length; i++) {
      if (this.n >= this.buf.length) {
        this.r = (this.r + 1) % this.buf.length;
        this.n--;
      }
      this.buf[this.w] = samples[i];
      this.w = (this.w + 1) % this.buf.length;
      this.n++;
    }
    if (!this.started && this.n >= this.target) this.started = true;
  }

  pull(out: Float32Array) {
    if (!this.started) {
      out.fill(0);
      return;
    }
    for (let i = 0; i < out.length; i++) {
      if (this.n > 0) {
        out[i] = this.buf[this.r];
        this.r = (this.r + 1) % this.buf.length;
        this.n--;
      } else {
        out[i] = 0;
      }
    }
  }
}

/**
 * Live studio preview over a single iroh stream.
 * Audio is flushed first and played through a jitter ring (~120ms).
 */
export function useStudioPreview(opts: {
  active: boolean;
  role: "host" | "guest";
  hostHasAdmittedGuest: boolean;
  participantId: string;
  getVideoEl: () => HTMLVideoElement | null;
  getStream: () => MediaStream | null;
  camOn: boolean;
  micOn: boolean;
}) {
  const [admitted, setAdmitted] = useState(false);
  const ringRef = useRef<PcmRing | null>(null);
  const sendEnabled =
    opts.role === "host" ? opts.hostHasAdmittedGuest : admitted;

  useEffect(() => {
    if (!opts.active) {
      setAdmitted(false);
      ringRef.current = null;
      return;
    }

    // Use the device's native rate — forcing 16kHz often fails silently in WKWebView.
    let playCtx: AudioContext | null = null;
    let playProc: ScriptProcessorNode | null = null;
    let playGain: GainNode | null = null;
    let ring: PcmRing | null = null;

    try {
      playCtx = new AudioContext();
      void playCtx.resume();
      const rate = playCtx.sampleRate || 48000;
      const target = Math.round(JITTER_TARGET_16K * (rate / PCM_RATE));
      const capacity = Math.round(JITTER_MAX_16K * (rate / PCM_RATE));
      ring = new PcmRing(capacity, target);
      ringRef.current = ring;

      const bufSize = rate > 40000 ? 2048 : 1024;
      playProc = playCtx.createScriptProcessor(bufSize, 1, 1);
      playProc.onaudioprocess = (e) => {
        ring?.pull(e.outputBuffer.getChannelData(0));
      };
      playGain = playCtx.createGain();
      playGain.gain.value = 1;
      playProc.connect(playGain);
      playGain.connect(playCtx.destination);
    } catch {
      /* playback optional */
    }

    const ctxRate = playCtx?.sampleRate || 48000;

    let unlisten: UnlistenFn | undefined;
    void (async () => {
      try {
        unlisten = await listen<PreviewEvent>("studio-preview", (ev) => {
          const p = ev.payload;
          if (!p?.participant_id) return;
          if (p.participant_id.startsWith("__admit__:")) {
            setAdmitted(true);
            return;
          }
          if (p.participant_id === opts.participantId) return;
          if (!p.pcm_b64 || !ring) return;
          const bytes = b64ToBytes(p.pcm_b64);
          if (bytes.byteLength < 2) return;
          const samples = new Int16Array(
            bytes.buffer,
            bytes.byteOffset,
            Math.floor(bytes.byteLength / 2),
          );
          const f32 = new Float32Array(samples.length);
          for (let i = 0; i < samples.length; i++) f32[i] = samples[i] / 0x8000;
          ring.push(resampleLinear(f32, PCM_RATE, ctxRate));
        });
      } catch {
        /* browser preview without Tauri */
      }
    })();

    return () => {
      unlisten?.();
      try {
        playProc?.disconnect();
        playGain?.disconnect();
      } catch {
        /* ignore */
      }
      void playCtx?.close().catch(() => undefined);
      ringRef.current = null;
    };
  }, [opts.active, opts.participantId]);

  useEffect(() => {
    if (!opts.active || !sendEnabled) return;
    let cancelled = false;

    const canvas = document.createElement("canvas");
    canvas.width = TARGET_W;
    canvas.height = TARGET_H;
    const ctx2d = canvas.getContext("2d", { alpha: false });

    const pcmQueue: Uint8Array[] = [];
    let latestJpeg: string | null = null;
    let pumping = false;

    // Single pump, audio first — one QUIC stream, no dual-stream races.
    const pump = async () => {
      if (pumping || cancelled) return;
      pumping = true;
      try {
        while (!cancelled) {
          const pcmChunks = pcmQueue.splice(0, pcmQueue.length);
          const jpeg = latestJpeg;
          latestJpeg = null;
          if (!pcmChunks.length && !jpeg) break;

          if (pcmChunks.length) {
            await api.sendPreviewFrame(
              opts.participantId,
              undefined,
              bytesToB64(concatBytes(pcmChunks)),
            );
          }
          if (jpeg) {
            await api.sendPreviewFrame(opts.participantId, jpeg, undefined);
          }
        }
      } catch {
        /* drop on backpressure */
      } finally {
        pumping = false;
        if (!cancelled && (pcmQueue.length || latestJpeg)) void pump();
      }
    };

    let audioCtx: AudioContext | null = null;
    let processor: ScriptProcessorNode | null = null;
    let source: MediaStreamAudioSourceNode | null = null;

    const attachAudio = () => {
      const stream = opts.getStream();
      if (!stream?.getAudioTracks().length || !opts.micOn) return;
      try {
        audioCtx = new AudioContext();
        void audioCtx.resume();
        source = audioCtx.createMediaStreamSource(stream);
        processor = audioCtx.createScriptProcessor(2048, 1, 1);
        processor.onaudioprocess = (e) => {
          if (!opts.micOn || cancelled) return;
          const input = e.inputBuffer.getChannelData(0);
          const pcm = encodePcm16k(input, audioCtx?.sampleRate || 48000);
          if (!pcm.length) return;
          pcmQueue.push(pcm);
          while (pcmQueue.length > 16) pcmQueue.shift();
        };
        source.connect(processor);
        const mute = audioCtx.createGain();
        mute.gain.value = 0;
        processor.connect(mute);
        mute.connect(audioCtx.destination);
      } catch {
        /* capture optional */
      }
    };
    attachAudio();

    const audioFlushId = window.setInterval(() => {
      if (pcmQueue.length) void pump();
    }, AUDIO_FLUSH_MS);

    const videoId = window.setInterval(() => {
      if (cancelled || !opts.camOn || !ctx2d) return;
      const video = opts.getVideoEl();
      if (!video || video.readyState < 2 || video.videoWidth <= 0) return;
      const scale = Math.max(
        TARGET_W / video.videoWidth,
        TARGET_H / video.videoHeight,
      );
      const dw = video.videoWidth * scale;
      const dh = video.videoHeight * scale;
      ctx2d.fillStyle = "#111";
      ctx2d.fillRect(0, 0, TARGET_W, TARGET_H);
      ctx2d.drawImage(
        video,
        (TARGET_W - dw) / 2,
        (TARGET_H - dh) / 2,
        dw,
        dh,
      );
      latestJpeg = canvas
        .toDataURL("image/jpeg", JPEG_QUALITY)
        .replace(/^data:image\/jpeg;base64,/, "");
      void pump();
    }, 1000 / VIDEO_FPS);

    return () => {
      cancelled = true;
      clearInterval(audioFlushId);
      clearInterval(videoId);
      try {
        processor?.disconnect();
        source?.disconnect();
      } catch {
        /* ignore */
      }
      void audioCtx?.close().catch(() => undefined);
    };
  }, [
    opts.active,
    sendEnabled,
    opts.participantId,
    opts.camOn,
    opts.micOn,
    opts.getVideoEl,
    opts.getStream,
  ]);

  return { admitted };
}

function pcmB64Level(b64: string): number {
  const bytes = b64ToBytes(b64);
  if (bytes.byteLength < 2) return 0;
  const samples = new Int16Array(
    bytes.buffer,
    bytes.byteOffset,
    Math.floor(bytes.byteLength / 2),
  );
  let sum = 0;
  const n = samples.length;
  for (let i = 0; i < n; i++) {
    const v = samples[i] / 0x8000;
    sum += v * v;
  }
  const rms = Math.sqrt(sum / Math.max(1, n));
  // Match local meter feel: expand quiet speech, soft-cap peaks.
  return Math.min(1, Math.pow(rms * 2.8, 0.7));
}

export function useRemotePeerVideo(
  peerId: string,
  active: boolean,
): {
  imgRef: React.RefObject<HTMLImageElement | null>;
  hasFrame: boolean;
  /** 0–1 smoothed mic level from preview PCM. */
  level: number;
} {
  const imgRef = useRef<HTMLImageElement | null>(null);
  const [hasFrame, setHasFrame] = useState(false);
  const [level, setLevel] = useState(0);
  const staleTimer = useRef(0);
  const levelSmooth = useRef(0);
  const levelRaf = useRef(0);
  const lastPcmAt = useRef(0);

  useEffect(() => {
    if (!active || !peerId) {
      setHasFrame(false);
      setLevel(0);
      levelSmooth.current = 0;
      return;
    }
    let unlisten: UnlistenFn | undefined;
    let decayId = 0;

    const paintLevel = () => {
      levelRaf.current = 0;
      setLevel(levelSmooth.current);
    };

    const scheduleLevel = () => {
      if (!levelRaf.current) {
        levelRaf.current = window.requestAnimationFrame(paintLevel);
      }
    };

    decayId = window.setInterval(() => {
      if (Date.now() - lastPcmAt.current > 180) {
        levelSmooth.current *= 0.82;
        if (levelSmooth.current < 0.02) levelSmooth.current = 0;
        scheduleLevel();
      }
    }, 50);

    void (async () => {
      try {
        unlisten = await listen<PreviewEvent>("studio-preview", (ev) => {
          const p = ev.payload;
          if (!p || p.participant_id !== peerId) return;

          if (p.jpeg_b64) {
            const img = imgRef.current;
            if (img) img.src = `data:image/jpeg;base64,${p.jpeg_b64}`;
            setHasFrame(true);
            window.clearTimeout(staleTimer.current);
            staleTimer.current = window.setTimeout(() => setHasFrame(false), 4000);
          }

          if (p.pcm_b64) {
            lastPcmAt.current = Date.now();
            const instant = pcmB64Level(p.pcm_b64);
            // Attack fast, release slower — same idea as local meter.
            levelSmooth.current =
              instant > levelSmooth.current
                ? levelSmooth.current * 0.35 + instant * 0.65
                : levelSmooth.current * 0.78 + instant * 0.22;
            scheduleLevel();
          }
        });
      } catch {
        /* ignore */
      }
    })();
    return () => {
      unlisten?.();
      window.clearTimeout(staleTimer.current);
      window.clearInterval(decayId);
      if (levelRaf.current) window.cancelAnimationFrame(levelRaf.current);
    };
  }, [peerId, active]);

  return { imgRef, hasFrame, level };
}

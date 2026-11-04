import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { resampleCubic, softClip } from "../lib/audioDsp";

type PreviewEvent = {
  participant_id: string;
  jpeg_b64?: string | null;
  pcm_b64?: string | null;
};

const TARGET_W = 640;
const TARGET_H = 360;
const JPEG_QUALITY = 0.72;
const VIDEO_FPS = 15;
/** Sentinel jpeg payload — remotes clear the last frame and show cam-off UI. */
export const CAM_OFF_SENTINEL = "__cam_off__";
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
  const resampled = resampleCubic(input, inputRate, PCM_RATE);
  const out = new Int16Array(resampled.length);
  for (let i = 0; i < resampled.length; i++) {
    const soft = softClip(resampled[i]);
    out[i] = soft < 0 ? soft * 0x8000 : soft * 0x7fff;
  }
  return new Uint8Array(out.buffer);
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
 * Audio is flushed first and mixed per-peer through jitter rings (~120ms),
 * with local-only playback gains (does not affect anyone else's mix).
 */
export function useStudioPreview(opts: {
  active: boolean;
  role: "host" | "guest";
  hostHasAdmittedGuest: boolean;
  participantId: string;
  getVideoEl: () => HTMLVideoElement | null;
  getStream: () => MediaStream | null;
  /** When capture remounts (new MediaStream), re-tap audio for P2P. */
  streamEpoch?: number;
  camOn: boolean;
  micOn: boolean;
  /** Local listen levels for remote peers (0 = mute, 1 = default, up to 2). */
  peerVolumes?: Record<string, number>;
}) {
  const [admitted, setAdmitted] = useState(false);
  const volumesRef = useRef<Record<string, number>>(opts.peerVolumes ?? {});
  volumesRef.current = opts.peerVolumes ?? {};
  const sendEnabled =
    opts.role === "host" ? opts.hostHasAdmittedGuest : admitted;

  useEffect(() => {
    if (!opts.active) {
      setAdmitted(false);
      return;
    }

    // Use the device's native rate — forcing 16kHz often fails silently in WKWebView.
    let playCtx: AudioContext | null = null;
    let playProc: ScriptProcessorNode | null = null;
    let playGain: GainNode | null = null;
    const rings = new Map<string, PcmRing>();
    let mixScratch: Float32Array | null = null;
    let target = JITTER_TARGET_16K;
    let capacity = JITTER_MAX_16K;

    try {
      playCtx = new AudioContext();
      void playCtx.resume();
      const rate = playCtx.sampleRate || 48000;
      target = Math.round(JITTER_TARGET_16K * (rate / PCM_RATE));
      capacity = Math.round(JITTER_MAX_16K * (rate / PCM_RATE));

      const bufSize = rate > 40000 ? 2048 : 1024;
      playProc = playCtx.createScriptProcessor(bufSize, 1, 1);
      playProc.onaudioprocess = (e) => {
        const out = e.outputBuffer.getChannelData(0);
        out.fill(0);
        if (!mixScratch || mixScratch.length !== out.length) {
          mixScratch = new Float32Array(out.length);
        }
        const vols = volumesRef.current;
        for (const [peerId, ring] of rings) {
          ring.pull(mixScratch);
          const g = vols[peerId];
          const gain = g == null ? 1 : Math.max(0, Math.min(2, g));
          if (gain <= 0.0001) continue;
          for (let i = 0; i < out.length; i++) {
            out[i] += mixScratch[i] * gain;
          }
        }
        // Soft clip after summing peers — avoids hard-edge crackle.
        for (let i = 0; i < out.length; i++) {
          out[i] = softClip(out[i]);
        }
      };
      playGain = playCtx.createGain();
      playGain.gain.value = 1.05;
      // Mild high shelf cut on playback — less fatiguing over headphones.
      const playTone = playCtx.createBiquadFilter();
      playTone.type = "highshelf";
      playTone.frequency.value = 10000;
      playTone.gain.value = -0.8;
      playProc.connect(playGain);
      playGain.connect(playTone);
      playTone.connect(playCtx.destination);
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
          if (!p.pcm_b64 || !playCtx) return;
          const bytes = b64ToBytes(p.pcm_b64);
          if (bytes.byteLength < 2) return;
          const samples = new Int16Array(
            bytes.buffer,
            bytes.byteOffset,
            Math.floor(bytes.byteLength / 2),
          );
          const f32 = new Float32Array(samples.length);
          for (let i = 0; i < samples.length; i++) f32[i] = samples[i] / 0x8000;
          let ring = rings.get(p.participant_id);
          if (!ring) {
            ring = new PcmRing(capacity, target);
            rings.set(p.participant_id, ring);
          }
          ring.push(resampleCubic(f32, PCM_RATE, ctxRate));
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
      rings.clear();
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

    // Tell peers immediately when the camera turns off (don't leave a frozen frame).
    let camOffId = 0;
    if (!opts.camOn) {
      const announceOff = () => {
        if (cancelled) return;
        latestJpeg = CAM_OFF_SENTINEL;
        void pump();
      };
      announceOff();
      // Keep announcing so late joiners / dropped packets still clear video.
      camOffId = window.setInterval(announceOff, 2000);
    }

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
      if (camOffId) clearInterval(camOffId);
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
    opts.streamEpoch,
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
  /** Admitted peer with no live video (camera off or not yet streaming). */
  camOff: boolean;
  /** 0–1 smoothed mic level from preview PCM. */
  level: number;
} {
  const imgRef = useRef<HTMLImageElement | null>(null);
  const [hasFrame, setHasFrame] = useState(false);
  const [camOff, setCamOff] = useState(false);
  const [level, setLevel] = useState(0);
  const staleTimer = useRef(0);
  const levelSmooth = useRef(0);
  const levelRaf = useRef(0);
  const lastPcmAt = useRef(0);

  useEffect(() => {
    if (!active || !peerId) {
      setHasFrame(false);
      setCamOff(false);
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

    const clearVideo = (off: boolean) => {
      setHasFrame(false);
      setCamOff(off);
      const img = imgRef.current;
      if (img) img.removeAttribute("src");
      window.clearTimeout(staleTimer.current);
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

          if (p.jpeg_b64 != null) {
            if (p.jpeg_b64 === CAM_OFF_SENTINEL || p.jpeg_b64 === "") {
              clearVideo(true);
            } else {
              const img = imgRef.current;
              if (img) img.src = `data:image/jpeg;base64,${p.jpeg_b64}`;
              setHasFrame(true);
              setCamOff(false);
              window.clearTimeout(staleTimer.current);
              // No frames for a bit → treat as camera off (grey placeholder).
              staleTimer.current = window.setTimeout(
                () => clearVideo(true),
                2500,
              );
            }
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

  return { imgRef, hasFrame, camOff, level };
}

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";

type PreviewEvent = {
  participant_id: string;
  jpeg_b64?: string | null;
  pcm_b64?: string | null;
};

const TARGET_W = 320;
const TARGET_H = 180;
const JPEG_QUALITY = 0.42;
const VIDEO_FPS = 12;
const PCM_RATE = 16000;
/** Flush mic PCM often; audio rides its own QUIC stream. */
const AUDIO_FLUSH_MS = 30;
/** Target buffered audio before we start draining (~120ms). */
const JITTER_TARGET_SAMPLES = Math.round(PCM_RATE * 0.12);
/** Soft cap — drop oldest if a stall balloons the buffer (~400ms). */
const JITTER_MAX_SAMPLES = Math.round(PCM_RATE * 0.4);

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

/** Continuous float ring — ScriptProcessor pulls this; underruns are silence, not gaps. */
class PcmRing {
  private buf: Float32Array;
  private r = 0;
  private w = 0;
  private n = 0;
  started = false;

  constructor(capacity: number) {
    this.buf = new Float32Array(capacity);
  }

  get length() {
    return this.n;
  }

  push(samples: Float32Array) {
    for (let i = 0; i < samples.length; i++) {
      if (this.n >= this.buf.length) {
        // Drop oldest sample to keep latency bounded.
        this.r = (this.r + 1) % this.buf.length;
        this.n--;
      }
      this.buf[this.w] = samples[i];
      this.w = (this.w + 1) % this.buf.length;
      this.n++;
    }
    if (!this.started && this.n >= JITTER_TARGET_SAMPLES) {
      this.started = true;
    }
  }

  /** Fill `out` from the ring; pad with 0 on underrun (keeps the clock continuous). */
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
        // Soft underrun: silence sample, do NOT stop the player (avoids audible "breaks").
        out[i] = 0;
      }
    }
  }
}

/**
 * Live studio preview: audio on its own QUIC stream + ring-buffer playout
 * so brief network jitter becomes silence padding instead of broken speech.
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
  const playCtxRef = useRef<AudioContext | null>(null);
  const sendEnabled =
    opts.role === "host" ? opts.hostHasAdmittedGuest : admitted;

  // ---- receive: admit + push PCM into continuous ring ----
  useEffect(() => {
    if (!opts.active) {
      setAdmitted(false);
      ringRef.current = null;
      return;
    }

    const ring = new PcmRing(JITTER_MAX_SAMPLES);
    ringRef.current = ring;

    let playCtx: AudioContext | null = null;
    let playProc: ScriptProcessorNode | null = null;
    let playGain: GainNode | null = null;

    try {
      playCtx = new AudioContext({ sampleRate: PCM_RATE });
      playCtxRef.current = playCtx;
      void playCtx.resume();
      // 1024 @ 16kHz ≈ 64ms callback — smooth pull from the jitter ring.
      playProc = playCtx.createScriptProcessor(1024, 1, 1);
      playProc.onaudioprocess = (e) => {
        const out = e.outputBuffer.getChannelData(0);
        ring.pull(out);
      };
      playGain = playCtx.createGain();
      playGain.gain.value = 1;
      playProc.connect(playGain);
      playGain.connect(playCtx.destination);
    } catch {
      /* playback optional */
    }

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
          if (!p.pcm_b64) return;
          const bytes = b64ToBytes(p.pcm_b64);
          if (bytes.byteLength < 2) return;
          const samples = new Int16Array(
            bytes.buffer,
            bytes.byteOffset,
            Math.floor(bytes.byteLength / 2),
          );
          const f32 = new Float32Array(samples.length);
          for (let i = 0; i < samples.length; i++) f32[i] = samples[i] / 0x8000;
          ring.push(f32);
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
      if (playCtxRef.current === playCtx) playCtxRef.current = null;
      ringRef.current = null;
    };
  }, [opts.active, opts.participantId]);

  // ---- send: independent audio + video pumps (separate QUIC streams) ----
  useEffect(() => {
    if (!opts.active || !sendEnabled) return;
    let cancelled = false;

    const canvas = document.createElement("canvas");
    canvas.width = TARGET_W;
    canvas.height = TARGET_H;
    const ctx2d = canvas.getContext("2d", { alpha: false });

    const pcmQueue: Uint8Array[] = [];
    let latestJpeg: string | null = null;
    let audioPumping = false;
    let videoPumping = false;

    const pumpAudio = async () => {
      if (audioPumping || cancelled) return;
      audioPumping = true;
      try {
        while (!cancelled && pcmQueue.length) {
          const pcm = concatBytes(pcmQueue.splice(0, pcmQueue.length));
          await api.sendPreviewFrame(
            opts.participantId,
            undefined,
            bytesToB64(pcm),
          );
        }
      } catch {
        /* drop on backpressure */
      } finally {
        audioPumping = false;
        if (!cancelled && pcmQueue.length) void pumpAudio();
      }
    };

    const pumpVideo = async () => {
      if (videoPumping || cancelled) return;
      videoPumping = true;
      try {
        while (!cancelled && latestJpeg) {
          const jpeg = latestJpeg;
          latestJpeg = null;
          await api.sendPreviewFrame(opts.participantId, jpeg, undefined);
        }
      } catch {
        /* drop on backpressure */
      } finally {
        videoPumping = false;
        if (!cancelled && latestJpeg) void pumpVideo();
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
          // ~0.6s cap
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
      if (pcmQueue.length) void pumpAudio();
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
      const dx = (TARGET_W - dw) / 2;
      const dy = (TARGET_H - dh) / 2;
      ctx2d.fillStyle = "#111";
      ctx2d.fillRect(0, 0, TARGET_W, TARGET_H);
      ctx2d.drawImage(video, dx, dy, dw, dh);
      latestJpeg = canvas
        .toDataURL("image/jpeg", JPEG_QUALITY)
        .replace(/^data:image\/jpeg;base64,/, "");
      void pumpVideo();
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

/** Paints remote JPEG previews without React re-rendering the whole studio. */
export function useRemotePeerVideo(
  peerId: string,
  active: boolean,
): { imgRef: React.RefObject<HTMLImageElement | null>; hasFrame: boolean } {
  const imgRef = useRef<HTMLImageElement | null>(null);
  const [hasFrame, setHasFrame] = useState(false);
  const staleTimer = useRef(0);

  useEffect(() => {
    if (!active || !peerId) {
      setHasFrame(false);
      return;
    }
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      try {
        unlisten = await listen<PreviewEvent>("studio-preview", (ev) => {
          const p = ev.payload;
          if (!p?.jpeg_b64 || p.participant_id !== peerId) return;
          const img = imgRef.current;
          if (img) {
            img.src = `data:image/jpeg;base64,${p.jpeg_b64}`;
          }
          setHasFrame(true);
          window.clearTimeout(staleTimer.current);
          staleTimer.current = window.setTimeout(() => setHasFrame(false), 4000);
        });
      } catch {
        /* ignore */
      }
    })();
    return () => {
      unlisten?.();
      window.clearTimeout(staleTimer.current);
    };
  }, [peerId, active]);

  return { imgRef, hasFrame };
}

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";

type PreviewEvent = {
  participant_id: string;
  jpeg_b64?: string | null;
  pcm_b64?: string | null;
};

/** Small frames @ higher rate — big JPEGs were blocking audio on the same QUIC stream. */
const TARGET_W = 320;
const TARGET_H = 180;
const JPEG_QUALITY = 0.42;
const VIDEO_FPS = 12;
const PCM_RATE = 16000;
/** How often we flush queued mic PCM (ms). */
const AUDIO_FLUSH_MS = 40;
/** Start playback this far ahead to absorb jitter. */
const AUDIO_PREROLL_S = 0.08;
/** If playout falls this far behind wall clock, snap forward (drop backlog). */
const AUDIO_MAX_LAG_S = 0.35;

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

/** Downsample Float32 → Int16 LE mono @ 16kHz with linear interpolation. */
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

/**
 * Live studio preview over iroh PreviewFrame.
 * Audio and video are queued/flushed separately so JPEG writes don't starve mic PCM.
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
  const audioCtxRef = useRef<AudioContext | null>(null);
  const playTimeRef = useRef(0);
  const sendEnabled =
    opts.role === "host" ? opts.hostHasAdmittedGuest : admitted;

  // ---- receive: admit + audio (video paints via RemotePeerVideo) ----
  useEffect(() => {
    if (!opts.active) {
      setAdmitted(false);
      playTimeRef.current = 0;
      return;
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
          if (p.pcm_b64) playPcm(p.pcm_b64);
        });
      } catch {
        /* browser preview without Tauri */
      }
    })();
    return () => {
      unlisten?.();
    };
  }, [opts.active, opts.participantId]);

  function playPcm(b64: string) {
    try {
      const ctx =
        audioCtxRef.current ??
        new AudioContext({ sampleRate: PCM_RATE });
      audioCtxRef.current = ctx;
      if (ctx.state === "suspended") void ctx.resume();
      const bytes = b64ToBytes(b64);
      if (bytes.byteLength < 2) return;
      const samples = new Int16Array(
        bytes.buffer,
        bytes.byteOffset,
        Math.floor(bytes.byteLength / 2),
      );
      const f32 = new Float32Array(samples.length);
      for (let i = 0; i < samples.length; i++) f32[i] = samples[i] / 0x8000;
      const buf = ctx.createBuffer(1, f32.length, PCM_RATE);
      buf.copyToChannel(f32, 0);
      const src = ctx.createBufferSource();
      src.buffer = buf;
      src.connect(ctx.destination);

      const now = ctx.currentTime;
      // Underrun: schedule with preroll. Overrun: snap forward so delay doesn't grow.
      if (playTimeRef.current < now + 0.02) {
        playTimeRef.current = now + AUDIO_PREROLL_S;
      } else if (playTimeRef.current - now > AUDIO_MAX_LAG_S) {
        playTimeRef.current = now + AUDIO_PREROLL_S;
      }
      const startAt = playTimeRef.current;
      src.start(startAt);
      playTimeRef.current = startAt + buf.duration;
    } catch {
      /* ignore decode glitches */
    }
  }

  // ---- send: separate audio queue + video, single serialized pump ----
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

    const pump = async () => {
      if (pumping || cancelled) return;
      pumping = true;
      try {
        while (!cancelled) {
          const pcmChunks = pcmQueue.splice(0, pcmQueue.length);
          const jpeg = latestJpeg;
          latestJpeg = null;
          if (!pcmChunks.length && !jpeg) break;

          // Prefer shipping audio first so mic isn't stuck behind a JPEG.
          if (pcmChunks.length) {
            const pcm = concatBytes(pcmChunks);
            await api.sendPreviewFrame(
              opts.participantId,
              undefined,
              bytesToB64(pcm),
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
        // 2048 @ 48k ≈ 43ms — close to AUDIO_FLUSH_MS for steadier packets.
        processor = audioCtx.createScriptProcessor(2048, 1, 1);
        processor.onaudioprocess = (e) => {
          if (!opts.micOn || cancelled) return;
          const input = e.inputBuffer.getChannelData(0);
          const pcm = encodePcm16k(input, audioCtx?.sampleRate || 48000);
          if (!pcm.length) return;
          pcmQueue.push(pcm);
          // Cap queue (~0.5s) so a stalled send doesn't explode memory.
          while (pcmQueue.length > 12) pcmQueue.shift();
        };
        source.connect(processor);
        const mute = audioCtx.createGain();
        mute.gain.value = 0;
        processor.connect(mute);
        mute.connect(audioCtx.destination);
      } catch {
        /* audio capture optional */
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
      const dx = (TARGET_W - dw) / 2;
      const dy = (TARGET_H - dh) / 2;
      ctx2d.fillStyle = "#111";
      ctx2d.fillRect(0, 0, TARGET_W, TARGET_H);
      ctx2d.drawImage(video, dx, dy, dw, dh);
      const dataUrl = canvas.toDataURL("image/jpeg", JPEG_QUALITY);
      // Keep only the newest frame — old ones aren't worth sending late.
      latestJpeg = dataUrl.replace(/^data:image\/jpeg;base64,/, "");
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

  useEffect(() => {
    return () => {
      void audioCtxRef.current?.close().catch(() => undefined);
      audioCtxRef.current = null;
    };
  }, []);

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

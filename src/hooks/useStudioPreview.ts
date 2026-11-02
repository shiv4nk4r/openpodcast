import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";

export type RemotePreview = {
  jpeg?: string;
  updatedAt: number;
};

type PreviewEvent = {
  participant_id: string;
  jpeg_b64?: string | null;
  pcm_b64?: string | null;
};

const TARGET_W = 480;
const TARGET_H = 270;
const JPEG_QUALITY = 0.55;
const VIDEO_FPS = 5;
const PCM_RATE = 16000;

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

/** Downsample Float32 audio to Int16 LE mono @ 16kHz. */
function encodePcm16k(input: Float32Array, inputRate: number): Uint8Array {
  if (input.length === 0) return new Uint8Array();
  const ratio = inputRate / PCM_RATE;
  const outLen = Math.max(1, Math.floor(input.length / ratio));
  const out = new Int16Array(outLen);
  for (let i = 0; i < outLen; i++) {
    const src = Math.min(input.length - 1, Math.floor(i * ratio));
    const s = Math.max(-1, Math.min(1, input[src]));
    out[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
  }
  return new Uint8Array(out.buffer);
}

/**
 * Sends local camera/mic preview frames to the peer and plays theirs.
 * Uses the existing iroh PreviewFrame channel (JPEG + PCM16).
 */
export function useStudioPreview(opts: {
  /** Subscribe to remote frames / admit (guest should keep this true in-studio). */
  active: boolean;
  role: "host" | "guest";
  /** Host-only: true once at least one guest has been admitted. */
  hostHasAdmittedGuest: boolean;
  participantId: string;
  /** Local hidden video element driven by useLocalMedia */
  getVideoEl: () => HTMLVideoElement | null;
  getStream: () => MediaStream | null;
  camOn: boolean;
  micOn: boolean;
}) {
  const [remotes, setRemotes] = useState<Record<string, RemotePreview>>({});
  const [admitted, setAdmitted] = useState(false);
  const audioCtxRef = useRef<AudioContext | null>(null);
  const playTimeRef = useRef(0);
  const sendingRef = useRef(false);
  const sendEnabled =
    opts.role === "host" ? opts.hostHasAdmittedGuest : admitted;

  useEffect(() => {
    if (!opts.active) {
      setRemotes({});
      setAdmitted(false);
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
          if (p.jpeg_b64) {
            setRemotes((prev) => ({
              ...prev,
              [p.participant_id]: {
                jpeg: p.jpeg_b64!,
                updatedAt: Date.now(),
              },
            }));
          }
          if (p.pcm_b64) {
            void playPcm(p.pcm_b64);
          }
        });
      } catch {
        /* browser preview without Tauri */
      }
    })();
    return () => {
      unlisten?.();
    };
  }, [opts.active, opts.participantId]);

  async function playPcm(b64: string) {
    try {
      const ctx =
        audioCtxRef.current ??
        new AudioContext({ sampleRate: PCM_RATE });
      audioCtxRef.current = ctx;
      if (ctx.state === "suspended") await ctx.resume();
      const bytes = b64ToBytes(b64);
      if (bytes.byteLength < 2) return;
      const samples = new Int16Array(
        bytes.buffer,
        bytes.byteOffset,
        Math.floor(bytes.byteLength / 2),
      );
      const f32 = new Float32Array(samples.length);
      for (let i = 0; i < samples.length; i++) {
        f32[i] = samples[i] / 0x8000;
      }
      const buf = ctx.createBuffer(1, f32.length, PCM_RATE);
      buf.copyToChannel(f32, 0);
      const src = ctx.createBufferSource();
      src.buffer = buf;
      src.connect(ctx.destination);
      const now = ctx.currentTime;
      const startAt = Math.max(now + 0.02, playTimeRef.current);
      src.start(startAt);
      playTimeRef.current = startAt + buf.duration;
    } catch {
      /* ignore decode glitches */
    }
  }

  useEffect(() => {
    if (!opts.active || !sendEnabled) return;
    let cancelled = false;
    const canvas = document.createElement("canvas");
    canvas.width = TARGET_W;
    canvas.height = TARGET_H;
    const ctx2d = canvas.getContext("2d", { alpha: false });

    let audioCtx: AudioContext | null = null;
    let processor: ScriptProcessorNode | null = null;
    let source: MediaStreamAudioSourceNode | null = null;
    let pendingPcm: string | null = null;

    const attachAudio = () => {
      const stream = opts.getStream();
      if (!stream?.getAudioTracks().length || !opts.micOn) return;
      try {
        audioCtx = new AudioContext();
        source = audioCtx.createMediaStreamSource(stream);
        // ScriptProcessor is deprecated but widely available in WKWebView/WebView2.
        processor = audioCtx.createScriptProcessor(4096, 1, 1);
        processor.onaudioprocess = (e) => {
          if (!opts.micOn) return;
          const input = e.inputBuffer.getChannelData(0);
          const pcm = encodePcm16k(input, audioCtx?.sampleRate || 48000);
          if (pcm.length) pendingPcm = bytesToB64(pcm);
        };
        source.connect(processor);
        // Keep the node alive without feeding speakers (avoid local echo).
        const mute = audioCtx.createGain();
        mute.gain.value = 0;
        processor.connect(mute);
        mute.connect(audioCtx.destination);
      } catch {
        /* audio capture optional */
      }
    };
    attachAudio();

    const tick = async () => {
      if (cancelled || sendingRef.current) return;
      sendingRef.current = true;
      try {
        let jpeg_b64: string | undefined;
        const video = opts.getVideoEl();
        if (opts.camOn && video && video.readyState >= 2 && video.videoWidth > 0 && ctx2d) {
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
          jpeg_b64 = dataUrl.replace(/^data:image\/jpeg;base64,/, "");
        }
        const pcm_b64 = pendingPcm || undefined;
        pendingPcm = null;
        if (jpeg_b64 || pcm_b64) {
          await api.sendPreviewFrame(opts.participantId, jpeg_b64, pcm_b64);
        }
      } catch {
        /* drop frame on backpressure */
      } finally {
        sendingRef.current = false;
      }
    };

    const id = window.setInterval(() => void tick(), 1000 / VIDEO_FPS);
    return () => {
      cancelled = true;
      clearInterval(id);
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

  return { remotes, admitted };
}

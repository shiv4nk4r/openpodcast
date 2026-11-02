import { useCallback, useEffect, useRef, useState } from "react";
import { resolutionToSize } from "../lib/studioQuality";

export type RecordSink = {
  /** Called for each MediaRecorder timeslice — write to disk immediately. */
  onChunk: (chunk: Blob) => void | Promise<void>;
  timesliceMs?: number;
};

export function useLocalMedia(opts: {
  video: boolean;
  audio: boolean;
  echoCancellation?: boolean;
  /** When true, caller must invoke start() with preferred device IDs. */
  skipAutoStart?: boolean;
  resolution?: string;
  frameRate?: number;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const streamingRef = useRef(false);
  const writeQueueRef = useRef<Promise<void>>(Promise.resolve());
  const writeErrorRef = useRef<string | null>(null);
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [level, setLevel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const rafRef = useRef<number>(0);

  const meterCtxRef = useRef<AudioContext | null>(null);
  const smoothedRef = useRef(0);

  const stopMeter = () => {
    if (rafRef.current) cancelAnimationFrame(rafRef.current);
    rafRef.current = 0;
    void meterCtxRef.current?.close().catch(() => undefined);
    meterCtxRef.current = null;
    smoothedRef.current = 0;
    setLevel(0);
  };

  const startMeter = (stream: MediaStream) => {
    stopMeter();
    const audioTracks = stream.getAudioTracks();
    if (!audioTracks.length) return;

    const ctx = new AudioContext();
    meterCtxRef.current = ctx;
    void ctx.resume();

    const source = ctx.createMediaStreamSource(stream);
    const analyser = ctx.createAnalyser();
    // Larger window + slight smoothing gives a stabler, fuller meter
    analyser.fftSize = 2048;
    analyser.smoothingTimeConstant = 0.8;
    source.connect(analyser);

    const data = new Float32Array(analyser.fftSize);
    let lastUi = 0;
    const tick = () => {
      if (ctx.state === "suspended") void ctx.resume();
      analyser.getFloatTimeDomainData(data);

      let sumSquares = 0;
      let peak = 0;
      for (let i = 0; i < data.length; i++) {
        const s = data[i];
        sumSquares += s * s;
        peak = Math.max(peak, Math.abs(s));
      }
      const rms = Math.sqrt(sumSquares / data.length);
      // Blend RMS + peak, then boost so normal speech fills most of the bar
      const raw = Math.min(1, rms * 3.2 + peak * 0.55);
      // Soft knee so quiet speech still moves, loud speech can hit ~100%
      const boosted = Math.min(1, Math.pow(raw, 0.65) * 1.35);
      // Attack fast / release slower for a natural VU feel
      const prev = smoothedRef.current;
      const next =
        boosted > prev ? prev * 0.35 + boosted * 0.65 : prev * 0.85 + boosted * 0.15;
      smoothedRef.current = next;
      // Cap React meter updates ~20fps to reduce render churn while recording
      const now = performance.now();
      if (now - lastUi > 50) {
        lastUi = now;
        setLevel(next);
      }
      rafRef.current = requestAnimationFrame(tick);
    };
    tick();
  };

  const start = useCallback(
    async (deviceIds?: { audio?: string; video?: string }) => {
      setError(null);
      try {
        streamRef.current?.getTracks().forEach((t) => t.stop());
        const size = resolutionToSize(opts.resolution ?? "1080p");
        const fps = opts.frameRate && opts.frameRate > 0 ? opts.frameRate : 30;
        const stream = await navigator.mediaDevices.getUserMedia({
          audio: opts.audio
            ? {
                deviceId: deviceIds?.audio
                  ? { exact: deviceIds.audio }
                  : undefined,
                echoCancellation: opts.echoCancellation ?? true,
                // NS ducks speech; AGC lifts quiet mics into a usable range
                noiseSuppression: false,
                autoGainControl: true,
              }
            : false,
          video: opts.video
            ? {
                deviceId: deviceIds?.video
                  ? { exact: deviceIds.video }
                  : undefined,
                width: { ideal: size.width },
                height: { ideal: size.height },
                frameRate: { ideal: fps },
              }
            : false,
        });
        streamRef.current = stream;
        if (videoRef.current) {
          videoRef.current.controls = false;
          videoRef.current.disablePictureInPicture = true;
          videoRef.current.srcObject = stream;
          await videoRef.current.play().catch(() => undefined);
        }
        if (opts.audio) startMeter(stream);
        const list = await navigator.mediaDevices.enumerateDevices();
        setDevices(list);
        setReady(true);
      } catch (e) {
        setError(String(e));
        setReady(false);
      }
    },
    [opts.audio, opts.video, opts.echoCancellation, opts.resolution, opts.frameRate],
  );

  useEffect(() => {
    if (!opts.skipAutoStart) {
      void start();
    }
    return () => {
      stopMeter();
      streamRef.current?.getTracks().forEach((t) => t.stop());
    };
  }, [start, opts.skipAutoStart]);

  const setVideoEl = useCallback((el: HTMLVideoElement | null) => {
    const prev = videoRef.current;
    if (prev && prev !== el) {
      prev.onpause = null;
    }
    videoRef.current = el;
    if (el) {
      el.controls = false;
      el.muted = true;
      el.playsInline = true;
      el.disablePictureInPicture = true;
      el.setAttribute("controlsList", "nodownload nofullscreen noremoteplayback");
      el.setAttribute("disablePictureInPicture", "");
      el.setAttribute("playsinline", "true");
      el.setAttribute("webkit-playsinline", "true");
      // Defeat WKWebView media overlay pausing the stream
      el.onpause = () => {
        if (el.paused && el.srcObject) void el.play().catch(() => undefined);
      };
      if (streamRef.current) {
        el.srcObject = streamRef.current;
        el.play().catch(() => undefined);
      }
    }
  }, []);

  const toggleTrack = (kind: "audio" | "video", enabled: boolean) => {
    streamRef.current?.getTracks().forEach((t) => {
      if (t.kind === kind) t.enabled = enabled;
    });
  };

  const enqueueWrite = (task: () => Promise<void>) => {
    writeQueueRef.current = writeQueueRef.current
      .then(task)
      .catch((e) => {
        writeErrorRef.current = String(e);
        console.error("Video chunk write failed", e);
      });
    return writeQueueRef.current;
  };

  const startRecording = (sink?: RecordSink) => {
    const stream = streamRef.current;
    if (!stream) throw new Error("No media stream");
    chunksRef.current = [];
    writeErrorRef.current = null;
    writeQueueRef.current = Promise.resolve();
    streamingRef.current = !!sink?.onChunk;
    // Prefer MP4 for WKWebView/macOS playback in the editor; WebM as fallback.
    const mime = MediaRecorder.isTypeSupported("video/mp4;codecs=avc1,mp4a.40.2")
      ? "video/mp4;codecs=avc1,mp4a.40.2"
      : MediaRecorder.isTypeSupported("video/mp4")
        ? "video/mp4"
        : MediaRecorder.isTypeSupported("video/webm;codecs=vp9,opus")
          ? "video/webm;codecs=vp9,opus"
          : MediaRecorder.isTypeSupported("video/webm")
            ? "video/webm"
            : "";
    const rec = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
    rec.ondataavailable = (e) => {
      if (!e.data.size) return;
      if (streamingRef.current && sink?.onChunk) {
        const chunk = e.data;
        enqueueWrite(async () => {
          await sink.onChunk(chunk);
        });
      } else {
        chunksRef.current.push(e.data);
      }
    };
    // 2s slices: fewer IPC writes, still bounded RAM if a write stalls briefly
    rec.start(sink?.timesliceMs ?? 2000);
    recorderRef.current = rec;
  };

  const stopRecording = async (): Promise<Blob | null> => {
    const rec = recorderRef.current;
    if (!rec) {
      await writeQueueRef.current;
      return null;
    }
    const streamed = streamingRef.current;
    await new Promise<void>((resolve) => {
      rec.onstop = () => resolve();
      try {
        if (rec.state === "recording") rec.requestData();
      } catch {
        /* some engines throw if idle */
      }
      rec.stop();
      recorderRef.current = null;
    });
    // Drain any in-flight chunk writes before finalize
    await writeQueueRef.current;
    streamingRef.current = false;
    if (writeErrorRef.current) {
      throw new Error(writeErrorRef.current);
    }
    if (streamed) return null;
    const type = rec.mimeType || "video/webm";
    return new Blob(chunksRef.current, { type });
  };

  /** Grab a still JPEG from the live camera for session thumbnails. */
  const grabJpegFrame = async (quality = 0.82): Promise<number[] | null> => {
    const v = videoRef.current;
    if (!v || v.readyState < 2 || v.videoWidth < 2) return null;
    const canvas = document.createElement("canvas");
    const maxW = 640;
    const scale = Math.min(1, maxW / v.videoWidth);
    canvas.width = Math.max(1, Math.round(v.videoWidth * scale));
    canvas.height = Math.max(1, Math.round(v.videoHeight * scale));
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.drawImage(v, 0, 0, canvas.width, canvas.height);
    const blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob((b) => resolve(b), "image/jpeg", quality),
    );
    if (!blob) return null;
    return blobToBytes(blob);
  };

  return {
    setVideoEl,
    devices,
    level,
    error,
    ready,
    start,
    toggleTrack,
    startRecording,
    stopRecording,
    grabJpegFrame,
    stream: streamRef,
    /** Active recorder mime — useful to pick file extension before start. */
    pickRecorderExtension: () => {
      if (MediaRecorder.isTypeSupported("video/mp4;codecs=avc1,mp4a.40.2"))
        return "mp4";
      if (MediaRecorder.isTypeSupported("video/mp4")) return "mp4";
      return "webm";
    },
  };
}

export async function blobToBytes(blob: Blob): Promise<number[]> {
  const buf = await blob.arrayBuffer();
  return Array.from(new Uint8Array(buf));
}

export async function blobToUint8(blob: Blob): Promise<Uint8Array> {
  return new Uint8Array(await blob.arrayBuffer());
}

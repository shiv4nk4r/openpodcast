import { useCallback, useEffect, useRef, useState } from "react";
import {
  applyBandToFilter,
  cloneProfile,
  DEFAULT_EQ_PROFILE_ID,
  makeupGainForProfile,
  MAX_EQ_SLOTS,
  padBandsToSlots,
  resolveEqProfile,
  type EqProfile,
} from "../lib/eq";
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
  /** Initial EQ profile (resolved from settings). */
  eqProfile?: EqProfile;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  /** Combined processed audio + camera video — used for record / P2P. */
  const streamRef = useRef<MediaStream | null>(null);
  /** Raw getUserMedia — stop tracks here on teardown / device change. */
  const rawStreamRef = useRef<MediaStream | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const streamingRef = useRef(false);
  const writeQueueRef = useRef<Promise<void>>(Promise.resolve());
  const writeErrorRef = useRef<string | null>(null);
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [level, setLevel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  /** True while mic is routed to local speakers for a pre-join listen test. */
  const [monitoring, setMonitoringState] = useState(false);
  const rafRef = useRef<number>(0);

  const audioCtxRef = useRef<AudioContext | null>(null);
  const monitorGainRef = useRef<GainNode | null>(null);
  const monitoringDesiredRef = useRef(false);
  const smoothedRef = useRef(0);
  const eqFiltersRef = useRef<BiquadFilterNode[]>([]);
  const makeupGainRef = useRef<GainNode | null>(null);
  const eqProfileRef = useRef<EqProfile>(
    opts.eqProfile
      ? cloneProfile(opts.eqProfile)
      : resolveEqProfile(DEFAULT_EQ_PROFILE_ID, []),
  );
  const processedDestRef = useRef<MediaStreamAudioDestinationNode | null>(null);
  /** Bumps when the outbound MediaStream is replaced (device restart only). */
  const [streamEpoch, setStreamEpoch] = useState(0);

  const stopAudioGraph = () => {
    if (rafRef.current) cancelAnimationFrame(rafRef.current);
    rafRef.current = 0;
    monitorGainRef.current = null;
    makeupGainRef.current = null;
    eqFiltersRef.current = [];
    processedDestRef.current = null;
    void audioCtxRef.current?.close().catch(() => undefined);
    audioCtxRef.current = null;
    smoothedRef.current = 0;
    setLevel(0);
    setMonitoringState(false);
  };

  const applyProfileToGraph = (profile: EqProfile) => {
    const filters = eqFiltersRef.current;
    if (!filters.length) return;
    const slots = padBandsToSlots(profile.bands);
    for (let i = 0; i < filters.length; i++) {
      applyBandToFilter(filters[i], slots[i], profile.bypass);
    }
    const makeup = makeupGainRef.current;
    if (makeup) {
      const now = makeup.context.currentTime;
      makeup.gain.cancelScheduledValues(now);
      makeup.gain.setTargetAtTime(makeupGainForProfile(profile), now, 0.03);
    }
  };

  const startAudioGraph = (raw: MediaStream) => {
    stopAudioGraph();
    const audioTracks = raw.getAudioTracks();
    if (!audioTracks.length) return null;

    const ctx = new AudioContext({ latencyHint: "interactive" });
    audioCtxRef.current = ctx;
    void ctx.resume();

    const source = ctx.createMediaStreamSource(raw);
    const dest = ctx.createMediaStreamDestination();
    processedDestRef.current = dest;

    const analyser = ctx.createAnalyser();
    analyser.fftSize = 2048;
    analyser.smoothingTimeConstant = 0.8;

    // Always-on polish (before profile EQ): kill rumble + tame harsh esses.
    const rumbleHpf = ctx.createBiquadFilter();
    rumbleHpf.type = "highpass";
    rumbleHpf.frequency.value = 70;
    rumbleHpf.Q.value = 0.707;

    const deEss = ctx.createBiquadFilter();
    deEss.type = "peaking";
    deEss.frequency.value = 6500;
    deEss.Q.value = 2.2;
    deEss.gain.value = -1.5;

    // Gentle speech leveler — more consistent volume without pumping.
    const voiceComp = ctx.createDynamicsCompressor();
    voiceComp.threshold.value = -26;
    voiceComp.knee.value = 14;
    voiceComp.ratio.value = 2.4;
    voiceComp.attack.value = 0.015;
    voiceComp.release.value = 0.22;

    const makeup = ctx.createGain();
    makeupGainRef.current = makeup;

    // Peak safety only — transparent until near full scale.
    const limiter = ctx.createDynamicsCompressor();
    limiter.threshold.value = -2.5;
    limiter.knee.value = 4;
    limiter.ratio.value = 18;
    limiter.attack.value = 0.001;
    limiter.release.value = 0.08;

    // Tiny presence lift after dynamics — “radio polish” without harshness.
    const airShelf = ctx.createBiquadFilter();
    airShelf.type = "highshelf";
    airShelf.frequency.value = 8500;
    airShelf.gain.value = 0.8;

    const monitorGain = ctx.createGain();
    monitorGain.gain.value = monitoringDesiredRef.current ? 1 : 0;
    monitorGainRef.current = monitorGain;

    // Fixed slot count: profile switches only retune filters — never remount tracks.
    const filters: BiquadFilterNode[] = [];
    for (let i = 0; i < MAX_EQ_SLOTS; i++) {
      filters.push(ctx.createBiquadFilter());
    }
    eqFiltersRef.current = filters;

    // mic → rumble HPF → profile EQ → de-ess → voice compressor → makeup → peak lim → air → out
    source.connect(rumbleHpf);
    let prev: AudioNode = rumbleHpf;
    for (const f of filters) {
      prev.connect(f);
      prev = f;
    }
    prev.connect(deEss);
    deEss.connect(voiceComp);
    voiceComp.connect(makeup);
    makeup.connect(limiter);
    limiter.connect(airShelf);
    airShelf.connect(dest);
    airShelf.connect(analyser);
    airShelf.connect(monitorGain);
    monitorGain.connect(ctx.destination);

    applyProfileToGraph(eqProfileRef.current);
    setMonitoringState(monitoringDesiredRef.current);

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
      const rawLevel = Math.min(1, rms * 3.2 + peak * 0.55);
      const boosted = Math.min(1, Math.pow(rawLevel, 0.65) * 1.35);
      const prevSm = smoothedRef.current;
      const next =
        boosted > prevSm
          ? prevSm * 0.35 + boosted * 0.65
          : prevSm * 0.85 + boosted * 0.15;
      smoothedRef.current = next;
      const now = performance.now();
      if (now - lastUi > 50) {
        lastUi = now;
        setLevel(next);
      }
      rafRef.current = requestAnimationFrame(tick);
    };
    tick();

    return dest.stream;
  };

  /** Live-update EQ in place — never replaces the outbound MediaStream. */
  const applyEqProfile = useCallback((profile: EqProfile) => {
    const next = cloneProfile(profile);
    eqProfileRef.current = next;
    applyProfileToGraph(next);
  }, []);

  const start = useCallback(
    async (deviceIds?: { audio?: string; video?: string }) => {
      setError(null);
      try {
        rawStreamRef.current?.getTracks().forEach((t) => t.stop());
        streamRef.current = null;
        rawStreamRef.current = null;
        stopAudioGraph();

        const size = resolutionToSize(opts.resolution ?? "1080p");
        const fps = opts.frameRate && opts.frameRate > 0 ? opts.frameRate : 30;
        const audioConstraints = (
          preferDevice: boolean,
        ): MediaTrackConstraints | false =>
          opts.audio
            ? {
                ...(preferDevice && deviceIds?.audio
                  ? { deviceId: { ideal: deviceIds.audio } }
                  : {}),
                echoCancellation: opts.echoCancellation ?? false,
                // Browser NS ducks speech; we polish in the Web Audio graph instead.
                noiseSuppression: false,
                autoGainControl: true,
                channelCount: { ideal: 1 },
                sampleRate: { ideal: 48000 },
              }
            : false;
        const videoConstraints = (
          preferDevice: boolean,
        ): MediaTrackConstraints | false =>
          opts.video
            ? {
                ...(preferDevice && deviceIds?.video
                  ? { deviceId: { ideal: deviceIds.video } }
                  : {}),
                width: { ideal: size.width },
                height: { ideal: size.height },
                frameRate: { ideal: fps },
              }
            : false;

        let raw: MediaStream;
        try {
          raw = await navigator.mediaDevices.getUserMedia({
            audio: audioConstraints(true),
            video: videoConstraints(true),
          });
        } catch (first) {
          try {
            raw = await navigator.mediaDevices.getUserMedia({
              audio: audioConstraints(false),
              video: videoConstraints(false),
            });
          } catch (second) {
            if (opts.audio && opts.video) {
              raw = await navigator.mediaDevices.getUserMedia({
                audio: audioConstraints(false),
                video: false,
              });
            } else {
              throw second ?? first;
            }
          }
        }
        rawStreamRef.current = raw;

        let combined: MediaStream = raw;
        if (opts.audio) {
          const processedAudio = startAudioGraph(raw);
          if (processedAudio) {
            combined = new MediaStream([
              ...processedAudio.getAudioTracks(),
              ...raw.getVideoTracks(),
            ]);
          }
        }
        streamRef.current = combined;
        setStreamEpoch((n) => n + 1);

        if (videoRef.current) {
          videoRef.current.controls = false;
          videoRef.current.disablePictureInPicture = true;
          videoRef.current.srcObject = combined;
          await videoRef.current.play().catch(() => undefined);
        }
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
      stopAudioGraph();
      rawStreamRef.current?.getTracks().forEach((t) => t.stop());
      rawStreamRef.current = null;
      streamRef.current = null;
    };
  }, [start, opts.skipAutoStart]);

  // Seed EQ from settings once when the active profile id changes (not on every object identity).
  const eqSeedId = opts.eqProfile?.id;
  const eqSeedBypass = opts.eqProfile?.bypass;
  useEffect(() => {
    if (opts.eqProfile) {
      applyEqProfile(opts.eqProfile);
    }
    // intentionally seed from id/bypass only — panel pushes live edits via applyEqProfile
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [eqSeedId, eqSeedBypass]);

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
    // Mute at the raw mic so EQ / dest go silent; video on the shared track refs.
    const target = kind === "audio" ? rawStreamRef.current : streamRef.current;
    target?.getTracks().forEach((t) => {
      if (t.kind === kind) t.enabled = enabled;
    });
    if (kind === "video") {
      rawStreamRef.current?.getVideoTracks().forEach((t) => {
        t.enabled = enabled;
      });
    }
  };

  /** Play the live (EQ'd) mic into local speakers for a pre-join listen test. */
  const setMonitoring = useCallback(async (on: boolean) => {
    monitoringDesiredRef.current = on;
    const ctx = audioCtxRef.current;
    const gain = monitorGainRef.current;
    if (!ctx || !gain) {
      setMonitoringState(false);
      return;
    }
    if (ctx.state === "suspended") {
      await ctx.resume().catch(() => undefined);
    }
    const now = ctx.currentTime;
    gain.gain.cancelScheduledValues(now);
    gain.gain.setTargetAtTime(on ? 1 : 0, now, 0.02);
    setMonitoringState(on);
  }, []);

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
    rec.start(sink?.timesliceMs ?? 2000);
    recorderRef.current = rec;
  };

  const stopRecording = async (): Promise<Blob | null> => {
    const rec = recorderRef.current;
    if (!rec) {
      await Promise.race([
        writeQueueRef.current,
        new Promise<void>((r) => setTimeout(r, 15_000)),
      ]);
      return null;
    }
    const streamed = streamingRef.current;
    const mimeType = rec.mimeType || "video/webm";

    if (rec.state !== "inactive") {
      await new Promise<void>((resolve) => {
        let settled = false;
        const finish = () => {
          if (settled) return;
          settled = true;
          resolve();
        };
        rec.addEventListener("stop", finish, { once: true });
        try {
          if (rec.state === "recording") rec.requestData();
        } catch {
          /* some engines throw if idle */
        }
        try {
          rec.stop();
        } catch {
          finish();
        }
        window.setTimeout(finish, 8_000);
      });
    }
    recorderRef.current = null;

    await Promise.race([
      writeQueueRef.current,
      new Promise<void>((resolve) => {
        window.setTimeout(() => {
          console.warn("Video chunk write queue timed out during stop");
          resolve();
        }, 45_000);
      }),
    ]);
    streamingRef.current = false;
    if (writeErrorRef.current) {
      throw new Error(writeErrorRef.current);
    }
    if (streamed) return null;
    return new Blob(chunksRef.current, { type: mimeType });
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
    monitoring,
    setMonitoring,
    applyEqProfile,
    start,
    toggleTrack,
    startRecording,
    stopRecording,
    grabJpegFrame,
    stream: streamRef,
    /** Increments when capture restarts — P2P tap should re-attach. */
    streamEpoch,
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

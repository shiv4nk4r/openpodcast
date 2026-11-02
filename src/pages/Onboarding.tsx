import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  IconArrowLeft,
  IconArrowRight,
  IconCamera,
  IconCheck,
  IconCircleFilled,
  IconMicrophone,
} from "@tabler/icons-react";
import { api } from "../api";
import { Select } from "../components/Select";
import { pickPreferred, useDeviceList } from "../hooks/useDeviceList";
import { useApp } from "../state";
import { useTheme, type ThemeId } from "../theme";

const ONBOARDING_KEY = "openpodcast.onboarding_complete";

export function isOnboardingComplete(): boolean {
  return localStorage.getItem(ONBOARDING_KEY) === "1";
}

export function markOnboardingComplete() {
  localStorage.setItem(ONBOARDING_KEY, "1");
}

export function resetOnboarding() {
  localStorage.removeItem(ONBOARDING_KEY);
}

type Step = "welcome" | "name" | "theme" | "devices" | "ready";

const STEPS: Step[] = ["welcome", "name", "theme", "devices", "ready"];

export default function Onboarding() {
  const navigate = useNavigate();
  const { settings, refreshSettings } = useApp();
  const { theme, setTheme, themes } = useTheme();
  const devices = useDeviceList();

  const [step, setStep] = useState<Step>("welcome");
  const [name, setName] = useState(
    settings.display_name && settings.display_name !== "Host"
      ? settings.display_name
      : "",
  );
  const [cameraId, setCameraId] = useState(settings.preferred_camera_id ?? "");
  const [micId, setMicId] = useState(settings.preferred_mic_id ?? "");
  const [saving, setSaving] = useState(false);
  const [permError, setPermError] = useState<string | null>(null);
  const [micLevel, setMicLevel] = useState(0);
  const [animKey, setAnimKey] = useState(0);

  const previewRef = useRef<HTMLVideoElement | null>(null);
  const streamRef = useRef<MediaStream | null>(null);

  const stepIndex = STEPS.indexOf(step);
  const progress = ((stepIndex + 1) / STEPS.length) * 100;

  useEffect(() => {
    const nextCam = pickPreferred(devices.cameras, settings.preferred_camera_id);
    const nextMic = pickPreferred(devices.mics, settings.preferred_mic_id);
    if (nextCam && !cameraId) setCameraId(nextCam);
    if (nextMic && !micId) setMicId(nextMic);
  }, [devices.cameras, devices.mics, settings, cameraId, micId]);

  useEffect(() => {
    if (isOnboardingComplete()) {
      navigate("/", { replace: true });
    }
  }, [navigate]);

  useEffect(() => {
    setAnimKey((k) => k + 1);
  }, [step]);

  // Live preview on devices step
  useEffect(() => {
    if (step !== "devices") {
      streamRef.current?.getTracks().forEach((t) => t.stop());
      streamRef.current = null;
      setMicLevel(0);
      return;
    }

    let cancelled = false;
    let raf = 0;
    let audioCtx: AudioContext | null = null;
    let smoothed = 0;

    async function run() {
      setPermError(null);
      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          video: cameraId ? { deviceId: { exact: cameraId } } : true,
          audio: micId
            ? { deviceId: { exact: micId }, echoCancellation: true }
            : true,
        });
        if (cancelled) {
          stream.getTracks().forEach((t) => t.stop());
          return;
        }
        streamRef.current?.getTracks().forEach((t) => t.stop());
        streamRef.current = stream;
        if (previewRef.current) {
          previewRef.current.srcObject = stream;
          await previewRef.current.play().catch(() => undefined);
        }
        await devices.refresh();

        if (stream.getAudioTracks()[0]) {
          audioCtx = new AudioContext();
          void audioCtx.resume();
          const source = audioCtx.createMediaStreamSource(stream);
          const analyser = audioCtx.createAnalyser();
          analyser.fftSize = 2048;
          source.connect(analyser);
          const data = new Float32Array(analyser.fftSize);
          const tick = () => {
            if (cancelled) return;
            analyser.getFloatTimeDomainData(data);
            let sum = 0;
            let peak = 0;
            for (let i = 0; i < data.length; i++) {
              sum += data[i] * data[i];
              peak = Math.max(peak, Math.abs(data[i]));
            }
            const rms = Math.sqrt(sum / data.length);
            const raw = Math.min(1, rms * 3.2 + peak * 0.55);
            const boosted = Math.min(1, Math.pow(raw, 0.65) * 1.35);
            smoothed =
              boosted > smoothed
                ? smoothed * 0.35 + boosted * 0.65
                : smoothed * 0.85 + boosted * 0.15;
            setMicLevel(smoothed);
            raf = requestAnimationFrame(tick);
          };
          tick();
        }
      } catch (e) {
        setPermError(String(e));
      }
    }

    void run();
    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
      void audioCtx?.close();
      streamRef.current?.getTracks().forEach((t) => t.stop());
      streamRef.current = null;
    };
  }, [step, cameraId, micId]); // eslint-disable-line react-hooks/exhaustive-deps

  const headline = useMemo(() => {
    switch (step) {
      case "welcome":
        return "Your booth.\nNo cloud tax.";
      case "name":
        return "What should\nguests call you?";
      case "theme":
        return "Dark or\nlight?";
      case "devices":
        return "Check the\nmic and frame.";
      case "ready":
        return "You’re lit.\nHit record.";
    }
  }, [step]);

  const sub = useMemo(() => {
    switch (step) {
      case "welcome":
        return "Local high-quality podcast recording — host and guests, separate tracks, Drive when you want it.";
      case "name":
        return "This shows in the lobby, on tracks, and in captions.";
      case "theme":
        return "Dark is Signal Dark by default. You can switch anytime in Settings.";
      case "devices":
        return "We’ll remember these as your defaults. Speak to test the meter.";
      case "ready":
        return "Create a studio or join with an invite. Everything stays on your machine until you upload.";
    }
  }, [step]);

  function next() {
    const i = STEPS.indexOf(step);
    if (i < STEPS.length - 1) setStep(STEPS[i + 1]);
  }

  function back() {
    const i = STEPS.indexOf(step);
    if (i > 0) setStep(STEPS[i - 1]);
  }

  async function finish() {
    setSaving(true);
    try {
      await api.saveSettings({
        ...settings,
        display_name: name.trim() || "Host",
        preferred_camera_id: cameraId || null,
        preferred_mic_id: micId || null,
      });
      await refreshSettings();
      markOnboardingComplete();
      // Land on Riverside-style lobby (cam/mic check)
      navigate("/", { replace: true });
    } catch (e) {
      setPermError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function skip() {
    markOnboardingComplete();
    navigate("/", { replace: true });
  }

  const canContinue =
    step === "welcome" ||
    step === "theme" ||
    step === "devices" ||
    step === "ready" ||
    (step === "name" && name.trim().length > 0);

  return (
    <div className="onboard">
      <div className="onboard-glow onboard-glow-a" />
      <div className="onboard-glow onboard-glow-b" />
      <div className="onboard-grid" />

      <header className="onboard-top">
        <div className="onboard-brand">
          <span className="onboard-dot" />
          OpenPodcast
        </div>
        <button type="button" className="btn btn-ghost onboard-skip" onClick={() => void skip()}>
          Skip
        </button>
      </header>

      <div className="onboard-progress" aria-hidden>
        <span style={{ width: `${progress}%` }} />
      </div>

      <main className="onboard-main" key={animKey}>
        <section className="onboard-copy">
          <p className="onboard-kicker">Step {stepIndex + 1} of {STEPS.length}</p>
          <h1 className="onboard-title">{headline}</h1>
          <p className="onboard-sub">{sub}</p>

          {step === "name" && (
            <div className="onboard-panel">
              <label className="op-field-label">Display name</label>
              <input
                className="onboard-input"
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && canContinue) next();
                }}
                placeholder="e.g. Alex"
              />
            </div>
          )}

          {step === "theme" && (
            <div className="onboard-panel">
              <Select
                label="Theme"
                value={theme}
                onChange={(v) => setTheme(v as ThemeId)}
                options={themes.map((t) => ({
                  value: t.id,
                  label: t.label,
                  description: t.description,
                }))}
              />
            </div>
          )}

          {step === "devices" && (
            <div className="onboard-panel onboard-devices">
              <Select
                label="Camera"
                icon={<IconCamera size={18} stroke={1.75} />}
                value={cameraId}
                onChange={setCameraId}
                options={devices.cameras.map((d) => ({
                  value: d.deviceId,
                  label: d.label,
                }))}
                emptyLabel="No camera found"
              />
              <Select
                label="Microphone"
                icon={<IconMicrophone size={18} stroke={1.75} />}
                value={micId}
                onChange={setMicId}
                options={devices.mics.map((d) => ({
                  value: d.deviceId,
                  label: d.label,
                }))}
                emptyLabel="No microphone found"
              />
              <div className="level-meter" aria-label="Microphone level">
                <span style={{ width: `${Math.round(micLevel * 100)}%` }} />
              </div>
              {permError && (
                <p className="onboard-error">
                  Allow camera & mic access to continue. {permError}
                </p>
              )}
            </div>
          )}

          {step === "ready" && (
            <ul className="onboard-checklist">
              <li>
                <span>
                  <IconCheck size={14} stroke={2.5} />
                </span>{" "}
                Local tracks on this machine
              </li>
              <li>
                <span>
                  <IconCheck size={14} stroke={2.5} />
                </span>{" "}
                Invite guests with a ticket
              </li>
              <li>
                <span>
                  <IconCheck size={14} stroke={2.5} />
                </span>{" "}
                Captions & Drive when you need them
              </li>
            </ul>
          )}

          <div className="onboard-actions">
            {step !== "welcome" && (
              <button type="button" className="btn btn-secondary" onClick={back}>
                <IconArrowLeft size={16} stroke={1.75} />
                Back
              </button>
            )}
            {step !== "ready" ? (
              <button
                type="button"
                className="btn btn-primary"
                disabled={!canContinue}
                onClick={next}
              >
                {step === "welcome" ? "Let’s go" : "Continue"}
                <IconArrowRight size={16} stroke={1.75} />
              </button>
            ) : (
              <button
                type="button"
                className="btn btn-primary"
                disabled={saving}
                onClick={() => void finish()}
              >
                {saving ? "Saving…" : "Enter OpenPodcast"}
              </button>
            )}
          </div>
        </section>

        <aside className="onboard-stage" aria-hidden={step !== "devices"}>
          <div className="onboard-stage-frame">
            {step === "devices" ? (
              <video
                ref={previewRef}
                className="onboard-video"
                muted
                playsInline
                autoPlay
              />
            ) : (
              <div className="onboard-stage-art">
                <div className="onboard-orb" />
                <div className="onboard-wave" />
                <p className="onboard-stage-label">
                  {step === "welcome" && "STUDIO READY"}
                  {step === "name" && (name.trim() || "YOUR NAME")}
                  {step === "theme" && themes.find((t) => t.id === theme)?.label.toUpperCase()}
                  {step === "ready" && (
                    <>
                      REC <IconCircleFilled size={10} />
                    </>
                  )}
                </p>
              </div>
            )}
          </div>
        </aside>
      </main>
    </div>
  );
}

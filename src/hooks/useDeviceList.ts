import { useCallback, useEffect, useState } from "react";

export type MediaKind = "audioinput" | "audiooutput" | "videoinput";

export interface ListedDevice {
  deviceId: string;
  label: string;
  kind: MediaKind;
}

function labelFor(d: MediaDeviceInfo, index: number): string {
  if (d.label) return d.label;
  if (d.kind === "audioinput") return `Microphone ${index + 1}`;
  if (d.kind === "audiooutput") return `Speaker ${index + 1}`;
  return `Camera ${index + 1}`;
}

async function ensurePermission(): Promise<void> {
  // Ask for mic first — on Windows WebView2 a combined cam+mic prompt often
  // fails entirely when camera is blocked, which also hides microphone labels.
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stream.getTracks().forEach((t) => t.stop());
  } catch {
    /* labels may stay empty until permission is granted */
  }
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ video: true });
    stream.getTracks().forEach((t) => t.stop());
  } catch {
    /* camera optional */
  }
}

export function useDeviceList(opts?: { requestPermission?: boolean }) {
  const [devices, setDevices] = useState<ListedDevice[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      if (opts?.requestPermission !== false) {
        await ensurePermission();
      }
      const list = await navigator.mediaDevices.enumerateDevices();
      const counters: Record<string, number> = {};
      const mapped: ListedDevice[] = list
        .filter(
          (d): d is MediaDeviceInfo & { kind: MediaKind } =>
            d.kind === "audioinput" ||
            d.kind === "audiooutput" ||
            d.kind === "videoinput",
        )
        .filter((d) => d.deviceId)
        .map((d) => {
          const n = counters[d.kind] ?? 0;
          counters[d.kind] = n + 1;
          return {
            deviceId: d.deviceId,
            label: labelFor(d, n),
            kind: d.kind,
          };
        });
      setDevices(mapped);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [opts?.requestPermission]);

  useEffect(() => {
    void refresh();
    const onChange = () => void refresh();
    navigator.mediaDevices?.addEventListener?.("devicechange", onChange);
    return () => {
      navigator.mediaDevices?.removeEventListener?.("devicechange", onChange);
    };
  }, [refresh]);

  return {
    devices,
    cameras: devices.filter((d) => d.kind === "videoinput"),
    mics: devices.filter((d) => d.kind === "audioinput"),
    speakers: devices.filter((d) => d.kind === "audiooutput"),
    loading,
    error,
    refresh,
  };
}

export function pickPreferred(
  list: ListedDevice[],
  preferredId: string | null | undefined,
): string {
  if (preferredId && list.some((d) => d.deviceId === preferredId)) {
    return preferredId;
  }
  return list[0]?.deviceId ?? "";
}

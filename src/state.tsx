import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { api } from "./api";
import type { AppSettings, SessionRecord, StudioRuntime } from "./types";

interface AppCtx {
  settings: AppSettings;
  sessions: SessionRecord[];
  studio: StudioRuntime | null;
  refreshSettings: () => Promise<void>;
  refreshSessions: () => Promise<void>;
  persistSettings: (patch: Partial<AppSettings>) => Promise<AppSettings>;
  setStudio: (s: StudioRuntime | null) => void;
}

const Ctx = createContext<AppCtx | null>(null);

const defaultSettings: AppSettings = {
  display_name: "Host",
  recording_folder: null,
  drive_connected: false,
  ai_base_url: null,
  ai_has_key: false,
  echo_cancellation: true,
  preferred_camera_id: null,
  preferred_mic_id: null,
  preferred_speaker_id: null,
  theme: "light",
};

export function AppProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [studio, setStudio] = useState<StudioRuntime | null>(null);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const refreshSettings = useCallback(async () => {
    try {
      setSettings(await api.getSettings());
    } catch {
      /* browser preview without Tauri */
    }
  }, []);

  const persistSettings = useCallback(async (patch: Partial<AppSettings>) => {
    const next: AppSettings = { ...settingsRef.current, ...patch };
    try {
      const saved = await api.saveSettings(next);
      setSettings(saved);
      return saved;
    } catch (e) {
      // Fall back to local merge so UI still reflects intent offline
      setSettings(next);
      throw e;
    }
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
    } catch {
      setSessions([]);
    }
  }, []);

  useEffect(() => {
    refreshSettings();
    refreshSessions();
  }, [refreshSettings, refreshSessions]);

  const value = useMemo(
    () => ({
      settings,
      sessions,
      studio,
      refreshSettings,
      refreshSessions,
      persistSettings,
      setStudio,
    }),
    [
      settings,
      sessions,
      studio,
      refreshSettings,
      refreshSessions,
      persistSettings,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useApp outside provider");
  return ctx;
}

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { api } from "./api";

export type ThemeId = "dark" | "light";

export interface ThemeDef {
  id: ThemeId;
  label: string;
  description: string;
}

export const THEMES: ThemeDef[] = [
  {
    id: "light",
    label: "Light",
    description: "Bright surfaces for daytime sessions",
  },
  {
    id: "dark",
    label: "Dark",
    description: "Signal Dark — cooler blacks with cyan accents",
  },
];

const STORAGE_KEY = "openpodcast.theme";
const LEGACY_DARK = new Set([
  "dark",
  "signal-dark",
  "night-booth",
  "graphite",
]);

function normalizeTheme(raw: string | null | undefined): ThemeId {
  if (raw && LEGACY_DARK.has(raw)) return "dark";
  return "light";
}

function applyTheme(id: ThemeId) {
  document.documentElement.setAttribute("data-theme", id);
  localStorage.setItem(STORAGE_KEY, id);
}

interface ThemeCtx {
  theme: ThemeId;
  setTheme: (id: ThemeId) => void;
  themes: ThemeDef[];
}

const Ctx = createContext<ThemeCtx | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemeId>(() => {
    const initial = normalizeTheme(localStorage.getItem(STORAGE_KEY));
    applyTheme(initial);
    return initial;
  });

  // Hydrate from persisted app settings (survives localStorage clears)
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const s = await api.getSettings();
        if (cancelled) return;
        const fromDisk = normalizeTheme(s.theme);
        const fromLocal = normalizeTheme(localStorage.getItem(STORAGE_KEY));
        // Prefer settings.json when it has an explicit theme; else keep local
        const next =
          s.theme === "light" || s.theme === "dark" ? fromDisk : fromLocal;
        setThemeState(next);
        applyTheme(next);
      } catch {
        /* browser preview */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const setTheme = useMemo(
    () => (id: ThemeId) => {
      setThemeState(id);
      applyTheme(id);
      void api
        .getSettings()
        .then((s) => api.saveSettings({ ...s, theme: id }))
        .catch(() => undefined);
    },
    [],
  );

  const value = useMemo(
    () => ({
      theme,
      setTheme,
      themes: THEMES,
    }),
    [theme, setTheme],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useTheme() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useTheme outside ThemeProvider");
  return ctx;
}

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type ThemeId = "dark" | "light";

export interface ThemeDef {
  id: ThemeId;
  label: string;
  description: string;
}

export const THEMES: ThemeDef[] = [
  {
    id: "dark",
    label: "Dark",
    description: "Signal Dark — cooler blacks with cyan accents",
  },
  {
    id: "light",
    label: "Light",
    description: "Bright surfaces for daytime sessions",
  },
];

const STORAGE_KEY = "openpodcast.theme";
const LEGACY_DARK = new Set([
  "dark",
  "signal-dark",
  "night-booth",
  "graphite",
]);

function normalizeTheme(raw: string | null): ThemeId {
  if (raw === "light") return "light";
  if (raw && LEGACY_DARK.has(raw)) return "dark";
  return "dark";
}

interface ThemeCtx {
  theme: ThemeId;
  setTheme: (id: ThemeId) => void;
  themes: ThemeDef[];
}

const Ctx = createContext<ThemeCtx | null>(null);

function applyTheme(id: ThemeId) {
  document.documentElement.setAttribute("data-theme", id);
  localStorage.setItem(STORAGE_KEY, id);
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemeId>(() => {
    const initial = normalizeTheme(localStorage.getItem(STORAGE_KEY));
    applyTheme(initial);
    return initial;
  });

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const value = useMemo(
    () => ({
      theme,
      setTheme: setThemeState,
      themes: THEMES,
    }),
    [theme],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useTheme() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useTheme outside ThemeProvider");
  return ctx;
}

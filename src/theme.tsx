import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type ThemeId = "night-booth" | "signal-dark" | "graphite";

export interface ThemeDef {
  id: ThemeId;
  label: string;
  description: string;
}

export const THEMES: ThemeDef[] = [
  {
    id: "night-booth",
    label: "Night Booth",
    description: "Graphite stage, phosphor meters, signal red",
  },
  {
    id: "signal-dark",
    label: "Signal Dark",
    description: "Cooler blacks with electric cyan accents",
  },
  {
    id: "graphite",
    label: "Graphite",
    description: "Neutral studio grey, soft amber focus",
  },
];

const STORAGE_KEY = "openpodcast.theme";

interface ThemeCtx {
  theme: ThemeId;
  setTheme: (id: ThemeId) => void;
  themes: ThemeDef[];
}

const Ctx = createContext<ThemeCtx | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemeId>(() => {
    const saved = localStorage.getItem(STORAGE_KEY) as ThemeId | null;
    return THEMES.some((t) => t.id === saved) ? saved! : "night-booth";
  });

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem(STORAGE_KEY, theme);
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

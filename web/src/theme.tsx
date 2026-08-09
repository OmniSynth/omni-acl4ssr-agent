import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type ThemeMode = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "omni-theme";

type ThemeCtx = {
  mode: ThemeMode;
  resolved: ResolvedTheme;
  setMode: (mode: ThemeMode) => void;
};

const ThemeContext = createContext<ThemeCtx | null>(null);

function systemPrefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function resolveTheme(mode: ThemeMode): ResolvedTheme {
  if (mode === "light") return "light";
  if (mode === "dark") return "dark";
  return systemPrefersDark() ? "dark" : "light";
}

export function applyTheme(resolved: ResolvedTheme) {
  document.documentElement.dataset.theme = resolved;
  document.documentElement.style.colorScheme = resolved;
}

export function readStoredMode(): ThemeMode {
  try {
    const v = localStorage.getItem(STORAGE_KEY) || localStorage.getItem("irez-theme");
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    /* ignore */
  }
  return "system";
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<ThemeMode>(() =>
    typeof window === "undefined" ? "system" : readStoredMode(),
  );
  const [resolved, setResolved] = useState<ResolvedTheme>(() =>
    typeof window === "undefined" ? "dark" : resolveTheme(readStoredMode()),
  );

  useEffect(() => {
    const next = resolveTheme(mode);
    setResolved(next);
    applyTheme(next);
    try {
      localStorage.setItem(STORAGE_KEY, mode);
    } catch {
      /* ignore */
    }
  }, [mode]);

  useEffect(() => {
    if (mode !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      const next = resolveTheme("system");
      setResolved(next);
      applyTheme(next);
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [mode]);

  const value = useMemo(
    () => ({
      mode,
      resolved,
      setMode: setModeState,
    }),
    [mode, resolved],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme() {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within ThemeProvider");
  return ctx;
}

const MODE_OPTIONS: { id: ThemeMode; label: string }[] = [
  { id: "system", label: "自动" },
  { id: "light", label: "明亮" },
  { id: "dark", label: "暗色" },
];

/** 顶栏主题：自动 / 明亮 / 暗色 */
export function ThemeToggle() {
  const { mode, resolved, setMode } = useTheme();
  return (
    <div
      className="inline-flex rounded-xl border border-edge bg-composer p-0.5"
      role="group"
      aria-label="主题"
    >
      {MODE_OPTIONS.map((opt) => {
        const active = mode === opt.id;
        return (
          <button
            key={opt.id}
            type="button"
            onClick={() => setMode(opt.id)}
            className={[
              "rounded-lg px-2.5 py-1 text-xs font-medium transition",
              active
                ? resolved === "dark"
                  ? "bg-zinc-100 text-zinc-900"
                  : "bg-zinc-900 text-white"
                : "text-fg-soft hover:bg-hover hover:text-fg",
            ].join(" ")}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

export type TipTone = "ok" | "danger" | "muted";

export type TipOptions = {
  message: string;
  tone?: TipTone;
  /** 毫秒，默认 2200；错误默认 5200 */
  duration?: number;
};

type TipItem = {
  id: number;
  message: string;
  tone: TipTone;
  duration: number;
};

type TipsApi = {
  (message: string | TipOptions): void;
  success: (message: string, duration?: number) => void;
  error: (message: string, duration?: number) => void;
  info: (message: string, duration?: number) => void;
  clear: () => void;
};

const TipsContext = createContext<TipsApi | null>(null);

const toneClass: Record<TipTone, string> = {
  ok: "border-ok/25 bg-surface text-ok",
  danger: "border-danger/25 bg-surface text-danger",
  muted: "border-edge bg-surface text-fg-soft",
};

export function TipsProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<TipItem[]>([]);
  const seq = useRef(0);
  const timers = useRef(new Map<number, number>());
  const hovered = useRef(new Set<number>());
  const durations = useRef(new Map<number, number>());

  const clearTimer = useCallback((id: number) => {
    const t = timers.current.get(id);
    if (t) {
      window.clearTimeout(t);
      timers.current.delete(id);
    }
  }, []);

  const dismiss = useCallback(
    (id: number) => {
      clearTimer(id);
      hovered.current.delete(id);
      durations.current.delete(id);
      setItems((prev) => prev.filter((x) => x.id !== id));
    },
    [clearTimer],
  );

  const scheduleDismiss = useCallback(
    (id: number, ms: number) => {
      clearTimer(id);
      if (hovered.current.has(id)) return;
      const t = window.setTimeout(() => dismiss(id), ms);
      timers.current.set(id, t);
    },
    [clearTimer, dismiss],
  );

  const clear = useCallback(() => {
    for (const id of [...timers.current.keys()]) clearTimer(id);
    hovered.current.clear();
    durations.current.clear();
    setItems([]);
  }, [clearTimer]);

  const push = useCallback(
    (options: string | TipOptions) => {
      const opts: TipOptions =
        typeof options === "string" ? { message: options } : options;
      const message = opts.message.trim();
      if (!message) return;
      // 同时只保留一条，避免「申请中」与失败原因叠在一起
      clear();
      const id = ++seq.current;
      const tone = opts.tone ?? "ok";
      const duration =
        opts.duration ?? (tone === "danger" ? 5200 : 2200);
      durations.current.set(id, duration);
      setItems([{ id, message, tone, duration }]);
      scheduleDismiss(id, duration);
    },
    [clear, scheduleDismiss],
  );

  const onEnter = useCallback(
    (id: number) => {
      hovered.current.add(id);
      clearTimer(id);
    },
    [clearTimer],
  );

  const onLeave = useCallback(
    (id: number) => {
      hovered.current.delete(id);
      // 移开后短暂再消失，便于读完长文案
      scheduleDismiss(id, 1200);
    },
    [scheduleDismiss],
  );

  const api = useMemo<TipsApi>(() => {
    const fn = ((message: string | TipOptions) => push(message)) as TipsApi;
    fn.success = (message, duration) => push({ message, tone: "ok", duration });
    fn.error = (message, duration) => push({ message, tone: "danger", duration });
    fn.info = (message, duration) => push({ message, tone: "muted", duration });
    fn.clear = clear;
    return fn;
  }, [push, clear]);

  return (
    <TipsContext.Provider value={api}>
      {children}
      {createPortal(
        <div
          className="pointer-events-none fixed inset-x-0 top-3 z-110 flex flex-col items-center gap-2 px-3"
          aria-live="polite"
        >
          {items.map((item) => (
            <div
              key={item.id}
              className={[
                "pointer-events-auto max-w-sm rounded-xl border px-3.5 py-2 text-sm font-medium shadow-lg",
                "animate-[tip-in_180ms_ease-out]",
                toneClass[item.tone],
              ].join(" ")}
              role="status"
              onMouseEnter={() => onEnter(item.id)}
              onMouseLeave={() => onLeave(item.id)}
            >
              {item.message}
            </div>
          ))}
        </div>,
        document.body,
      )}
    </TipsContext.Provider>
  );
}

export function useTips(): TipsApi {
  const ctx = useContext(TipsContext);
  if (!ctx) throw new Error("useTips 需在 TipsProvider 内使用");
  return ctx;
}

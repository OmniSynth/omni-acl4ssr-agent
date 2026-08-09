import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { Button } from "./components/ui";

export type ConfirmOptions = {
  title?: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** 危险操作：确认按钮用 danger 样式 */
  danger?: boolean;
};

export type ConfirmFn = (options: ConfirmOptions | string) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmFn | null>(null);

type DialogState = ConfirmOptions & { open: boolean };

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const resolveRef = useRef<((value: boolean) => void) | null>(null);
  const confirmBtnRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descId = useId();

  const confirm = useCallback<ConfirmFn>((options) => {
    const opts: ConfirmOptions =
      typeof options === "string" ? { message: options } : options;
    return new Promise<boolean>((resolve) => {
      // 若上一次未关闭，先按取消收尾
      resolveRef.current?.(false);
      resolveRef.current = resolve;
      setDialog({
        title: opts.title ?? "请确认",
        message: opts.message,
        confirmLabel: opts.confirmLabel ?? "确认",
        cancelLabel: opts.cancelLabel ?? "取消",
        danger: opts.danger ?? false,
        open: true,
      });
    });
  }, []);

  const finish = useCallback((value: boolean) => {
    const resolve = resolveRef.current;
    resolveRef.current = null;
    setDialog(null);
    resolve?.(value);
  }, []);

  useEffect(() => {
    if (!dialog?.open) return;
    const t = window.setTimeout(() => confirmBtnRef.current?.focus(), 0);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        finish(false);
      }
    };
    document.addEventListener("keydown", onKey);
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      window.clearTimeout(t);
      document.removeEventListener("keydown", onKey);
      document.body.style.overflow = prev;
    };
  }, [dialog?.open, finish]);

  return (
    <ConfirmContext.Provider value={confirm}>
      {children}
      {dialog?.open
        ? createPortal(
            <div
              className="fixed inset-0 z-100 flex items-center justify-center p-4"
              role="presentation"
            >
              <button
                type="button"
                aria-label="关闭"
                className="absolute inset-0 bg-black/40 backdrop-blur-[2px]"
                onClick={() => finish(false)}
              />
              <div
                role="alertdialog"
                aria-modal="true"
                aria-labelledby={titleId}
                aria-describedby={descId}
                className="relative z-10 w-full max-w-sm rounded-2xl border border-edge bg-surface p-5 shadow-(--app-shadow)"
              >
                <h3 id={titleId} className="text-[15px] font-semibold tracking-tight text-fg">
                  {dialog.title}
                </h3>
                <p id={descId} className="mt-2 text-sm leading-relaxed text-fg-soft">
                  {dialog.message}
                </p>
                <div className="mt-5 flex justify-end gap-2">
                  <Button variant="ghost" onClick={() => finish(false)}>
                    {dialog.cancelLabel}
                  </Button>
                  <Button
                    ref={confirmBtnRef}
                    variant={dialog.danger ? "danger" : "primary"}
                    onClick={() => finish(true)}
                  >
                    {dialog.confirmLabel}
                  </Button>
                </div>
              </div>
            </div>,
            document.body,
          )
        : null}
    </ConfirmContext.Provider>
  );
}

export function useConfirm(): ConfirmFn {
  const ctx = useContext(ConfirmContext);
  if (!ctx) {
    throw new Error("useConfirm 需在 ConfirmProvider 内使用");
  }
  return ctx;
}

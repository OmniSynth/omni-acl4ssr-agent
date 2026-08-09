import {
  forwardRef,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from "react";
import { useTheme } from "../theme";

export function Card({
  title,
  children,
  actions,
}: {
  title: string;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section className="min-h-full overflow-hidden bg-surface">
      <div className="flex items-center justify-between gap-3 px-4 py-3.5 sm:px-5">
        <h2 className="text-[15px] font-semibold tracking-tight text-fg">{title}</h2>
        {actions}
      </div>
      <div className="border-t border-edge px-4 py-4 sm:px-5">{children}</div>
    </section>
  );
}

export function Label({ children }: { children: ReactNode }) {
  return (
    <label className="mb-1.5 block text-[11px] font-medium tracking-wide text-fg-soft">
      {children}
    </label>
  );
}

const fieldClass = [
  "w-full rounded-xl border border-edge bg-input px-3 py-2.5 text-sm text-fg outline-none transition",
  "placeholder:text-faint hover:border-fg/20 focus:border-accent focus:ring-2 focus:ring-accent-soft",
  "disabled:cursor-not-allowed disabled:opacity-45",
].join(" ");

export function Input(props: InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={[fieldClass, props.className || ""].join(" ")} />;
}

export function Select(props: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select {...props} className={[fieldClass, props.className || ""].join(" ")} />;
}

export type SearchableOption = {
  value: string;
  label: string;
  badge?: string;
  badgeTone?: "ok" | "warn" | "muted" | "danger";
};

function badgeClass(tone?: SearchableOption["badgeTone"]) {
  if (tone === "ok") return "bg-ok-soft text-ok";
  if (tone === "warn") return "bg-warn-soft text-warn";
  if (tone === "danger") return "bg-danger-soft text-danger";
  return "bg-hover text-muted";
}

export function SearchableSelect({
  value,
  options,
  onChange,
  disabled,
  placeholder = "搜索…",
  emptyText = "无匹配项",
  variant = "field",
  placement = "bottom",
  className = "",
  footer,
}: {
  value: string;
  options: SearchableOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  emptyText?: string;
  variant?: "field" | "pill";
  placement?: "bottom" | "top";
  className?: string;
  footer?: ReactNode;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  const selected = options.find((o) => o.value === value);
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return options;
    return options.filter(
      (o) => o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q),
    );
  }, [options, query]);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    const t = window.setTimeout(() => searchRef.current?.focus(), 0);
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      window.clearTimeout(t);
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const triggerClass =
    variant === "pill"
      ? [
          "inline-flex max-w-[12rem] items-center gap-1.5 rounded-lg border border-edge px-2.5 py-1 text-xs",
          "bg-input text-fg-soft transition hover:border-fg/20 hover:text-fg",
          "outline-none disabled:cursor-not-allowed disabled:opacity-45",
          open ? "border-accent text-fg" : "",
        ].join(" ")
      : [
          fieldClass,
          "flex items-center justify-between gap-2 text-left",
          open ? "border-accent" : "",
        ].join(" ");

  const menuClass = [
    "absolute z-40 w-72 max-w-[min(18rem,calc(100vw-2rem))] overflow-hidden rounded-xl border border-edge",
    "bg-menu shadow-[var(--app-shadow)]",
    placement === "top" ? "bottom-full left-0 mb-1.5" : "top-full left-0 mt-1",
    variant === "field" ? "w-full max-w-none" : "",
  ].join(" ");

  return (
    <div ref={rootRef} className={["relative", className].join(" ")}>
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        className={triggerClass}
        title={selected?.label || value}
      >
        <span className="truncate">{selected?.label || value || "请选择"}</span>
        {selected?.badge && (
          <span
            className={[
              "shrink-0 rounded-md px-1.5 py-0.5 text-[10px] font-medium leading-none",
              badgeClass(selected.badgeTone),
            ].join(" ")}
          >
            {selected.badge}
          </span>
        )}
        <span className="shrink-0 text-faint">{open ? "▴" : "▾"}</span>
      </button>
      {open && (
        <div className={menuClass}>
          <div className="border-b border-edge p-2">
            <input
              ref={searchRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={placeholder}
              className="w-full rounded-lg border border-edge bg-input px-2.5 py-1.5 text-sm text-fg outline-none placeholder:text-faint focus:border-accent"
            />
          </div>
          <ul className="max-h-64 overflow-y-auto overscroll-contain py-1">
            {filtered.length === 0 ? (
              <li className="px-3 py-2.5 text-xs text-muted">{emptyText}</li>
            ) : (
              filtered.map((o) => {
                const active = o.value === value;
                return (
                  <li key={o.value}>
                    <button
                      type="button"
                      onClick={() => {
                        onChange(o.value);
                        setOpen(false);
                      }}
                      className={[
                        "flex w-full items-start gap-2 px-3 py-2 text-left text-sm transition",
                        active ? "bg-accent-soft text-accent-fg" : "text-fg-soft hover:bg-hover",
                      ].join(" ")}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">{o.label}</span>
                        {o.label !== o.value && (
                          <span className="mt-0.5 block truncate text-[11px] text-faint">
                            {o.value}
                          </span>
                        )}
                      </span>
                      {o.badge && (
                        <span
                          className={[
                            "mt-0.5 shrink-0 rounded-md px-1.5 py-0.5 text-[10px] font-medium leading-none",
                            badgeClass(o.badgeTone),
                          ].join(" ")}
                        >
                          {o.badge}
                        </span>
                      )}
                    </button>
                  </li>
                );
              })
            )}
          </ul>
          {footer && (
            <div className="border-t border-edge px-3 py-2 text-[11px] leading-snug text-muted">
              {footer}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(
  function Textarea(props, ref) {
    return (
      <textarea
        ref={ref}
        {...props}
        className={[fieldClass, "font-mono leading-relaxed", props.className || ""].join(" ")}
      />
    );
  },
);

export const Button = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "ghost" | "danger" }
>(function Button({ variant = "primary", type, className, ...rest }, ref) {
  const { resolved } = useTheme();
  const styles =
    variant === "primary"
      ? resolved === "dark"
        ? "bg-zinc-100 text-zinc-900 hover:bg-white"
        : "bg-zinc-900 text-white hover:bg-zinc-800"
      : variant === "danger"
        ? "border border-danger/25 bg-danger-soft text-danger hover:bg-danger/15"
        : "border border-edge bg-surface text-fg-soft hover:border-fg/20 hover:bg-hover hover:text-fg";
  return (
    <button
      ref={ref}
      type={type ?? "button"}
      {...rest}
      className={[
        "rounded-xl px-3.5 py-1.5 text-sm font-medium transition disabled:cursor-not-allowed disabled:opacity-40",
        styles,
        className || "",
      ].join(" ")}
    />
  );
});

export function Msg({ error, ok }: { error?: string; ok?: string }) {
  if (error) return <p className="text-sm text-danger">{error}</p>;
  if (ok) return <p className="text-sm text-ok">{ok}</p>;
  return null;
}

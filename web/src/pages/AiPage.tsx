import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
  type ReactNode,
} from "react";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import { useConfirm } from "../confirm";
import { pathForAiOps, requestConfigReload } from "../configReload";
import { useTips } from "../tips";
import type {
  AiAttachment,
  AiChatMessage,
  AiChatSummary,
  AiChatThread,
  AiModelInfo,
  AiOp,
  AiProvider,
  AiSettingsPublic,
  AiUsage,
} from "../types";
import { Button, Input, Label, SearchableSelect, Select, Textarea } from "../components/ui";
import { SpeechDictation, speechDictationSupported } from "../speechDictation";
import { useTheme } from "../theme";

const DEFAULT_CONTEXT_CHOICES = [0, 32_768, 65_536, 131_072, 262_144, 524_288, 1_048_576];

function formatContextWindowOption(n: number, modelMax?: number): string {
  if (!n) {
    return modelMax
      ? `跟随模型（${formatTokens(modelMax)}）`
      : "跟随模型（推荐）";
  }
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1)}M tokens`;
  if (n >= 1000) return `${Math.round(n / 1000)}k tokens`;
  return `${n} tokens`;
}

/** 仅保留当前模型/Key 实际上限内的档位；未获知上限时只允许「跟随模型」。 */
function contextChoicesForModel(
  all: number[],
  modelMax: number | undefined,
  modelKnown: boolean,
): number[] {
  if (!modelKnown || !modelMax || modelMax <= 0) return [0];
  const allowed = all.filter((n) => n === 0 || n <= modelMax);
  return allowed.length ? allowed : [0];
}

function effectiveContextLimit(
  pref: number | undefined,
  modelMax: number | undefined,
  usageLimit: number | undefined,
): number {
  const max = modelMax || usageLimit || 1_048_576;
  if (pref && pref > 0) return Math.min(pref, max);
  return usageLimit || max;
}

const MAX_ATTACH = 5;
const MAX_ATTACH_BYTES = 4 * 1024 * 1024;
const ACCEPT_FILES = "image/jpeg,image/png,image/webp,image/gif,.jpg,.jpeg,.png,.webp,.gif";

const IMAGE_MIME_PREFIX = ["image/jpeg", "image/png", "image/webp", "image/gif"];

type LocalAttach = AiAttachment & { id: string };

function isImageMime(mime: string): boolean {
  const m = mime.trim().toLowerCase();
  return IMAGE_MIME_PREFIX.some((x) => m === x || m.startsWith(x + ";"));
}

function mimeAllowed(mime: string): boolean {
  return isImageMime(mime);
}

function attachDataUrl(a: LocalAttach): string {
  return `data:${a.mime_type};base64,${a.data_base64}`;
}

const PREVIEW_ZOOM_MIN = 0.5;
const PREVIEW_ZOOM_MAX = 4;
const PREVIEW_ZOOM_STEP = 0.25;

function clampPreviewZoom(n: number): number {
  return Math.min(PREVIEW_ZOOM_MAX, Math.max(PREVIEW_ZOOM_MIN, Math.round(n * 100) / 100));
}

function ImagePreviewer({
  images,
  currentId,
  onClose,
  onChangeId,
}: {
  images: LocalAttach[];
  currentId: string;
  onClose: () => void;
  onChangeId: (id: string) => void;
}) {
  const idx = Math.max(
    0,
    images.findIndex((a) => a.id === currentId),
  );
  const current = images[idx] ?? images[0];
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const stageRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{
    startX: number;
    startY: number;
    origX: number;
    origY: number;
  } | null>(null);
  const idxRef = useRef(idx);
  const imagesRef = useRef(images);
  idxRef.current = idx;
  imagesRef.current = images;

  useEffect(() => {
    setScale(1);
    setOffset({ x: 0, y: 0 });
    setDragging(false);
  }, [currentId]);

  const go = (delta: number) => {
    const list = imagesRef.current;
    if (list.length <= 1) return;
    const next = (idxRef.current + delta + list.length) % list.length;
    onChangeId(list[next].id);
  };

  const zoomBy = (delta: number) => {
    setScale((s) => {
      const next = clampPreviewZoom(s + delta);
      if (next <= 1) setOffset({ x: 0, y: 0 });
      return next;
    });
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      } else if (e.key === "+" || e.key === "=") {
        e.preventDefault();
        zoomBy(PREVIEW_ZOOM_STEP);
      } else if (e.key === "-" || e.key === "_") {
        e.preventDefault();
        zoomBy(-PREVIEW_ZOOM_STEP);
      } else if (e.key === "0") {
        e.preventDefault();
        setScale(1);
        setOffset({ x: 0, y: 0 });
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onClose, onChangeId]);

  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      zoomBy(e.deltaY > 0 ? -PREVIEW_ZOOM_STEP : PREVIEW_ZOOM_STEP);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!current) return null;

  const btnClass =
    "rounded-full bg-white/15 px-3 py-1.5 text-sm text-white transition hover:bg-white/25 disabled:opacity-35";

  return (
    <div
      className="fixed inset-0 z-120 flex flex-col bg-black/75"
      role="dialog"
      aria-modal="true"
      aria-label="图片预览"
      onClick={onClose}
    >
      <div
        className="flex shrink-0 items-center justify-between gap-2 px-3 py-2.5 sm:px-4"
        onClick={(e) => e.stopPropagation()}
      >
        <span className="truncate text-sm text-white/80 tabular-nums">
          {idx + 1} / {images.length}
          {current.name ? ` · ${current.name}` : ""}
        </span>
        <div className="flex shrink-0 items-center gap-1.5">
          <button type="button" className={btnClass} onClick={() => zoomBy(-PREVIEW_ZOOM_STEP)} title="缩小">
            −
          </button>
          <button
            type="button"
            className={`${btnClass} min-w-14 tabular-nums`}
            onClick={() => {
              setScale(1);
              setOffset({ x: 0, y: 0 });
            }}
            title="重置缩放"
          >
            {Math.round(scale * 100)}%
          </button>
          <button type="button" className={btnClass} onClick={() => zoomBy(PREVIEW_ZOOM_STEP)} title="放大">
            +
          </button>
          <button type="button" className={btnClass} onClick={onClose}>
            关闭
          </button>
        </div>
      </div>

      <div ref={stageRef} className="relative min-h-0 flex-1 overflow-hidden" onClick={onClose}>
        {images.length > 1 ? (
          <>
            <button
              type="button"
              className="absolute left-2 top-1/2 z-10 -translate-y-1/2 rounded-full bg-white/15 px-3 py-2 text-lg text-white hover:bg-white/25 sm:left-4"
              title="上一张 ←"
              onClick={(e) => {
                e.stopPropagation();
                go(-1);
              }}
            >
              ‹
            </button>
            <button
              type="button"
              className="absolute right-2 top-1/2 z-10 -translate-y-1/2 rounded-full bg-white/15 px-3 py-2 text-lg text-white hover:bg-white/25 sm:right-4"
              title="下一张 →"
              onClick={(e) => {
                e.stopPropagation();
                go(1);
              }}
            >
              ›
            </button>
          </>
        ) : null}
        <div className="flex h-full w-full items-center justify-center p-4">
          <img
            src={attachDataUrl(current)}
            alt={current.name || "预览"}
            draggable={false}
            className={[
              "max-h-full max-w-full select-none rounded-xl object-contain shadow-lg",
              scale > 1 ? "cursor-grab active:cursor-grabbing" : "cursor-default",
            ].join(" ")}
            style={{
              transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})`,
              transformOrigin: "center center",
              transition: dragging ? "none" : "transform 120ms ease-out",
            }}
            onClick={(e) => e.stopPropagation()}
            onDoubleClick={(e) => {
              e.stopPropagation();
              if (scale > 1) {
                setScale(1);
                setOffset({ x: 0, y: 0 });
              } else {
                setScale(2);
              }
            }}
            onPointerDown={(e) => {
              if (scale <= 1) return;
              e.preventDefault();
              e.stopPropagation();
              (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
              dragRef.current = {
                startX: e.clientX,
                startY: e.clientY,
                origX: offset.x,
                origY: offset.y,
              };
              setDragging(true);
            }}
            onPointerMove={(e) => {
              const d = dragRef.current;
              if (!d) return;
              setOffset({
                x: d.origX + (e.clientX - d.startX),
                y: d.origY + (e.clientY - d.startY),
              });
            }}
            onPointerUp={() => {
              dragRef.current = null;
              setDragging(false);
            }}
            onPointerCancel={() => {
              dragRef.current = null;
              setDragging(false);
            }}
          />
        </div>
      </div>
    </div>
  );
}

function normalizePasteFile(file: File, index: number): File {
  const type = file.type || "application/octet-stream";
  if (file.name && file.name !== "image.png" && file.name !== "blob") return file;
  const ext =
    type === "image/jpeg"
      ? "jpg"
      : type === "image/png"
        ? "png"
        : type === "image/webp"
          ? "webp"
          : type === "image/gif"
            ? "gif"
            : "bin";
  return new File([file], `paste-${Date.now()}-${index + 1}.${ext}`, { type });
}

function filesFromClipboard(data: DataTransfer | null): File[] {
  if (!data) return [];
  const out: File[] = [];
  const seen = new Set<string>();
  const push = (f: File | null) => {
    if (!f || !f.size) return;
    const key = `${f.name}:${f.size}:${f.type}:${f.lastModified}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push(f);
  };
  if (data.items) {
    for (let i = 0; i < data.items.length; i++) {
      const item = data.items[i];
      if (item.kind === "file") push(item.getAsFile());
    }
  }
  if (data.files?.length) {
    for (let i = 0; i < data.files.length; i++) push(data.files[i]);
  }
  return out;
}

function IconBtn({
  title,
  disabled,
  active,
  dense,
  onClick,
  children,
}: {
  title: string;
  disabled?: boolean;
  active?: boolean;
  dense?: boolean;
  onClick?: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      aria-pressed={active || undefined}
      disabled={disabled}
      onClick={onClick}
      className={[
        "inline-flex shrink-0 items-center justify-center rounded-full transition",
        "disabled:cursor-not-allowed disabled:opacity-40",
        dense ? "h-7 w-7" : "h-8 w-8",
        active
          ? "bg-accent-soft text-accent"
          : "text-faint hover:bg-hover hover:text-fg",
      ].join(" ")}
    >
      {children}
    </button>
  );
}

/** 主操作：实心圆 + 深色线标（发送 / 语音），对齐参考图 */
function PrimaryCircleBtn({
  title,
  disabled,
  recording,
  onClick,
  children,
}: {
  title: string;
  disabled?: boolean;
  recording?: boolean;
  onClick?: () => void;
  children: ReactNode;
}) {
  const { resolved } = useTheme();
  const styles = recording
    ? "bg-danger text-white hover:opacity-90"
    : resolved === "dark"
      ? "bg-zinc-100 text-zinc-900 hover:bg-white"
      : "bg-zinc-900 text-white hover:bg-zinc-800";
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className={[
        "inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-full transition",
        "disabled:cursor-not-allowed disabled:opacity-40",
        styles,
      ].join(" ")}
    >
      {children}
    </button>
  );
}

function IconPlus() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

function IconHistory() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M3 3v5h5" />
      <path d="M3.05 13A9 9 0 1 0 6 5.3L3 8" />
      <path d="M12 7v5l3 2" />
    </svg>
  );
}

function IconSettings() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function IconArchive({ size = 15 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M21 8v13H3V8" />
      <path d="M23 3H1v5h22V3z" />
      <path d="M10 12h4" />
    </svg>
  );
}

function IconTrash({ size = 15 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M3 6h18" />
      <path d="M8 6V4h8v2" />
      <path d="M19 6l-1 14H6L5 6" />
      <path d="M10 11v6M14 11v6" />
    </svg>
  );
}

function IconPaperclip() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48" />
    </svg>
  );
}

function formatChatUpdatedAt(iso: string): string {
  const raw = (iso || "").trim();
  if (!raw) return "";
  const d = new Date(raw);
  if (Number.isNaN(d.getTime())) {
    return raw.slice(0, 16).replace("T", " ");
  }
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getMonth() + 1}/${d.getDate()} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function IconMic() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M12 2a3 3 0 00-3 3v7a3 3 0 006 0V5a3 3 0 00-3-3z" />
      <path d="M19 10v1a7 7 0 01-14 0v-1M12 18v4M8 22h8" />
    </svg>
  );
}

function IconSend() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M12 19V5" />
      <path d="M5 12l7-7 7 7" />
    </svg>
  );
}

function readFileAsAttachment(file: File): Promise<LocalAttach> {
  return new Promise((resolve, reject) => {
    if (file.size > MAX_ATTACH_BYTES) {
      reject(new Error(`「${file.name}」超过 4MB 上限`));
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const result = String(reader.result || "");
      const i = result.indexOf("base64,");
      const data = i >= 0 ? result.slice(i + 7) : result;
      const mime = file.type || "application/octet-stream";
      resolve({
        id: `att-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
        mime_type: mime === "audio/mp3" ? "audio/mpeg" : mime,
        data_base64: data,
        name: file.name,
      });
    };
    reader.onerror = () => reject(new Error(`读取「${file.name}」失败`));
    reader.readAsDataURL(file);
  });
}

const FALLBACK_GEMINI_MODELS: AiModelInfo[] = [
  {
    id: "gemini-2.0-flash",
    display_name: "Gemini 2.0 Flash",
    tier: "free",
    tier_label: "免费",
    input_token_limit: 1048576,
  },
  {
    id: "gemini-2.5-flash",
    display_name: "Gemini 2.5 Flash",
    tier: "free",
    tier_label: "免费",
    input_token_limit: 1048576,
  },
  {
    id: "gemini-2.0-flash-lite",
    display_name: "Gemini 2.0 Flash-Lite",
    tier: "free",
    tier_label: "免费",
    input_token_limit: 1048576,
  },
];

const FALLBACK_DEEPSEEK_MODELS: AiModelInfo[] = [
  {
    id: "deepseek-v4-flash",
    display_name: "DeepSeek V4 Flash",
    tier: "paid",
    tier_label: "按量",
    input_token_limit: 1048576,
    output_token_limit: 384000,
  },
  {
    id: "deepseek-v4-pro",
    display_name: "DeepSeek V4 Pro",
    tier: "paid",
    tier_label: "按量",
    input_token_limit: 1048576,
    output_token_limit: 384000,
  },
];

function fallbackModels(provider: string): AiModelInfo[] {
  return provider === "deepseek" ? FALLBACK_DEEPSEEK_MODELS : FALLBACK_GEMINI_MODELS;
}

function defaultModelFor(provider: string): string {
  return provider === "deepseek" ? "deepseek-v4-flash" : "gemini-2.0-flash";
}

const CHAT_STORAGE_KEY = "omni-ai-chat-id";

function modelBadgeTone(tier?: string): "ok" | "warn" | "muted" | "danger" {
  if (tier === "free") return "ok";
  if (tier === "paid") return "warn";
  return "muted";
}

function modelQuotaBlocked(usage: AiUsage | null, modelId: string): boolean {
  if (usage?.balance_available === false) return true;
  if (!usage?.quota_exhausted || !usage.quota_blocked_model) return false;
  const a = usage.quota_blocked_model.toLowerCase();
  const b = modelId.toLowerCase();
  return a === b || a.includes(b) || b.includes(a);
}

function formatTokens(n: number): string {
  if (!n || n < 0) return "0";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

function formatMoney(amount: string | null | undefined, currency?: string | null): string | null {
  const raw = amount?.trim();
  if (!raw) return null;
  const cur = (currency || "CNY").toUpperCase();
  if (cur === "USD") return `$${raw}`;
  if (cur === "CNY") return `¥${raw}`;
  return `${raw} ${cur}`;
}

function formatBalance(usage: AiUsage | null | undefined): string | null {
  return formatMoney(usage?.balance_total, usage?.balance_currency);
}

function RingMeter({
  pct,
  label,
  warn,
}: {
  pct: number;
  label: string;
  warn: boolean;
}) {
  const size = 18;
  const stroke = 2.25;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const clamped = Math.max(0, Math.min(100, pct));
  const offset = c * (1 - clamped / 100);
  const color = warn ? "var(--app-warn)" : "var(--app-accent)";
  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      className="-rotate-90"
      aria-hidden
    >
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke="var(--app-border)"
        strokeWidth={stroke}
      />
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke={color}
        strokeWidth={stroke}
        strokeLinecap="round"
        strokeDasharray={c}
        strokeDashoffset={offset}
        className="transition-[stroke-dashoffset]"
      />
      <title>{label}</title>
    </svg>
  );
}

function UsageTipRow({
  label,
  detail,
  pct,
}: {
  label: string;
  detail: string;
  pct: number;
}) {
  const clamped = Math.max(0, Math.min(100, pct));
  const warn = clamped >= 80;
  return (
    <div>
      <div className="mb-1 flex items-baseline justify-between gap-2 text-[10px] leading-none">
        <span className="font-medium text-fg">{label}</span>
        <span className="truncate tabular-nums text-fg-soft">{detail}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-edge/80">
        <div
          className={`h-full rounded-full transition-all ${warn ? "bg-warn" : "bg-accent"}`}
          style={{ width: `${clamped <= 0 ? 0 : Math.max(6, clamped)}%` }}
        />
      </div>
    </div>
  );
}

function UsageRings({
  contextPct,
  quotaPct,
  contextLine,
  quotaLine,
  tokensLine,
  exhausted,
  exhaustedHint,
}: {
  contextPct: number;
  quotaPct: number;
  contextLine: string;
  quotaLine: string;
  tokensLine: string;
  exhausted?: boolean;
  exhaustedHint?: string;
}) {
  const ctx = Math.max(0, Math.min(100, contextPct));
  const q = Math.max(0, Math.min(100, exhausted ? 100 : quotaPct));
  return (
    <div className="group relative shrink-0">
      <div
        tabIndex={0}
        className="relative flex cursor-default items-center gap-1 rounded-md outline-none focus-visible:ring-1 focus-visible:ring-accent"
        aria-label={
          exhausted
            ? `${exhaustedHint || "额度已用尽"}；${contextLine}`
            : `${contextLine}；${quotaLine}`
        }
      >
        <RingMeter pct={ctx} label="上下文" warn={ctx >= 80} />
        <RingMeter pct={q} label="额度" warn={exhausted || q >= 80} />
        {exhausted ? (
          <span className="absolute -right-1 -top-1.5 rounded bg-danger px-1 py-px text-[9px] font-semibold leading-none text-white">
            尽
          </span>
        ) : null}
      </div>
      <div
        role="tooltip"
        className={[
          "pointer-events-none absolute bottom-full right-0 z-50 mb-2 w-56",
          "rounded-xl border border-edge bg-menu px-3 py-2.5 text-[11px] leading-relaxed text-fg-soft",
          "opacity-0 shadow-lg transition-opacity",
          "group-hover:opacity-100 group-focus-within:opacity-100",
        ].join(" ")}
      >
        {exhausted ? (
          <p className="mb-2 rounded-lg border border-danger/25 bg-danger-soft px-2 py-1.5 text-[10px] font-medium text-danger">
            {exhaustedHint || "当前模型额度已用尽，请换 flash 模型或稍后再试"}
          </p>
        ) : null}
        <div className="flex flex-col gap-2.5">
          <UsageTipRow label="上下文" detail={contextLine} pct={ctx} />
          <UsageTipRow label="额度" detail={quotaLine} pct={q} />
        </div>
        <p className="mt-2.5 border-t border-edge pt-1.5 text-[10px] text-faint">{tokensLine}</p>
      </div>
    </div>
  );
}

function messageKind(m: AiChatMessage): string {
  return (m.kind || (m.ops?.length ? "plan" : "plan")).toLowerCase();
}

function lastAssistantWithOps(messages: AiChatMessage[]): AiChatMessage | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (
      m.role === "assistant" &&
      messageKind(m) === "plan" &&
      m.ops?.length &&
      !m.applied_at
    ) {
      return m;
    }
  }
  return null;
}

function lastAskPending(messages: AiChatMessage[]): AiChatMessage | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role !== "assistant") continue;
    if (messageKind(m) === "ask" && m.options?.length) return m;
    break;
  }
  return null;
}

export default function AiPage() {
  const confirm = useConfirm();
  const tips = useTips();
  const navigate = useNavigate();
  const [settings, setSettings] = useState<AiSettingsPublic | null>(null);
  const [provider, setProvider] = useState<AiProvider>("gemini");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("gemini-2.0-flash");
  const [contextWindow, setContextWindow] = useState(0);
  const [thinkingEnabled, setThinkingEnabled] = useState(false);
  const [deepseekSpentSync, setDeepseekSpentSync] = useState("");
  const [models, setModels] = useState<AiModelInfo[]>(FALLBACK_GEMINI_MODELS);
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [liveStatus, setLiveStatus] = useState("");
  const [liveThinking, setLiveThinking] = useState("");
  const [liveSteps, setLiveSteps] = useState<string[]>([]);
  const [thinkingOpen, setThinkingOpen] = useState(true);
  /** 发送后立刻显示在对话里，等服务端落库后再清掉 */
  const [pendingUser, setPendingUser] = useState<string | null>(null);
  const [liveElapsed, setLiveElapsed] = useState(0);
  const thinkingBufRef = useRef("");
  const thinkingRafRef = useRef(0);
  const [loadingModels, setLoadingModels] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [usage, setUsage] = useState<AiUsage | null>(null);
  const [attachments, setAttachments] = useState<LocalAttach[]>([]);
  const attachmentsRef = useRef<LocalAttach[]>([]);
  const [previewId, setPreviewId] = useState<string | null>(null);
  const [listening, setListening] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const pinComposerCaretRef = useRef(false);
  const speechRef = useRef<SpeechDictation | null>(null);
  const promptRef = useRef(prompt);
  promptRef.current = prompt;

  useLayoutEffect(() => {
    if (!pinComposerCaretRef.current && !listening) return;
    pinComposerCaretRef.current = false;
    const el = composerRef.current;
    if (!el) return;
    const len = el.value.length;
    try {
      if (listening) el.focus({ preventScroll: true });
      el.setSelectionRange(len, len);
    } catch {
      /* ignore */
    }
    el.scrollTop = el.scrollHeight;
  }, [prompt, listening]);

  useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);

  const [chatId, setChatId] = useState<string | null>(() => {
    try {
      return localStorage.getItem(CHAT_STORAGE_KEY);
    } catch {
      return null;
    }
  });
  const [chat, setChat] = useState<AiChatThread | null>(null);
  const [chats, setChats] = useState<AiChatSummary[]>([]);
  const [showHistory, setShowHistory] = useState(false);
  const [showArchived, setShowArchived] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const liveThinkingPreRef = useRef<HTMLPreElement>(null);
  const stickScrollRef = useRef(true);

  function scrollChatToBottom() {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }

  function scrollLiveThinkingToBottom() {
    const pre = liveThinkingPreRef.current;
    if (!pre) return;
    pre.scrollTop = pre.scrollHeight;
  }

  function persistChatId(id: string | null) {
    setChatId(id);
    try {
      if (id) localStorage.setItem(CHAT_STORAGE_KEY, id);
      else localStorage.removeItem(CHAT_STORAGE_KEY);
    } catch {
      /* ignore */
    }
  }

  async function loadModels(silent = false, providerOverride?: string) {
    const p = providerOverride || provider;
    setLoadingModels(true);
    try {
      const r = await api.getAiModels();
      const list = r.models?.length ? r.models : fallbackModels(p);
      setModels(list);
      setModel((prev) => {
        if (list.some((m) => m.id === prev)) return prev;
        return list[0]?.id || defaultModelFor(p);
      });
      if (!silent) tips.success(`已加载 ${list.length} 个可用模型`);
    } catch (e) {
      setModels(fallbackModels(p));
      if (!silent) tips.error(String((e as Error).message || e));
    } finally {
      setLoadingModels(false);
    }
  }

  async function loadUsage() {
    try {
      setUsage(await api.getAiUsage());
    } catch {
      /* 忽略 */
    }
  }

  async function refreshChatList(includeArchived = showArchived) {
    try {
      const r = await api.listAiChats(includeArchived);
      setChats(r.chats || []);
    } catch {
      /* 忽略 */
    }
  }

  async function openChat(id: string | null) {
    if (!id) {
      setChat(null);
      persistChatId(null);
      return;
    }
    try {
      const c = await api.getAiChat(id);
      setChat(c);
      persistChatId(c.id);
      if (c.model) setModel(c.model);
    } catch {
      setChat(null);
      persistChatId(null);
    }
  }

  useEffect(() => {
    api
      .getAiSettings()
      .then((s) => {
        const p = (s.provider || "gemini") as AiProvider;
        setSettings(s);
        setProvider(p);
        setModel(s.model || defaultModelFor(p));
        setContextWindow(s.context_window ?? 0);
        setThinkingEnabled(!!s.thinking_enabled);
        setDeepseekSpentSync(
          s.deepseek_spent_sync != null && Number.isFinite(s.deepseek_spent_sync)
            ? String(s.deepseek_spent_sync)
            : "",
        );
        setApiKey(s.api_key_masked || "");
        setModels(fallbackModels(p));
        if (!s.has_api_key) setShowSettings(true);
        if (s.has_api_key) {
          return loadModels(true);
        }
      })
      .then(() => loadUsage())
      .then(() => refreshChatList(false))
      .then(() => {
        const saved = (() => {
          try {
            return localStorage.getItem(CHAT_STORAGE_KEY);
          } catch {
            return null;
          }
        })();
        if (saved) return openChat(saved);
      })
      .catch((e) => tips.error(String(e.message || e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useLayoutEffect(() => {
    if (!stickScrollRef.current) return;
    scrollChatToBottom();
    if (busy && thinkingOpen) scrollLiveThinkingToBottom();
  }, [
    chat?.messages.length,
    busy,
    pendingUser,
    liveThinking,
    liveSteps.length,
    liveStatus,
    thinkingOpen,
    liveElapsed,
  ]);

  useEffect(() => {
    if (busy) stickScrollRef.current = true;
  }, [busy]);

  async function persistSettings(
    partial?: {
      provider?: string;
      api_key?: string;
      model?: string;
      context_window?: number;
      thinking_enabled?: boolean;
      deepseek_spent_sync?: number | null;
    },
    opts?: { includeSpentSync?: boolean },
  ) {
    const p = partial?.provider ?? provider;
    const body: Parameters<typeof api.putAiSettings>[0] = {
      provider: p,
      api_key: partial?.api_key ?? apiKey,
      model: (partial?.model ?? model).trim() || defaultModelFor(p),
      context_window: partial?.context_window ?? contextWindow,
      thinking_enabled: partial?.thinking_enabled ?? thinkingEnabled,
    };
    if (partial?.deepseek_spent_sync !== undefined) {
      const v = partial.deepseek_spent_sync;
      body.deepseek_spent_sync = v == null || !Number.isFinite(v) ? -1 : v;
    } else if (opts?.includeSpentSync) {
      const syncRaw = deepseekSpentSync.trim();
      if (syncRaw === "") body.deepseek_spent_sync = -1;
      else {
        const n = Number(syncRaw);
        body.deepseek_spent_sync = Number.isFinite(n) ? n : -1;
      }
    }
    return api.putAiSettings(body);
  }

  async function saveSettings() {
    setBusy(true);
    try {
      const s = await persistSettings(undefined, { includeSpentSync: true });
      setSettings(s);
      setProvider((s.provider || provider) as AiProvider);
      setModel(s.model || defaultModelFor(s.provider || provider));
      setApiKey(s.api_key_masked || "");
      setContextWindow(s.context_window ?? 0);
      setThinkingEnabled(!!s.thinking_enabled);
      setDeepseekSpentSync(
        s.deepseek_spent_sync != null && Number.isFinite(s.deepseek_spent_sync)
          ? String(s.deepseek_spent_sync)
          : "",
      );
      tips.success("AI 设置已保存");
      if (s.has_api_key) {
        await loadModels(true);
        await loadUsage();
        setShowSettings(false);
      }
    } catch (e) {
      tips.error(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  async function onProviderChange(next: string) {
    const p = next === "deepseek" ? "deepseek" : "gemini";
    setProvider(p);
    setModels(fallbackModels(p));
    setModel(defaultModelFor(p));
    setApiKey("");
    if (p === "deepseek") {
      setAttachments([]);
      setPreviewId(null);
    }
    setBusy(true);
    try {
      const s = await persistSettings({
        provider: p,
        api_key: "",
        model: defaultModelFor(p),
      });
      setSettings(s);
      setProvider((s.provider || p) as AiProvider);
      setModel(s.model || defaultModelFor(p));
      setApiKey(s.api_key_masked || "");
      if (s.has_api_key) await loadModels(true, p);
      else setShowSettings(true);
      await loadUsage();
      tips.success(p === "deepseek" ? "已切换到 DeepSeek" : "已切换到 Gemini");
    } catch (e) {
      tips.error(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  async function onModelChange(next: string) {
    setModel(next);
    try {
      const s = await persistSettings({ model: next });
      setSettings(s);
    } catch {
      /* 选择仍保留本地，保存失败不打断 */
    }
  }

  async function onContextWindowChange(next: number, silent = false) {
    const max = models.find((m) => m.id === model)?.input_token_limit || 0;
    if (next > 0 && max > 0 && next > max) {
      if (!silent) tips.error(`当前模型上限为 ${formatTokens(max)}，无法选择该档位`);
      return;
    }
    setContextWindow(next);
    try {
      const s = await persistSettings({ context_window: next });
      setSettings(s);
      setContextWindow(s.context_window ?? next);
      if (!silent) tips.success("上下文窗口已更新");
    } catch (e) {
      tips.error(String((e as Error).message || e));
    }
  }

  // 换模型 / 拉取模型后：若已选档位超出当前 Key 模型上限，回退到「跟随模型」
  useEffect(() => {
    const max = models.find((m) => m.id === model)?.input_token_limit || 0;
    if (!max || contextWindow <= 0) return;
    if (contextWindow > max) {
      void onContextWindowChange(0, true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [model, models]);

  async function newChat() {
    // 只清空当前窗口；真正落库等到首条消息发出（避免空「新对话」进历史）
    setChat(null);
    persistChatId(null);
    setPrompt("");
    setAttachments([]);
    setPreviewId(null);
    setShowHistory(false);
    setShowSettings(false);
    tips.success("已开始新对话");
  }

  async function addFiles(files: FileList | File[], opts?: { fromPaste?: boolean }) {
    if (provider === "deepseek") {
      tips.info("DeepSeek 暂不支持图片附件，请改用 Gemini 或仅用文字描述");
      return;
    }
    const raw = Array.from(files).filter(Boolean);
    if (!raw.length) return;

    const supported: File[] = [];
    let skippedType = 0;
    raw.forEach((f, i) => {
      const file = opts?.fromPaste ? normalizePasteFile(f, i) : f;
      const byExt = /\.(jpe?g|png|webp|gif)$/i.test(file.name);
      const ok = mimeAllowed(file.type) || byExt;
      if (!ok) {
        skippedType += 1;
        return;
      }
      supported.push(file);
    });
    if (!supported.length) {
      tips.error(
        skippedType
          ? opts?.fromPaste
            ? "仅支持粘贴图片（JPG/PNG/WebP/GIF）"
            : "仅支持上传图片（JPG/PNG/WebP/GIF）"
          : "没有可添加的附件",
      );
      return;
    }

    try {
      const parsed: LocalAttach[] = [];
      for (const f of supported) {
        parsed.push(await readFileAsAttachment(f));
      }
      const prev = attachmentsRef.current;
      const room = MAX_ATTACH - prev.length;
      if (room <= 0) {
        tips.error(`最多 ${MAX_ATTACH} 个附件`);
        return;
      }
      const take = parsed.slice(0, room);
      const next = [...prev, ...take];
      attachmentsRef.current = next;
      setAttachments(next);
      const truncated = parsed.length > room;
      if (truncated || skippedType) {
        tips.info(
          [
            `已添加 ${take.length} 个附件`,
            truncated ? `（上限 ${MAX_ATTACH}）` : "",
            skippedType ? `，跳过 ${skippedType} 个不支持类型` : "",
          ].join(""),
        );
      } else if (opts?.fromPaste) {
        tips.info(`已粘贴 ${take.length} 个附件`);
      }
    } catch (e) {
      tips.error(String((e as Error).message || e));
    }
  }

  function onComposerPaste(e: ClipboardEvent) {
    if (busy) return;
    const files = filesFromClipboard(e.clipboardData);
    if (!files.length) return;
    e.preventDefault();
    const text = e.clipboardData.getData("text");
    if (text) {
      pinComposerCaretRef.current = true;
      setPrompt((prev) => (prev ? `${prev}${text}` : text));
    }
    void addFiles(files, { fromPaste: true });
  }

  function onComposerDragOver(e: DragEvent) {
    if (busy) return;
    if (![...e.dataTransfer.types].includes("Files")) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    setDragOver(true);
  }

  function onComposerDragLeave(e: DragEvent) {
    if (!e.currentTarget.contains(e.relatedTarget as Node)) {
      setDragOver(false);
    }
  }

  function onComposerDrop(e: DragEvent) {
    e.preventDefault();
    setDragOver(false);
    if (busy) return;
    const files = e.dataTransfer.files;
    if (files?.length) void addFiles(files);
  }

  function httpsConsoleHint(): string {
    const { protocol, hostname } = window.location;
    if (protocol === "https:") return "";
    // 默认 TLS = HTTP 端口 + 1（8787 → 8788）
    const httpPort = window.location.port || (protocol === "http:" ? "80" : "");
    const n = httpPort ? Number(httpPort) : 8787;
    const tlsPort = Number.isFinite(n) && n > 0 ? n + 1 : 8788;
    return `https://${hostname}:${tlsPort}/`;
  }

  function speechErrorMessage(code: string): string {
    if (!window.isSecureContext || code === "service-not-allowed") {
      const httpsUrl = httpsConsoleHint();
      return httpsUrl
        ? `浏览器规定：局域网 HTTP 不能用麦克风。请打开 ${httpsUrl} ，首次信任自签证书后再点语音。`
        : "当前页面不是安全上下文，无法使用语音识别。请改用 HTTPS。";
    }
    if (code === "not-allowed") {
      return "麦克风权限被拒绝。请在浏览器地址栏左侧站点设置中允许麦克风，然后重试。";
    }
    if (code === "audio-capture") {
      return "未检测到麦克风，或麦克风被占用";
    }
    if (code === "unsupported") {
      return "当前浏览器不支持实时语音识别，请使用 Chrome / Edge（HTTPS）";
    }
    if (code === "network") {
      return "浏览器语音识别需要联网，请检查网络后重试";
    }
    return `语音识别失败（${code}）`;
  }

  function ensureSpeech(): SpeechDictation {
    if (!speechRef.current) {
      speechRef.current = new SpeechDictation({
        onText: (text) => {
          pinComposerCaretRef.current = true;
          setPrompt(text);
        },
        onListeningChange: (on) => setListening(on),
        onFatalError: (code) => tips.error(speechErrorMessage(code)),
      });
    }
    return speechRef.current;
  }

  function toggleSpeech() {
    const session = ensureSpeech();
    if (session.isArmed || listening) {
      session.stop();
      tips.clear();
      return;
    }
    if (!speechDictationSupported()) {
      tips.error(speechErrorMessage(window.isSecureContext ? "unsupported" : "service-not-allowed"));
      return;
    }
    session.start(promptRef.current);
  }

  useEffect(() => {
    return () => {
      speechRef.current?.dispose();
      speechRef.current = null;
    };
  }, []);

  async function runAgent(text: string, choiceId?: string | null, att?: AiAttachment[]) {
    if (listening) {
      tips.info("请先结束语音输入");
      return;
    }
    const displayUser = text.trim() || (att?.length ? "[附件]" : "");
    setBusy(true);
    setPendingUser(displayUser || "…");
    setPrompt("");
    setAttachments([]);
    setLiveStatus(thinkingEnabled ? "正在思考…" : "正在查阅配置…");
    setLiveThinking("");
    setLiveSteps([]);
    setThinkingOpen(true);
    setLiveElapsed(0);
    thinkingBufRef.current = "";
    if (thinkingRafRef.current) {
      cancelAnimationFrame(thinkingRafRef.current);
      thinkingRafRef.current = 0;
    }
    const startedAt = Date.now();
    const elapsedTimer = window.setInterval(() => {
      setLiveElapsed(Math.floor((Date.now() - startedAt) / 1000));
    }, 500);
    try {
      if (chatId && chat?.archived) {
        await api.patchAiChat(chatId, { archived: false });
      }
      await persistSettings();
      const current = models.find((m) => m.id === model);
      const p = await api.aiPlanStream(
        text,
        current?.input_token_limit || 0,
        chatId,
        att,
        choiceId,
        (ev) => {
          if (ev.type === "status") setLiveStatus(ev.text);
          if (ev.type === "thinking") {
            thinkingBufRef.current += ev.delta;
            if (!thinkingRafRef.current) {
              thinkingRafRef.current = requestAnimationFrame(() => {
                thinkingRafRef.current = 0;
                setLiveThinking(thinkingBufRef.current);
              });
            }
          }
          if (ev.type === "step" && ev.text) {
            setLiveSteps((prev) => [...prev, ev.text]);
          }
        },
      );
      if (thinkingRafRef.current) {
        cancelAnimationFrame(thinkingRafRef.current);
        thinkingRafRef.current = 0;
      }
      setLiveThinking(thinkingBufRef.current);
      if (p.usage) setUsage(p.usage);
      if (p.chat_id) {
        persistChatId(p.chat_id);
        await openChat(p.chat_id);
      }
      await refreshChatList();
      const kind = (p.kind || "plan").toLowerCase();
      if (kind === "ask") tips.info("请选择一个选项继续");
      else if (kind === "advice") tips.info("助手给出了纠错建议");
      else if (kind === "error") tips.error(p.summary || "无法处理该需求");
      else if (p.ops.length) tips.success("方案已生成，请确认后应用");
      else tips.success("无需变更");
    } catch (e) {
      const msg = String((e as Error).message || e);
      tips.error(msg);
      void loadUsage();
    } finally {
      window.clearInterval(elapsedTimer);
      if (thinkingRafRef.current) {
        cancelAnimationFrame(thinkingRafRef.current);
        thinkingRafRef.current = 0;
      }
      setBusy(false);
      setPendingUser(null);
      setLiveStatus("");
      setLiveThinking("");
      setLiveSteps([]);
      setLiveElapsed(0);
      thinkingBufRef.current = "";
    }
  }

  async function generatePlan() {
    if (!prompt.trim() && !attachments.length) return;
    const text = prompt;
    const att = attachments.map(({ mime_type, data_base64, name }) => ({
      mime_type,
      data_base64,
      name,
    }));
    await runAgent(text, null, att);
  }

  async function chooseOption(choiceId: string, label: string) {
    if (!choiceId || busy) return;
    await runAgent(label, choiceId, []);
  }

  // 额度耗尽倒计时（本地递减，到 0 再拉一次 usage）
  useEffect(() => {
    if (!usage?.quota_exhausted) return;
    const t = window.setInterval(() => {
      setUsage((prev) => {
        if (!prev?.quota_exhausted) return prev;
        const left = Math.max(0, (prev.quota_retry_after_secs || 0) - 1);
        if (left <= 0) {
          void loadUsage();
          return {
            ...prev,
            quota_exhausted: false,
            quota_blocked_model: "",
            quota_retry_after_secs: 0,
          };
        }
        return {
          ...prev,
          quota_retry_after_secs: left,
          quota_note: `模型 ${prev.quota_blocked_model} 额度暂时用尽，约 ${left} 秒后可重试；也可换 flash 模型`,
        };
      });
    }, 1000);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [usage?.quota_exhausted, usage?.quota_blocked_model]);

  async function applyOps(ops: AiOp[], messageId: string) {
    if (!ops.length || !messageId) return;
    setBusy(true);
    try {
      const r = await api.aiApply(ops, chatId, messageId);
      if (r.chat) {
        setChat(r.chat);
        persistChatId(r.chat.id);
      } else if (chatId) {
        await openChat(chatId);
      }
      const path = pathForAiOps(ops);
      navigate(path);
      // 等目标页挂上监听后再刷，避免仍停在原页时数据不更新
      window.setTimeout(() => requestConfigReload(), 0);
      tips.success(r.message || "已应用");
    } catch (e) {
      tips.error(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  async function archiveChat(id: string, archived: boolean) {
    setBusy(true);
    try {
      await api.patchAiChat(id, { archived });
      if (chatId === id && archived) {
        setChat(null);
        persistChatId(null);
      } else if (chatId === id) {
        await openChat(id);
      }
      await refreshChatList(showArchived);
      tips.success(archived ? "已归档" : "已取消归档");
    } catch (e) {
      tips.error(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  async function deleteChat(id: string) {
    const ok = await confirm({
      title: "删除对话",
      message: "删除后不可恢复，确定继续？",
      confirmLabel: "删除",
      danger: true,
    });
    if (!ok) return;
    setBusy(true);
    try {
      await api.deleteAiChat(id);
      if (chatId === id) {
        setChat(null);
        persistChatId(null);
      }
      await refreshChatList(showArchived);
      tips.success("已删除");
    } catch (e) {
      tips.error(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  async function branchFrom(messageId: string) {
    if (!chatId) return;
    setBusy(true);
    try {
      const c = await api.branchAiChat(chatId, messageId);
      setChat(c);
      persistChatId(c.id);
      setShowHistory(false);
      // 分支不进入历史主列表，仅作为当前窗口的派生会话
      tips.success("已在当前窗口从此处继续（不会新增历史条目）");
    } catch (e) {
      tips.error(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  const modelOptions = models.some((m) => m.id === model)
    ? models
    : [{ id: model, display_name: model }, ...models];

  const currentModel = models.find((m) => m.id === model);
  const modelContextMax = currentModel?.input_token_limit || 0;
  /** ListModels（当前 Key）已返回该模型的 inputTokenLimit */
  const modelContextKnown = Boolean(currentModel && modelContextMax > 0);
  const allContextChoices =
    settings?.context_window_choices?.length
      ? settings.context_window_choices
      : DEFAULT_CONTEXT_CHOICES;
  const contextChoices = contextChoicesForModel(
    allContextChoices,
    modelContextMax,
    modelContextKnown,
  );
  // 上下文跟随当前对话（对齐 Gemini promptTokenCount）；新对话为 0
  const chatContextUsed = chat?.last_prompt_tokens || chat?.last_total_tokens || 0;
  const contextLimit = effectiveContextLimit(
    contextWindow,
    modelContextMax || undefined,
    chat?.context_limit || usage?.context_limit,
  );
  const contextUsed = chat ? chatContextUsed : 0;
  const contextPct =
    contextLimit > 0 ? Math.round((contextUsed / contextLimit) * 1000) / 10 : 0;
  const usageProvider = usage?.provider || provider;
  const rpdHint = usage?.quota_rpd_hint ?? (usageProvider === "deepseek" ? 0 : 1500);
  const reqToday = usage?.requests_today || 0;
  const balanceText = formatBalance(usage);
  const spentText = formatMoney(usage?.balance_spent, usage?.balance_currency);
  const quotaTotalText = formatMoney(usage?.balance_quota_total, usage?.balance_currency);
  const toppedText = formatMoney(usage?.balance_topped_up, usage?.balance_currency);
  const balanceEmpty = usageProvider === "deepseek" && usage?.balance_available === false;
  const quotaExhausted = Boolean(
    balanceEmpty || (usage?.quota_exhausted && modelQuotaBlocked(usage, model)),
  );
  const deepseekQuotaPct =
    usageProvider === "deepseek" && usage?.balance_quota_pct != null
      ? Math.max(0, Math.min(100, usage.balance_quota_pct))
      : null;
  const quotaPct = quotaExhausted
    ? 100
    : deepseekQuotaPct != null
      ? deepseekQuotaPct
      : rpdHint > 0
        ? Math.min(100, (reqToday / rpdHint) * 100)
        : 0;

  const messages = chat?.messages || [];
  const pending = lastAssistantWithOps(messages);
  const empty = !messages.length && !showSettings && !showHistory;

  const contextLine =
    contextUsed > 0
      ? `${formatTokens(contextUsed)} / ${formatTokens(contextLimit)} · ${contextPct}%`
      : `本对话 ${formatTokens(0)} / ${formatTokens(contextLimit)}`;
  const retrySecs = usage?.quota_retry_after_secs || 0;
  const quotaLine = balanceEmpty
    ? `余额不足${balanceText ? ` · ${balanceText}` : ""}`
    : quotaExhausted
      ? `额度已用尽 · ${retrySecs > 0 ? `${retrySecs}s 后可重试` : "请换模型"}`
      : usageProvider === "deepseek"
        ? spentText && quotaTotalText
          ? `已消费 ${spentText} / ${quotaTotalText}`
          : balanceText
            ? `余额 ${balanceText}`
            : `今日 ${reqToday} 次 · DeepSeek`
        : `${reqToday} / ~${rpdHint || 1500} 次 · 今日`;
  const chatIn = chat?.last_prompt_tokens || 0;
  const chatOut = chat?.last_output_tokens || 0;
  const tokensLine = usage
    ? usage.quota_note ||
      (usageProvider === "deepseek" && spentText && quotaTotalText
        ? `已消费 ${spentText} / 总额 ${quotaTotalText}${toppedText ? ` · 充值余额 ${toppedText}` : ""}`
        : `今日 ${formatTokens(usage.tokens_today)} · 本轮 in ${formatTokens(chatIn)} / out ${formatTokens(chatOut)}`)
    : "上下文随对话；用量按当前供应商分别累计";
  const exhaustedHint = balanceEmpty
    ? `DeepSeek 余额不足${balanceText ? `（${balanceText}）` : ""}，请到 platform.deepseek.com 充值`
    : quotaExhausted && usage?.quota_blocked_model
      ? `${usage.quota_blocked_model} 额度已用尽${retrySecs > 0 ? `，约 ${retrySecs}s 后可重试` : ""}；可换 ${
          usageProvider === "deepseek" ? "deepseek-v4-flash" : "gemini-2.0-flash"
        }`
      : undefined;

  return (
    <aside className="flex h-full min-h-0 flex-col overflow-hidden bg-surface">
      <div className="flex shrink-0 items-center justify-between gap-2 px-4 py-2.5">
        <div className="min-w-0">
          <h2 className="text-[15px] font-semibold tracking-tight text-fg">AI 助手</h2>
          <p className="truncate text-[11px] text-fg-soft">
            {chat?.title || "自然语言改配置"}
            {chat?.archived ? " · 已归档" : ""}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-0.5">
          <IconBtn title="新对话" disabled={busy} onClick={() => void newChat()}>
            <IconPlus />
          </IconBtn>
          <IconBtn
            title={showHistory ? "关闭历史" : "对话历史"}
            disabled={busy}
            active={showHistory}
            onClick={() => {
              setShowHistory((v) => !v);
              if (!showHistory) void refreshChatList(showArchived);
            }}
          >
            <IconHistory />
          </IconBtn>
          <IconBtn
            title={showSettings ? "关闭设置" : "设置"}
            disabled={busy}
            active={showSettings}
            onClick={() => setShowSettings((v) => !v)}
          >
            <IconSettings />
          </IconBtn>
        </div>
      </div>

      <div
        ref={scrollRef}
        className="min-h-0 flex-1 space-y-3 overflow-y-auto border-t border-edge px-4 py-3"
        onScroll={(e) => {
          const el = e.currentTarget;
          const dist = el.scrollHeight - el.scrollTop - el.clientHeight;
          // 生成中若用户上翻阅读则暂停跟滚；回到底部附近再恢复
          stickScrollRef.current = dist < 80;
        }}
      >
        {!showHistory && !showSettings && chat?.parent_id ? (
          <div className="flex items-center justify-between gap-2 rounded-lg bg-composer px-3 py-2 text-[11px] text-fg-soft">
            <span>当前为派生会话，不会出现在历史主列表</span>
            <button
              type="button"
              className="shrink-0 text-accent underline-offset-2 hover:underline disabled:opacity-40"
              disabled={busy}
              onClick={() => void openChat(chat.parent_id!)}
            >
              返回主对话
            </button>
          </div>
        ) : null}
        {showSettings && (
          <div className="space-y-3 rounded-xl border border-edge bg-composer p-3">
            <div>
              <Label>供应商</Label>
              <Select
                value={provider === "deepseek" ? "deepseek" : "gemini"}
                disabled={busy}
                onChange={(e) => void onProviderChange(e.target.value)}
              >
                <option value="gemini">Google Gemini</option>
                <option value="deepseek">DeepSeek</option>
              </Select>
            </div>
            <p className="text-xs leading-relaxed text-fg-soft">
              {provider === "deepseek" ? (
                <>
                  到{" "}
                  <a
                    className="text-accent underline"
                    href="https://platform.deepseek.com/api_keys"
                    target="_blank"
                    rel="noreferrer"
                  >
                    platform.deepseek.com/api_keys
                  </a>{" "}
                  创建 Key（按量计费）。也可设{" "}
                  <code className="text-accent-fg">OMNI_DEEPSEEK_API_KEY</code>。文档见{" "}
                  <a
                    className="text-accent underline"
                    href="https://api-docs.deepseek.com/zh-cn/"
                    target="_blank"
                    rel="noreferrer"
                  >
                    DeepSeek API
                  </a>
                  。
                </>
              ) : (
                <>
                  到{" "}
                  <a
                    className="text-accent underline"
                    href="https://aistudio.google.com/apikey"
                    target="_blank"
                    rel="noreferrer"
                  >
                    aistudio.google.com/apikey
                  </a>{" "}
                  创建免费 Key。也可设{" "}
                  <code className="text-accent-fg">OMNI_GEMINI_API_KEY</code>。
                </>
              )}
            </p>
            <div>
              <Label>API Key{settings?.has_api_key ? `（已配置 · ${settings.source}）` : ""}</Label>
              <Input
                type="password"
                value={apiKey}
                placeholder={provider === "deepseek" ? "sk-..." : "AIza..."}
                onChange={(e) => setApiKey(e.target.value)}
                onFocus={() => {
                  if (settings?.api_key_masked && apiKey === settings.api_key_masked) {
                    setApiKey("");
                  }
                }}
              />
            </div>
            <div>
              <Label>上下文窗口</Label>
              <Select
                value={String(
                  contextChoices.includes(contextWindow) ? contextWindow : 0,
                )}
                disabled={busy || !settings?.has_api_key || !modelContextKnown}
                onChange={(e) => void onContextWindowChange(Number(e.target.value) || 0)}
              >
                {contextChoices.map((n) => (
                  <option key={n} value={n}>
                    {formatContextWindowOption(n, modelContextMax || undefined)}
                  </option>
                ))}
              </Select>
              <p className="mt-1.5 text-[11px] leading-snug text-faint">
                {!settings?.has_api_key
                  ? "请先配置 API Key 并刷新模型，再按模型上限选择档位。"
                  : !modelContextKnown
                    ? "正在获取当前模型上限…请点击「刷新模型」。"
                    : provider === "deepseek"
                      ? `当前模型上限 ${formatTokens(modelContextMax)}（DeepSeek 文档：1M 上下文）。`
                      : `当前模型上限 ${formatTokens(modelContextMax)}（由 Key 的 ListModels 返回）；仅可选择不超过该上限的档位。`}
              </p>
            </div>
            <label className="flex cursor-pointer items-start gap-2.5 rounded-lg px-0.5 py-1">
              <input
                type="checkbox"
                className="mt-0.5"
                checked={thinkingEnabled}
                disabled={busy}
                onChange={(e) => {
                  const next = e.target.checked;
                  setThinkingEnabled(next);
                  void persistSettings({ thinking_enabled: next })
                    .then((s) => {
                      setSettings(s);
                      setThinkingEnabled(!!s.thinking_enabled);
                    })
                    .catch((err) => tips.error(String((err as Error).message || err)));
                }}
              />
              <span>
                <span className="text-sm text-fg">思考模式</span>
                <span className="mt-0.5 block text-[11px] leading-snug text-faint">
                  {provider === "deepseek"
                    ? "开启后 DeepSeek 流式输出 reasoning（更慢、更贵）；工具调用时会回传思考内容。"
                    : "开启后对支持的 Gemini（如 2.5）启用 thinkingConfig 并流式展示思考摘要；2.0 等旧模型会自动跳过。"}
                </span>
              </span>
            </label>
            {provider === "deepseek" && (
              <div>
                <Label>累计消费对齐（元）</Label>
                <Input
                  type="number"
                  inputMode="decimal"
                  step="0.01"
                  min="0"
                  placeholder="例如 0.18"
                  value={deepseekSpentSync}
                  disabled={busy}
                  onChange={(e) => setDeepseekSpentSync(e.target.value)}
                />
                <p className="mt-1.5 text-[11px] leading-snug text-faint">
                  官方余额接口不含「累计消费金额」。可填 platform 用量页上的数值以对齐额度环；留空则仅按余额下降与本机
                  token 费用估算。
                </p>
              </div>
            )}
            <div className="flex flex-wrap gap-2">
              <Button
                variant="ghost"
                onClick={() => loadModels(false)}
                disabled={busy || loadingModels || !settings?.has_api_key}
              >
                {loadingModels ? "拉取中…" : "刷新模型"}
              </Button>
              <Button onClick={saveSettings} disabled={busy}>
                保存
              </Button>
            </div>
          </div>
        )}

        {showHistory && (
          <div>
            <div className="mb-1 flex items-center justify-between gap-2 px-1">
              <p className="text-[11px] text-faint">
                {chats.length ? `${chats.length} 条主对话` : "对话历史"}
              </p>
              <label className="flex cursor-pointer items-center gap-1.5 text-[11px] text-faint">
                <input
                  type="checkbox"
                  className="accent-accent"
                  checked={showArchived}
                  onChange={(e) => {
                    const v = e.target.checked;
                    setShowArchived(v);
                    void refreshChatList(v);
                  }}
                />
                含归档
              </label>
            </div>
            {!chats.length ? (
              <p className="px-1 py-6 text-center text-xs text-faint">暂无对话</p>
            ) : (
              <ul className="divide-y divide-edge/60">
                {chats.map((c) => {
                  const active = c.id === chatId;
                  const meta = [
                    `${c.message_count} 条`,
                    formatChatUpdatedAt(c.updated_at),
                    c.archived ? "归档" : "",
                  ]
                    .filter(Boolean)
                    .join(" · ");
                  return (
                    <li
                      key={c.id}
                      className={[
                        "flex items-center gap-1 rounded-lg px-1.5 py-2 transition",
                        active ? "bg-accent-soft" : "hover:bg-hover/70",
                      ].join(" ")}
                    >
                      <button
                        type="button"
                        className="min-w-0 flex-1 py-0.5 text-left"
                        disabled={busy}
                        onClick={() => {
                          void openChat(c.id);
                          setShowHistory(false);
                        }}
                      >
                        <div
                          className={[
                            "truncate text-[13px] leading-snug",
                            active ? "font-medium text-accent-fg" : "text-fg",
                          ].join(" ")}
                        >
                          {c.title || "未命名对话"}
                        </div>
                        <div className="mt-0.5 truncate text-[11px] leading-snug text-faint">
                          {meta}
                        </div>
                      </button>
                      <div className="flex shrink-0 items-center">
                        <IconBtn
                          dense
                          title={c.archived ? "取消归档" : "归档"}
                          disabled={busy}
                          onClick={() => void archiveChat(c.id, !c.archived)}
                        >
                          <IconArchive />
                        </IconBtn>
                        <IconBtn
                          dense
                          title="删除"
                          disabled={busy}
                          onClick={() => void deleteChat(c.id)}
                        >
                          <IconTrash />
                        </IconBtn>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        )}

        {!showHistory && empty && (
          <div className="rounded-xl bg-composer px-4 py-5 text-left">
            <p className="text-sm font-medium text-fg">描述需求，助手会查阅配置后再给方案或提问</p>
            <p className="mt-1 text-[11px] text-faint">
              支持策略组、规则集、局域网分流、落地代理 · 点击示例填入
            </p>
            <ul className="mt-3 space-y-2 text-xs leading-relaxed text-fg-soft">
              {[
                {
                  label: "策略组 + 规则集",
                  text: "为 TradingView 添加策略组与规则集，策略组按香港->台湾->日本->韩国顺序，规则集匹配 tradingview.com 域名。",
                },
                {
                  label: "局域网分流",
                  text: "给 172.16.1.50 加局域网分流，走奈飞组。",
                },
                {
                  label: "落地代理",
                  text: "新增 SOCKS5 落地代理 127.0.0.1:1080，名称家宽落地，前置走香港。",
                },
              ].map((ex) => (
                <li key={ex.label}>
                  <button
                    type="button"
                    disabled={busy}
                    className="w-full rounded-lg px-2.5 py-2 text-left transition hover:bg-hover disabled:opacity-40"
                    onClick={() => {
                      setPrompt(ex.text);
                      pinComposerCaretRef.current = true;
                    }}
                  >
                    <span className="font-medium text-fg">{ex.label}</span>
                    <br />
                    <span className="text-fg-soft">{ex.text}</span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}

        {!showHistory &&
          messages.map((m) => {
            const kind = messageKind(m);
            const isAsk = kind === "ask";
            const isAdvice = kind === "advice";
            const isError = kind === "error";
            const isPlan = kind === "plan";
            const askActive = isAsk && lastAskPending(messages)?.id === m.id;
            return (
              <div
                key={m.id}
                className={
                  m.role === "user"
                    ? "ml-6 rounded-xl bg-composer px-3 py-2.5 text-sm text-fg"
                    : "mr-2 space-y-2 rounded-xl border border-preview-border bg-preview p-3"
                }
              >
                {m.role === "user" ? (
                  <p className="whitespace-pre-wrap">{m.content}</p>
                ) : (
                  <>
                    {m.thinking?.trim() ? (
                      <details
                        className="rounded-lg bg-input/60 px-2.5 py-1.5"
                        onToggle={(e) => {
                          if (!(e.currentTarget as HTMLDetailsElement).open) return;
                          stickScrollRef.current = true;
                          const node = e.currentTarget;
                          requestAnimationFrame(() => {
                            scrollChatToBottom();
                            node.scrollIntoView({ block: "nearest", behavior: "smooth" });
                          });
                        }}
                      >
                        <summary className="cursor-pointer text-[11px] text-muted">
                          思考过程
                        </summary>
                        <pre className="mt-1.5 max-h-40 overflow-auto whitespace-pre-wrap text-[11px] leading-relaxed text-faint">
                          {m.thinking}
                        </pre>
                      </details>
                    ) : null}
                    {m.steps?.length ? (
                      <ul className="space-y-0.5 text-[11px] text-muted">
                        {m.steps.map((s, i) => (
                          <li key={i}>· {s}</li>
                        ))}
                      </ul>
                    ) : null}
                    <p
                      className={
                        isError
                          ? "text-sm text-danger"
                          : isAdvice
                            ? "text-sm text-fg-soft"
                            : "text-sm text-fg"
                      }
                    >
                      {m.summary ||
                        (isAsk
                          ? "请选择"
                          : isAdvice
                            ? "建议如下"
                            : isError
                              ? "无法处理"
                              : "已生成方案")}
                    </p>
                    {isAsk && m.options?.length ? (
                      <div className="flex flex-wrap gap-1.5 pt-0.5">
                        {m.options.map((opt) => (
                          <Button
                            key={opt.id}
                            className="px-3!"
                            disabled={busy || !askActive}
                            onClick={() => void chooseOption(opt.id, opt.label)}
                          >
                            {opt.label}
                          </Button>
                        ))}
                      </div>
                    ) : null}
                    {isPlan && m.preview?.length ? (
                      <ul className="list-disc space-y-1 pl-4 text-xs text-fg-soft">
                        {m.preview.map((line, i) => (
                          <li key={i}>{line}</li>
                        ))}
                      </ul>
                    ) : null}
                    {isPlan && m.ops?.length ? (
                      <details>
                        <summary className="cursor-pointer text-[11px] text-muted">
                          原始 ops JSON
                        </summary>
                        <pre className="mt-2 max-h-28 overflow-auto rounded-lg bg-input p-2 text-[11px] text-muted">
                          {JSON.stringify(m.ops as AiOp[], null, 2)}
                        </pre>
                      </details>
                    ) : null}
                    <div className="flex flex-wrap gap-1.5 pt-0.5">
                      {isPlan && m.ops?.length && m.applied_at ? (
                        <Button className="px-3!" disabled>
                          已应用
                        </Button>
                      ) : isPlan && m.ops?.length && pending?.id === m.id ? (
                        <Button
                          className="px-3!"
                          onClick={() => void applyOps(m.ops || [], m.id)}
                          disabled={busy}
                        >
                          应用变更
                        </Button>
                      ) : null}
                      <button
                        type="button"
                        disabled={busy}
                        className="rounded-lg px-2 py-1 text-[11px] text-fg-soft transition hover:bg-hover hover:text-fg disabled:opacity-40"
                        onClick={() => void branchFrom(m.id)}
                      >
                        从此处继续
                      </button>
                    </div>
                  </>
                )}
              </div>
            );
          })}

        {!showHistory && pendingUser ? (
          <div className="ml-6 rounded-xl bg-composer px-3 py-2.5 text-sm text-fg">
            <p className="whitespace-pre-wrap">{pendingUser}</p>
          </div>
        ) : null}

        {!showHistory && busy ? (
          <div className="mr-2 space-y-2 rounded-xl border border-accent/30 bg-preview p-3 shadow-sm">
            <div className="flex items-center gap-2 text-[12px] text-fg">
              <span className="inline-block h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-accent" />
              <span className="min-w-0 flex-1 truncate font-medium">
                {liveStatus || (thinkingEnabled ? "正在思考…" : "正在生成…")}
              </span>
              <span className="shrink-0 tabular-nums text-[11px] text-muted">
                {liveElapsed}s
              </span>
            </div>
            {(liveThinking || thinkingEnabled) && (
              <div className="rounded-lg bg-input/70 px-2.5 py-2">
                <button
                  type="button"
                  className="text-[11px] text-fg-soft underline-offset-2 hover:underline"
                  onClick={() => {
                    setThinkingOpen((v) => {
                      const next = !v;
                      if (next) {
                        stickScrollRef.current = true;
                        requestAnimationFrame(() => {
                          scrollChatToBottom();
                          scrollLiveThinkingToBottom();
                        });
                      }
                      return next;
                    });
                  }}
                >
                  {thinkingOpen ? "收起思考过程" : "展开思考过程"}
                  {liveThinking ? ` · ${liveThinking.length} 字` : ""}
                </button>
                {thinkingOpen ? (
                  <pre
                    ref={liveThinkingPreRef}
                    className="mt-1.5 max-h-56 overflow-auto whitespace-pre-wrap text-[11px] leading-relaxed text-faint"
                  >
                    {liveThinking || (
                      <span className="text-muted">等待模型输出思考内容…</span>
                    )}
                    {liveThinking ? (
                      <span className="inline-block animate-pulse text-accent">▍</span>
                    ) : null}
                  </pre>
                ) : null}
              </div>
            )}
            {liveSteps.length ? (
              <ul className="space-y-1 border-t border-edge/60 pt-2 text-[11px] text-muted">
                {liveSteps.map((s, i) => (
                  <li key={`${s}-${i}`} className="flex gap-1.5">
                    <span className="text-accent">✓</span>
                    <span>{s}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-[11px] text-faint">
                {thinkingEnabled
                  ? "思考内容会逐字出现；随后可能查阅配置并给出方案。"
                  : "可在设置中开启「思考模式」以查看推理过程。"}
              </p>
            )}
          </div>
        ) : null}

      </div>

      <div className="shrink-0 border-t border-edge p-3">
        {chat?.archived ? (
          <div className="mb-2 flex items-center justify-between gap-2 rounded-xl bg-composer px-3 py-2 text-xs text-fg-soft">
            <span>此对话已归档，发送将先取消归档</span>
            <button
              type="button"
              className="text-accent"
              disabled={busy}
              onClick={() => chatId && void archiveChat(chatId, false)}
            >
              取消归档
            </button>
          </div>
        ) : null}
        <div
          className={[
            "rounded-2xl border bg-composer transition focus-within:border-accent",
            dragOver ? "border-accent ring-2 ring-accent-soft" : "border-edge",
          ].join(" ")}
          onDragOver={onComposerDragOver}
          onDragLeave={onComposerDragLeave}
          onDrop={onComposerDrop}
        >
          {attachments.length > 0 && (
            <div className="px-3.5 pt-2.5">
              <div className="flex gap-1.5 overflow-x-auto overscroll-x-contain pb-0.5">
                {attachments.map((a) => (
                  <div
                    key={a.id}
                    className="relative h-8 w-8 shrink-0 overflow-hidden rounded-md border border-edge bg-input"
                  >
                    <button
                      type="button"
                      className="h-full w-full"
                      title={a.name || "预览图片"}
                      disabled={busy}
                      onClick={() => setPreviewId(a.id)}
                    >
                      <img
                        src={attachDataUrl(a)}
                        alt={a.name || "附件"}
                        className="h-full w-full object-cover"
                      />
                    </button>
                    <button
                      type="button"
                      className="absolute -right-0.5 -top-0.5 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-black/55 text-[9px] leading-none text-white hover:bg-black/75"
                      disabled={busy}
                      aria-label="移除附件"
                      onClick={() => {
                        setAttachments((prev) => prev.filter((x) => x.id !== a.id));
                        if (previewId === a.id) setPreviewId(null);
                      }}
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}
          <Textarea
            ref={composerRef}
            rows={3}
            value={prompt}
            onChange={(e) => {
              const el = e.target;
              const atEnd = el.selectionStart === el.value.length && el.selectionEnd === el.value.length;
              setPrompt(el.value);
              if (atEnd) pinComposerCaretRef.current = true;
            }}
            onPaste={onComposerPaste}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void generatePlan();
              }
            }}
            placeholder={
              listening
                ? "正在听写…说完再点麦克风结束"
                : provider === "deepseek"
                  ? "描述配置变更，或点麦克风语音转文字…（DeepSeek 不支持图片 · Enter 发送）"
                  : "描述配置变更；可粘贴/拖入图片，或点麦克风语音转文字…（Enter 发送）"
            }
            className="max-h-40 resize-none overflow-y-auto border-0! bg-transparent! px-3.5 pb-1 pt-3 font-sans shadow-none! outline-none! ring-0! focus:border-transparent! focus:ring-0!"
            disabled={busy}
          />
          <input
            ref={fileInputRef}
            type="file"
            className="hidden"
            accept={ACCEPT_FILES}
            multiple
            onChange={(e) => {
              const files = e.target.files;
              if (files?.length) void addFiles(files);
              e.target.value = "";
            }}
          />
          <div className="flex items-center justify-between gap-2 px-2.5 pb-2.5">
            <SearchableSelect
              variant="pill"
              placement="top"
              value={model}
              disabled={busy || loadingModels}
              placeholder="搜索模型…"
              options={modelOptions.map((m) => {
                const blocked = modelQuotaBlocked(usage, m.id);
                return {
                  value: m.id,
                  label: m.display_name,
                  badge: blocked
                    ? "额度尽"
                    : m.tier_label ||
                      (m.tier === "free" ? "免费" : m.tier === "paid" ? "付费" : "未知"),
                  badgeTone: blocked ? "danger" : modelBadgeTone(m.tier),
                };
              })}
              onChange={onModelChange}
              footer={
                quotaExhausted
                  ? provider === "deepseek"
                    ? "当前模型限流：可换 deepseek-v4-flash，或稍后再试"
                    : "当前模型额度已用尽：可换 gemini-2.0-flash / 2.5-flash，或等待冷却后重试"
                  : provider === "deepseek"
                    ? "DeepSeek 按量计费 · Flash 并发更高 · 暂不支持图片附件"
                    : "免费：免费 API Key 可用 · 付费：需开通计费后可用（据官方定价）"
              }
            />
            <div className="flex shrink-0 items-center gap-0.5">
              <UsageRings
                contextPct={contextUsed > 0 ? contextPct : 0}
                quotaPct={quotaPct}
                contextLine={contextLine}
                quotaLine={quotaLine}
                tokensLine={tokensLine}
                exhausted={quotaExhausted}
                exhaustedHint={exhaustedHint}
              />
              <IconBtn
                title={
                  provider === "deepseek"
                    ? "DeepSeek 暂不支持图片附件"
                    : `上传图片（最多 ${MAX_ATTACH} 张，可预览/粘贴/拖入）`
                }
                disabled={
                  busy ||
                  provider === "deepseek" ||
                  attachments.length >= MAX_ATTACH
                }
                onClick={() => fileInputRef.current?.click()}
              >
                <IconPaperclip />
              </IconBtn>
              {listening || (!prompt.trim() && !attachments.length) ? (
                <PrimaryCircleBtn
                  title={listening ? "结束听写" : "语音转文字"}
                  disabled={busy}
                  recording={listening}
                  onClick={() => toggleSpeech()}
                >
                  <IconMic />
                </PrimaryCircleBtn>
              ) : (
                <PrimaryCircleBtn title="发送" disabled={busy} onClick={() => void generatePlan()}>
                  {busy ? <span className="text-xs font-medium">…</span> : <IconSend />}
                </PrimaryCircleBtn>
              )}
            </div>
          </div>
        </div>
      </div>

      {previewId
        ? (() => {
            const images = attachments.filter((a) => isImageMime(a.mime_type));
            if (!images.some((a) => a.id === previewId)) return null;
            return (
              <ImagePreviewer
                images={images}
                currentId={previewId}
                onClose={() => setPreviewId(null)}
                onChangeId={setPreviewId}
              />
            );
          })()
        : null}
    </aside>
  );
}

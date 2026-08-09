import { useState } from "react";
import { api } from "./api";
import { useTips } from "./tips";

/** 顶栏：打开本机 Nikki 控制面板（zashboard） */
export function NikkiPanelButton() {
  const tips = useTips();
  const [busy, setBusy] = useState(false);

  async function onClick() {
    if (busy) return;
    setBusy(true);
    try {
      const info = await api.getNikkiPanel();
      if (!info.ok) throw new Error(info.message || "无法获取面板地址");
      const host = window.location.hostname || "127.0.0.1";
      const port = info.port || 9090;
      const path = info.path?.startsWith("/") ? info.path : `/${info.path || "ui"}/`;
      const url = new URL(`http://${host}:${port}${path}`);
      // zashboard 支持 URL 参数预填连接信息
      url.searchParams.set("hostname", host);
      url.searchParams.set("port", String(port));
      url.searchParams.set("http", "true");
      if (info.secret) url.searchParams.set("secret", info.secret);
      window.open(url.toString(), "_blank", "noopener,noreferrer");
    } catch (e) {
      tips.error(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <button
      type="button"
      disabled={busy}
      onClick={() => void onClick()}
      title="打开 Nikki 控制面板"
      className="rounded-xl border border-edge bg-composer px-2.5 py-1 text-xs font-medium text-fg-soft transition hover:bg-hover hover:text-fg disabled:opacity-50"
    >
      {busy ? "打开中…" : "打开面板"}
    </button>
  );
}

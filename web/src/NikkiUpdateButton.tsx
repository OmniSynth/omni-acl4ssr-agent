import { useState } from "react";
import { api } from "./api";
import { useTips } from "./tips";

/** 顶栏：更新本机 Nikki 订阅并 reload */
export function NikkiUpdateButton() {
  const tips = useTips();
  const [busy, setBusy] = useState(false);

  async function onClick() {
    if (busy) return;
    setBusy(true);
    try {
      const r = await api.updateNikkiSubscription({ reload: true });
      if (!r.ok) throw new Error(r.message || "更新 Nikki 失败");
      tips.success(r.message || "Nikki 订阅已更新");
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
      title="拉取本机订阅并重载 Nikki"
      className="rounded-xl border border-edge bg-composer px-2.5 py-1 text-xs font-medium text-fg-soft transition hover:bg-hover hover:text-fg disabled:opacity-50"
    >
      {busy ? "更新 Nikki…" : "更新 Nikki"}
    </button>
  );
}

import { useEffect, useMemo, useState } from "react";
import { api, newId } from "../api";
import { CONFIG_RELOAD_EVENT } from "../configReload";
import type { DhcpClient, LanRoute } from "../types";
import { Button, Card, Input, Label, Msg, Select } from "../components/ui";
import { useTips } from "../tips";

const SPECIAL = ["DIRECT", "REJECT"];
const CUSTOM = "__custom__";

function formatDhcpOption(c: DhcpClient): string {
  const host = c.hostname?.trim() || "未知主机";
  const mac = c.mac?.trim() || "—";
  const tag = c.static_lease ? " · 静态" : "";
  return `${host} · ${c.ip} · ${mac}${tag}`;
}

function isDefaultName(name: string): boolean {
  return !name.trim() || /^设备\d+$/.test(name.trim());
}

function srcInDhcp(src: string, clients: DhcpClient[]): boolean {
  const s = src.trim();
  return clients.some((c) => c.ip === s);
}

/** 与后端 engine::normalize_src_cidr / build_rules 对齐的预览 */
function previewClashRule(src: string, target: string): string | null {
  const s = src.trim();
  const t = target.trim();
  if (!s || !t) return null;
  let cidr = s;
  if (!s.includes("/")) {
    if (s.includes(":")) cidr = `${s}/128`;
    else if (/^\d{1,3}(\.\d{1,3}){3}$/.test(s)) cidr = `${s}/32`;
    else return null;
  }
  return `SRC-IP-CIDR,${cidr},${t}`;
}

export default function LanRoutesPage() {
  const tips = useTips();
  const [routes, setRoutes] = useState<LanRoute[]>([]);
  const [targets, setTargets] = useState<string[]>([]);
  const [dhcp, setDhcp] = useState<DhcpClient[]>([]);
  const [dhcpError, setDhcpError] = useState("");
  const [loadingDhcp, setLoadingDhcp] = useState(false);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);

  async function loadDhcp(silent = false) {
    setLoadingDhcp(true);
    if (!silent) setDhcpError("");
    try {
      const list = await api.getDhcpClients();
      setDhcp(Array.isArray(list) ? list : []);
    } catch (e) {
      setDhcp([]);
      setDhcpError(String((e as Error).message || e));
    } finally {
      setLoadingDhcp(false);
    }
  }

  useEffect(() => {
    const load = () => {
      api
        .getConfig()
        .then((c) => {
          setRoutes(Array.isArray(c.lan_routes) ? c.lan_routes : []);
          const names = [
            ...c.groups.map((g) => g.name),
            ...(c.landings || []).filter((l) => l.enabled && l.name).map((l) => l.name),
            ...SPECIAL,
          ];
          setTargets([...new Set(names)]);
        })
        .catch((e) => setError(String(e.message || e)));
      void loadDhcp(true);
    };
    load();
    window.addEventListener(CONFIG_RELOAD_EVENT, load);
    return () => window.removeEventListener(CONFIG_RELOAD_EVENT, load);
  }, []);

  const dhcpByIp = useMemo(() => {
    const m = new Map<string, DhcpClient>();
    for (const c of dhcp) m.set(c.ip, c);
    return m;
  }, [dhcp]);

  function update(i: number, patch: Partial<LanRoute>) {
    setRoutes((prev) => prev.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  }

  function pickDevice(i: number, value: string) {
    if (value === CUSTOM) {
      const cur = routes[i];
      if (cur && srcInDhcp(cur.src, dhcp)) {
        update(i, { src: "" });
      }
      return;
    }
    const client = dhcpByIp.get(value);
    if (!client) {
      update(i, { src: value });
      return;
    }
    const cur = routes[i];
    const patch: Partial<LanRoute> = { src: client.ip };
    if (client.hostname && (!cur || isDefaultName(cur.name))) {
      patch.name = client.hostname;
    }
    update(i, patch);
  }

  function add() {
    setError("");
    const first = dhcp[0];
    setRoutes((prev) => [
      ...prev,
      {
        id: newId("lan"),
        name: first?.hostname || `设备${prev.length + 1}`,
        src: first?.ip || "",
        target: targets[0] || "🚀 默认",
        enabled: true,
      },
    ]);
    tips.info("已添加一条，填写后点击保存");
  }

  async function save() {
    setError("");
    for (const r of routes) {
      if (!r.src.trim()) {
        setError("请为每条规则选择源设备，或填写自定义 IP / CIDR");
        return;
      }
    }
    setSaving(true);
    try {
      const saved = await api.putLanRoutes(routes);
      setRoutes(saved);
      tips.success("局域网分流已保存；可点右上角「更新订阅」拉取并重载生效");
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card
      title="局域网分流"
      actions={
        <div className="flex gap-2">
          <Button variant="ghost" onClick={() => void loadDhcp(false)} disabled={loadingDhcp}>
            {loadingDhcp ? "刷新中…" : "刷新 DHCP"}
          </Button>
          <Button variant="ghost" onClick={add}>
            新增
          </Button>
          <Button onClick={() => void save()} disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </Button>
        </div>
      }
    >
      <p className="mb-4 text-xs text-fg-soft">
        从 OpenWrt DHCP 列表选择设备（主机名 · IP · MAC），并可为规则设置备注。保存后会以{" "}
        <code>SRC-IP-CIDR</code> 写在订阅规则最前；生效请点右上角「更新订阅」。不会出现在左侧「规则集」页。
        也可选「自定义」填写 CIDR（如 <code>172.16.1.0/24</code>）。
      </p>
      {error ? (
        <div className="mb-3">
          <Msg error={error} />
        </div>
      ) : null}
      {dhcpError ? (
        <div className="mb-3">
          <Msg error={`DHCP 列表：${dhcpError}`} />
        </div>
      ) : null}
      {!dhcpError && dhcp.length === 0 && !loadingDhcp ? (
        <p className="mb-3 text-xs text-faint">未读到 DHCP 客户端，可点「刷新 DHCP」或使用自定义 IP / CIDR。</p>
      ) : null}
      <div className="space-y-4">
        {routes.length === 0 && (
          <p className="text-sm text-faint">暂无规则，点击「新增」添加。</p>
        )}
        {routes.map((r, i) => {
          const matched = srcInDhcp(r.src, dhcp);
          const selectValue = matched ? r.src.trim() : CUSTOM;
          return (
            <div key={r.id || `lan-${i}`} className="rounded-xl border border-edge bg-composer/40 p-3.5">
              <div className="mb-2 flex items-center justify-between">
                <label className="flex items-center gap-2 text-sm text-fg-soft">
                  <input
                    type="checkbox"
                    checked={r.enabled}
                    onChange={(e) => update(i, { enabled: e.target.checked })}
                  />
                  启用
                </label>
                <Button
                  variant="danger"
                  onClick={() => setRoutes((prev) => prev.filter((_, idx) => idx !== i))}
                >
                  删除
                </Button>
              </div>
              <div className="grid gap-3 md:grid-cols-2">
                <div>
                  <Label>备注</Label>
                  <Input
                    value={r.name}
                    placeholder="如：客厅电视"
                    onChange={(e) => update(i, { name: e.target.value })}
                  />
                </div>
                <div>
                  <Label>源设备（DHCP）</Label>
                  <Select value={selectValue} onChange={(e) => pickDevice(i, e.target.value)}>
                    <option value={CUSTOM}>
                      {!matched && r.src.trim()
                        ? `自定义 · ${r.src.trim()}`
                        : "自定义 IP / CIDR…"}
                    </option>
                    {dhcp.map((c) => (
                      <option key={`${c.ip}-${c.mac}`} value={c.ip}>
                        {formatDhcpOption(c)}
                      </option>
                    ))}
                  </Select>
                </div>
                {selectValue === CUSTOM ? (
                  <div className="md:col-span-2">
                    <Label>自定义源 IP / CIDR</Label>
                    <Input
                      value={r.src}
                      placeholder="172.16.1.50 或 172.16.1.0/24"
                      onChange={(e) => update(i, { src: e.target.value })}
                    />
                  </div>
                ) : null}
                <div className="md:col-span-2">
                  <Label>目标（策略组或节点）</Label>
                  {(() => {
                    const known = targets.includes(r.target);
                    const targetSelect = known ? r.target : CUSTOM;
                    return (
                      <>
                        <Select
                          value={targetSelect}
                          onChange={(e) => {
                            const v = e.target.value;
                            if (v === CUSTOM) {
                              if (known) update(i, { target: "" });
                              return;
                            }
                            update(i, { target: v });
                          }}
                        >
                          <option value={CUSTOM}>
                            {!known && r.target.trim()
                              ? `自定义 · ${r.target.trim()}`
                              : "自定义节点全名…"}
                          </option>
                          {targets.map((n) => (
                            <option key={n} value={n}>
                              {n}
                            </option>
                          ))}
                        </Select>
                        {targetSelect === CUSTOM ? (
                          <Input
                            className="mt-2"
                            value={r.target}
                            placeholder="填写订阅里的节点全名"
                            onChange={(e) => update(i, { target: e.target.value })}
                          />
                        ) : null}
                      </>
                    );
                  })()}
                </div>
                {previewClashRule(r.src, r.target) ? (
                  <p className="md:col-span-2 rounded-lg bg-input/70 px-2.5 py-1.5 font-mono text-[11px] text-muted">
                    写入订阅：{previewClashRule(r.src, r.target)}
                    {!r.enabled ? "（当前未启用，不会写入）" : ""}
                  </p>
                ) : null}
              </div>
            </div>
          );
        })}
      </div>
    </Card>
  );
}

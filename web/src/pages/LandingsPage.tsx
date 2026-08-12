import { useEffect, useState } from "react";
import { api, newId } from "../api";
import { CONFIG_RELOAD_EVENT } from "../configReload";
import type { LandingProxy, LandingType } from "../types";
import { Button, Card, Input, Label, Msg, Select } from "../components/ui";
import { useTips } from "../tips";

export default function LandingsPage() {
  const tips = useTips();
  const [landings, setLandings] = useState<LandingProxy[]>([]);
  const [groupNames, setGroupNames] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const load = () => {
      api
        .getConfig()
        .then((c) => {
          setLandings(Array.isArray(c.landings) ? c.landings : []);
          setGroupNames(c.groups.map((g) => g.name));
        })
        .catch((e) => setError(String(e.message || e)));
    };
    load();
    window.addEventListener(CONFIG_RELOAD_EVENT, load);
    return () => window.removeEventListener(CONFIG_RELOAD_EVENT, load);
  }, []);

  function update(i: number, patch: Partial<LandingProxy>) {
    setLandings((prev) => prev.map((l, idx) => (idx === i ? { ...l, ...patch } : l)));
  }

  function add() {
    setError("");
    const item: LandingProxy = {
      id: newId("l"),
      name: `落地-SOCKS-${landings.length + 1}`,
      landing_type: "socks5",
      server: "127.0.0.1",
      port: 1080,
      username: "",
      password: "",
      // 默认独立节点，不强制前置代理；需要链式时再选 dialer-proxy
      dialer_proxy: "",
      enabled: true,
    };
    setLandings((prev) => [...prev, item]);
    tips.info("已添加独立 SOCKS5/HTTP，可直接保存；需要链式时再选前置代理");
  }

  async function save() {
    setSaving(true);
    setError("");
    try {
      for (const l of landings) {
        if (!l.name.trim()) throw new Error("落地代理名称不能为空");
        if (!l.server.trim()) throw new Error(`「${l.name}」服务器不能为空`);
        if (!l.port) throw new Error(`「${l.name}」端口无效`);
      }
      const saved = await api.putLandings(landings);
      setLandings(saved);
      tips.success(`已保存 ${saved.length} 条落地代理`);
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card
      title="落地代理（SOCKS5 / HTTP）"
      actions={
        <div className="flex gap-2">
          <Button variant="ghost" onClick={add}>
            新增
          </Button>
          <Button onClick={save} disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </Button>
        </div>
      }
    >
      <p className="mb-4 text-xs text-fg-soft">
        可直接追加独立 SOCKS5/HTTP（无需前置）。若填写「前置代理」，则先走该策略组再连落地（链式）。
        节点会加入「⛓ 链路」与「🚀 默认」供选择；链路内可选全部地区策略组。新增后请保存。
      </p>
      {error ? (
        <div className="mb-3">
          <Msg error={error} />
        </div>
      ) : null}
      <div className="space-y-4">
        {landings.length === 0 && (
          <p className="text-sm text-faint">暂无落地代理，点击「新增」添加。</p>
        )}
        {landings.map((l, i) => (
          <div key={l.id || `landing-${i}`} className="rounded-xl border border-edge bg-composer/40 p-3.5">
            <div className="mb-2 flex items-center justify-between">
              <label className="flex items-center gap-2 text-sm text-fg-soft">
                <input
                  type="checkbox"
                  checked={l.enabled}
                  onChange={(e) => update(i, { enabled: e.target.checked })}
                />
                启用
              </label>
              <Button
                variant="danger"
                onClick={() => setLandings((prev) => prev.filter((_, idx) => idx !== i))}
              >
                删除
              </Button>
            </div>
            <div className="grid gap-3 md:grid-cols-2">
              <div>
                <Label>名称</Label>
                <Input value={l.name} onChange={(e) => update(i, { name: e.target.value })} />
              </div>
              <div>
                <Label>类型</Label>
                <Select
                  value={l.landing_type}
                  onChange={(e) => update(i, { landing_type: e.target.value as LandingType })}
                >
                  <option value="socks5">socks5</option>
                  <option value="http">http</option>
                </Select>
              </div>
              <div>
                <Label>服务器</Label>
                <Input value={l.server} onChange={(e) => update(i, { server: e.target.value })} />
              </div>
              <div>
                <Label>端口</Label>
                <Input
                  type="number"
                  value={l.port}
                  onChange={(e) => update(i, { port: Number(e.target.value) || 0 })}
                />
              </div>
              <div>
                <Label>用户名</Label>
                <Input
                  value={l.username}
                  onChange={(e) => update(i, { username: e.target.value })}
                />
              </div>
              <div>
                <Label>密码</Label>
                <Input
                  type="password"
                  value={l.password}
                  onChange={(e) => update(i, { password: e.target.value })}
                />
              </div>
              <div className="md:col-span-2">
                <Label>前置代理（可选，空=直连该 SOCKS5/HTTP）</Label>
                <Select
                  value={l.dialer_proxy}
                  onChange={(e) => update(i, { dialer_proxy: e.target.value })}
                >
                  <option value="">不使用前置（独立节点）</option>
                  {groupNames.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </Select>
              </div>
            </div>
          </div>
        ))}
      </div>
    </Card>
  );
}

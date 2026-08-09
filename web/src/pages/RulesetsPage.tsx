import { useEffect, useState } from "react";
import { api, newId } from "../api";
import { CONFIG_RELOAD_EVENT } from "../configReload";
import type { RuleSet } from "../types";
import { Button, Card, Input, Label, Msg, Select, Textarea } from "../components/ui";
import { useTips } from "../tips";

export default function RulesetsPage() {
  const tips = useTips();
  const [rulesets, setRulesets] = useState<RuleSet[]>([]);
  const [groupNames, setGroupNames] = useState<string[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    const load = () => {
      api
        .getConfig()
        .then((c) => {
          setRulesets(c.rulesets);
          setGroupNames(c.groups.map((g) => g.name));
        })
        .catch((e) => setError(String(e.message || e)));
    };
    load();
    window.addEventListener(CONFIG_RELOAD_EVENT, load);
    return () => window.removeEventListener(CONFIG_RELOAD_EVENT, load);
  }, []);

  function update(i: number, patch: Partial<RuleSet>) {
    setRulesets((prev) => prev.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  }

  function add() {
    setRulesets((prev) => [
      ...prev,
      {
        id: newId("r"),
        name: "新规则集",
        group: groupNames[0] || "🚀 默认",
        rules: "DOMAIN-SUFFIX,example.com",
        enabled: true,
      },
    ]);
  }

  async function save() {
    setError("");
    try {
      const saved = await api.putRulesets(rulesets);
      setRulesets(saved);
      tips.success("规则集已保存");
    } catch (e) {
      setError(String((e as Error).message || e));
    }
  }

  return (
    <Card
      title="规则集"
      actions={
        <div className="flex gap-2">
          <Button variant="ghost" onClick={add}>
            新增
          </Button>
          <Button onClick={save}>保存</Button>
        </div>
      }
    >
      <p className="mb-4 text-xs text-fg-soft">
        每行一条 Clash 规则载荷（可不写策略名，保存时会自动接到绑定组）。例如{" "}
        <code>DOMAIN-SUFFIX,openai.com</code>。按设备源 IP 整机分流请到「局域网分流」，不会出现在本页。
      </p>
      <div className="space-y-4">
        {rulesets.map((r, i) => (
          <div key={r.id} className="rounded-xl border border-edge bg-composer/40 p-3.5">
            <div className="mb-2 flex items-center justify-between gap-2">
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
                onClick={() => setRulesets((prev) => prev.filter((_, idx) => idx !== i))}
              >
                删除
              </Button>
            </div>
            <div className="grid gap-3 md:grid-cols-2">
              <div>
                <Label>名称</Label>
                <Input value={r.name} onChange={(e) => update(i, { name: e.target.value })} />
              </div>
              <div>
                <Label>绑定策略组</Label>
                <Select value={r.group} onChange={(e) => update(i, { group: e.target.value })}>
                  {groupNames.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </Select>
              </div>
              <div className="md:col-span-2">
                <Label>规则</Label>
                <Textarea
                  rows={8}
                  value={r.rules}
                  onChange={(e) => update(i, { rules: e.target.value })}
                />
              </div>
            </div>
          </div>
        ))}
        {error ? <Msg error={error} /> : null}
      </div>
    </Card>
  );
}

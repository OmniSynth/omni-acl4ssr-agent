import { useEffect, useMemo, useState } from "react";
import { api, newId } from "../api";
import { CONFIG_RELOAD_EVENT } from "../configReload";
import type { GroupsMode, ProxyGroup } from "../types";
import { Button, Card, Input, Label, Msg } from "../components/ui";
import { useTheme } from "../theme";
import { useTips } from "../tips";

const DEFAULT_USER_GROUPS: Omit<ProxyGroup, "id">[] = [
  {
    name: "🤖 AI",
    group_type: "select",
    filter: "",
    proxies: ["🇺🇸 美国", "🇯🇵 日本", "🇸🇬 新加坡", "🇭🇰 香港"],
    url: "https://www.gstatic.com/generate_204",
    interval: 300,
    tolerance: 50,
    lazy: true,
  },
  {
    name: "💰 币安",
    group_type: "select",
    filter: "",
    proxies: ["🇭🇰 香港", "🇸🇬 新加坡", "🇯🇵 日本", "🇺🇸 美国"],
    url: "https://www.gstatic.com/generate_204",
    interval: 300,
    tolerance: 50,
    lazy: true,
  },
  {
    name: "📺 奈飞",
    group_type: "select",
    filter: "",
    proxies: ["🇸🇬 新加坡", "🇯🇵 日本", "🇹🇼 台湾", "🇺🇸 美国", "🇭🇰 香港"],
    url: "https://www.gstatic.com/generate_204",
    interval: 300,
    tolerance: 50,
    lazy: true,
  },
];

const DEFAULT_USER_IDS = ["g-ai", "g-binance", "g-netflix"] as const;

function isUserStrategyGroup(g: ProxyGroup): boolean {
  if (g.id === "g-default" || g.id === "g-chain" || g.id === "g-other") return false;
  return !String(g.filter || "").trim();
}

function seedUserGroups(all: ProxyGroup[]): ProxyGroup[] {
  if (all.some(isUserStrategyGroup)) return all;
  const seeded = DEFAULT_USER_GROUPS.map((g, i) => ({
    ...g,
    id: DEFAULT_USER_IDS[i] || newId("g"),
  }));
  return [...all, ...seeded];
}

export default function GroupsPage() {
  const tips = useTips();
  const [mode, setMode] = useState<GroupsMode>("managed");
  const [groups, setGroups] = useState<ProxyGroup[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const load = () => {
      api
        .getConfig()
        .then((c) => {
          setMode(c.groups_mode || "managed");
          setGroups(seedUserGroups(c.groups));
        })
        .catch((e) => setError(String(e.message || e)));
    };
    load();
    window.addEventListener(CONFIG_RELOAD_EVENT, load);
    return () => window.removeEventListener(CONFIG_RELOAD_EVENT, load);
  }, []);

  const userGroups = useMemo(() => groups.filter(isUserStrategyGroup), [groups]);
  const userIndexes = useMemo(
    () =>
      groups
        .map((g, i) => (isUserStrategyGroup(g) ? i : -1))
        .filter((i) => i >= 0),
    [groups],
  );

  function addUserGroup() {
    setGroups((prev) => [
      ...prev,
      {
        id: newId("g"),
        name: "新策略组",
        group_type: "select",
        filter: "",
        proxies: ["DIRECT"],
        url: "https://www.gstatic.com/generate_204",
        interval: 300,
        tolerance: 50,
        lazy: true,
      },
    ]);
  }

  function removeUserGroup(id: string) {
    setGroups((prev) => prev.filter((g) => g.id !== id));
  }

  function updateAt(i: number, patch: Partial<ProxyGroup>) {
    setGroups((prev) => prev.map((g, idx) => (idx === i ? { ...g, ...patch } : g)));
  }

  function add() {
    addUserGroup();
  }

  function update(i: number, patch: Partial<ProxyGroup>) {
    updateAt(i, patch);
  }

  async function switchMode(next: GroupsMode) {
    setError("");
    setBusy(true);
    try {
      const r = await api.putGroupsMode(next);
      setMode(r.groups_mode);
      tips.success(
        next === "managed"
          ? "已切换：国家自动托管，用户策略组可自定义"
          : "已切换为完全自定义",
      );
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    setError("");
    try {
      const saved = await api.putGroups(groups);
      setGroups(saved);
      tips.success("策略组已保存");
    } catch (e) {
      setError(String((e as Error).message || e));
    }
  }

  const { resolved } = useTheme();

  return (
    <div className="space-y-4">
      <Card title="策略组模式">
        <p className="mb-3 text-sm text-fg-soft">
          「自动托管」：国家/地区组按订阅自动生成；下方用户策略组可增删，成员填写地区组名。「完全自定义」则全部手写。
        </p>
        <div
          className="inline-flex flex-wrap rounded-xl border border-edge bg-composer p-0.5"
          role="group"
          aria-label="策略组模式"
        >
          {(
            [
              ["managed", "自动托管（推荐）"],
              ["custom", "完全自定义"],
            ] as const
          ).map(([id, label]) => {
            const active = mode === id;
            return (
              <button
                key={id}
                type="button"
                disabled={busy}
                aria-pressed={active}
                onClick={() => {
                  if (!active) void switchMode(id);
                }}
                className={[
                  "rounded-lg px-3.5 py-1.5 text-sm font-medium transition disabled:opacity-50",
                  active
                    ? resolved === "dark"
                      ? "bg-zinc-100 text-zinc-900"
                      : "bg-zinc-900 text-white"
                    : "text-fg-soft hover:bg-hover hover:text-fg",
                ].join(" ")}
              >
                {label}
              </button>
            );
          })}
        </div>
        {error ? (
          <div className="mt-3">
            <Msg error={error} />
          </div>
        ) : null}
      </Card>

      {mode === "managed" && (
        <Card
          title="用户策略组"
          actions={
            <div className="flex gap-2">
              <Button variant="ghost" onClick={addUserGroup}>
                新增
              </Button>
              <Button onClick={save}>保存</Button>
            </div>
          }
        >
          <p className="mb-3 text-xs text-fg-soft">
            成员填地区组名或 DIRECT，逗号分隔。本次订阅没有的地区会自动忽略；AI/币安/奈飞若成员被清空则回退默认优选。
          </p>
          <div className="space-y-4">
            {userGroups.length === 0 && (
              <p className="text-sm text-fg-soft">暂无用户策略组，可点「新增」。规则集若引用已删组名需同步修改。</p>
            )}
            {userIndexes.map((i) => {
              const g = groups[i];
              return (
                <div key={g.id} className="rounded-xl border border-edge bg-composer/40 p-3.5">
                  <div className="mb-2 flex items-center justify-between">
                    <span className="text-xs text-faint">{g.id}</span>
                    <Button variant="danger" onClick={() => removeUserGroup(g.id)}>
                      删除
                    </Button>
                  </div>
                  <div className="grid gap-3 md:grid-cols-2">
                    <div>
                      <Label>名称</Label>
                      <Input
                        value={g.name}
                        onChange={(e) => updateAt(i, { name: e.target.value })}
                      />
                    </div>
                    <div className="md:col-span-2">
                      <Label>成员 proxies（逗号分隔）</Label>
                      <Input
                        value={g.proxies.join(",")}
                        placeholder="🇭🇰 香港, 🇯🇵 日本, DIRECT"
                        onChange={(e) =>
                          updateAt(i, {
                            proxies: e.target.value
                              .split(",")
                              .map((s) => s.trim())
                              .filter(Boolean),
                          })
                        }
                      />
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </Card>
      )}

      {mode === "custom" && (
        <Card
          title="自定义策略组"
          actions={
            <div className="flex gap-2">
              <Button variant="ghost" onClick={add}>
                新增
              </Button>
              <Button onClick={save}>保存</Button>
            </div>
          }
        >
          <div className="space-y-4">
            {groups.map((g, i) => (
              <div key={g.id} className="rounded-xl border border-edge bg-composer/40 p-3.5">
                <div className="mb-2 flex items-center justify-between">
                  <span className="text-xs text-faint">{g.id}</span>
                  <Button
                    variant="danger"
                    onClick={() => setGroups((prev) => prev.filter((_, idx) => idx !== i))}
                  >
                    删除
                  </Button>
                </div>
                <div className="grid gap-3 md:grid-cols-2">
                  <div>
                    <Label>名称</Label>
                    <Input value={g.name} onChange={(e) => update(i, { name: e.target.value })} />
                  </div>
                  <div className="md:col-span-2">
                    <Label>节点名正则 filter</Label>
                    <Input
                      value={g.filter}
                      placeholder="(?i)香港|HK"
                      onChange={(e) => update(i, { filter: e.target.value })}
                    />
                  </div>
                  <div className="md:col-span-2">
                    <Label>额外 proxies（逗号分隔）</Label>
                    <Input
                      value={g.proxies.join(",")}
                      onChange={(e) =>
                        update(i, {
                          proxies: e.target.value
                            .split(",")
                            .map((s) => s.trim())
                            .filter(Boolean),
                        })
                      }
                    />
                  </div>
                </div>
              </div>
            ))}
          </div>
        </Card>
      )}
    </div>
  );
}

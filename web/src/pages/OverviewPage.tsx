import { useEffect, useState } from "react";
import { api, subscriptionUrl } from "../api";
import { CONFIG_RELOAD_EVENT, CONVERT_INFO_EVENT } from "../configReload";
import { useTips } from "../tips";
import { normalizeProfile, type ConvertResponse, type GroupsMode, type Profile } from "../types";
import { Button, Card, Input, Label, Msg, Select } from "../components/ui";

export default function OverviewPage() {
  const tips = useTips();
  const [profile, setProfile] = useState<Profile | null>(null);
  const [groupsMode, setGroupsMode] = useState<GroupsMode>("managed");
  const [groupNames, setGroupNames] = useState<string[]>([]);
  const [convertInfo, setConvertInfo] = useState<ConvertResponse | null>(null);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [converting, setConverting] = useState(false);
  const sub = subscriptionUrl();

  useEffect(() => {
    const load = () => {
      api
        .getConfig()
        .then((c) => {
          setProfile(normalizeProfile(c.profile));
          setGroupsMode(c.groups_mode || "managed");
          setGroupNames(c.groups.map((g) => g.name));
        })
        .catch((e) => setError(String(e.message || e)));
    };
    load();
    window.addEventListener(CONFIG_RELOAD_EVENT, load);
    const onConvertInfo = (ev: Event) => {
      const detail = (ev as CustomEvent<{
        proxy_count: number;
        group_count: number;
        rule_count: number;
        region_count?: number;
        unmatched_count?: number;
      }>).detail;
      if (!detail) return;
      setConvertInfo({
        ok: true,
        proxy_count: detail.proxy_count,
        group_count: detail.group_count,
        rule_count: detail.rule_count,
        message: "",
        groups_mode: groupsMode,
        region_count: detail.region_count,
        regions: [],
        unmatched_count: detail.unmatched_count ?? 0,
        unmatched_samples: [],
      });
    };
    window.addEventListener(CONVERT_INFO_EVENT, onConvertInfo);
    return () => {
      window.removeEventListener(CONFIG_RELOAD_EVENT, load);
      window.removeEventListener(CONVERT_INFO_EVENT, onConvertInfo);
    };
  }, [groupsMode]);

  async function save() {
    if (!profile) return;
    setSaving(true);
    setError("");
    try {
      const payload = normalizeProfile({
        ...profile,
        upstream_urls: profile.upstream_urls.map((u) => u.trim()).filter(Boolean),
      });
      if (!payload.upstream_urls.length) {
        throw new Error("请至少填写一个上游订阅 URL");
      }
      const p = await api.putProfile(payload);
      setProfile(normalizeProfile(p));
      tips.success("已保存");
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setSaving(false);
    }
  }

  async function convertNow() {
    setConverting(true);
    setError("");
    try {
      const r = await api.convert(false);
      setConvertInfo(r);
      if (!r.ok) throw new Error(r.message);
      tips.success(r.message);
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setConverting(false);
    }
  }

  async function copySub() {
    await navigator.clipboard.writeText(sub);
    tips.success("订阅地址已复制");
  }

  function setUrl(i: number, value: string) {
    if (!profile) return;
    const upstream_urls = [...profile.upstream_urls];
    upstream_urls[i] = value;
    setProfile({ ...profile, upstream_urls });
  }

  function addUrl() {
    if (!profile) return;
    setProfile({ ...profile, upstream_urls: [...profile.upstream_urls, ""] });
  }

  function removeUrl(i: number) {
    if (!profile) return;
    const upstream_urls = profile.upstream_urls.filter((_, idx) => idx !== i);
    setProfile({ ...profile, upstream_urls: upstream_urls.length ? upstream_urls : [""] });
  }

  if (!profile) {
    return <Msg error={error || "加载中…"} />;
  }

  return (
    <div className="space-y-4">
      <Card
        title="档案"
        actions={
          <div className="flex gap-2">
            <Button variant="ghost" onClick={convertNow} disabled={converting}>
              {converting ? "预览中…" : "预览转换"}
            </Button>
            <Button onClick={save} disabled={saving}>
              {saving ? "保存中…" : "保存"}
            </Button>
          </div>
        }
      >
        <div className="grid gap-4 md:grid-cols-2">
          <div>
            <Label>名称</Label>
            <Input
              value={profile.name}
              onChange={(e) => setProfile({ ...profile, name: e.target.value })}
            />
          </div>
          <div>
            <Label>默认出口组（MATCH）</Label>
            {groupsMode === "managed" ? (
              <>
                <Input value="🚀 默认" disabled />
                <p className="mt-1 text-xs text-faint">
                  自动托管模式固定为「🚀 默认」（按订阅自适应地区）。可在「策略组」页切手动。
                </p>
              </>
            ) : (
              <Select
                value={profile.default_group}
                onChange={(e) => setProfile({ ...profile, default_group: e.target.value })}
              >
                {groupNames.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </Select>
            )}
          </div>
          <div className="md:col-span-2">
            <div className="mb-2 flex items-center justify-between">
              <Label>上游订阅（可多个，转换时聚合节点）</Label>
              <Button variant="ghost" onClick={addUrl}>
                添加订阅
              </Button>
            </div>
            <div className="space-y-2">
              {profile.upstream_urls.map((url, i) => (
                <div key={`url-${i}`} className="flex gap-2">
                  <Input
                    value={url}
                    placeholder={`https://... 订阅 #${i + 1}`}
                    onChange={(e) => setUrl(i, e.target.value)}
                  />
                  <Button variant="danger" onClick={() => removeUrl(i)} disabled={profile.upstream_urls.length <= 1}>
                    删
                  </Button>
                </div>
              ))}
            </div>
          </div>
          <div className="md:col-span-2">
            <Label>User-Agent</Label>
            <Input
              value={profile.user_agent}
              onChange={(e) => setProfile({ ...profile, user_agent: e.target.value })}
            />
          </div>
          <label className="flex items-center gap-2 text-sm text-fg-soft">
            <input
              type="checkbox"
              checked={profile.enabled}
              onChange={(e) => setProfile({ ...profile, enabled: e.target.checked })}
            />
            启用档案（关闭后 /sub 拒绝服务）
          </label>
        </div>
        {error ? (
          <div className="mt-3">
            <Msg error={error} />
          </div>
        ) : null}
      </Card>

      {convertInfo?.ok ? (
        <Card title="最近转换结果">
          <p className="text-sm text-fg-soft">
            共 {convertInfo.proxy_count} 个节点 → {convertInfo.group_count} 个策略组 →{" "}
            {convertInfo.rule_count} 条规则。
            {groupsMode === "managed" && convertInfo.region_count != null
              ? ` 识别 ${convertInfo.region_count} 个地区。`
              : null}
            {groupsMode === "managed"
              ? " 托管模式下节点按地区归入 url-test 组（如 🇭🇰 香港）；server 为 IPv6 字面地址的节点另入 🧬 IPv6。在 zashboard 需展开对应组才能看到全部节点。"
              : null}
          </p>
          {(convertInfo.fetch_warnings?.length ?? 0) > 0 ? (
            <div className="mt-3 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
              <p className="font-medium">部分上游拉取失败（/sub 会优先使用上次完整缓存）</p>
              <ul className="mt-1 list-inside list-disc text-fg-soft">
                {convertInfo.fetch_warnings!.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            </div>
          ) : null}
          {(convertInfo.regions?.length ?? 0) > 0 ? (
            <div className="mt-3 flex flex-wrap gap-2">
              {convertInfo.regions!.map((r) => (
                <span
                  key={r.id}
                  className="rounded-lg border border-edge bg-composer px-2 py-1 text-xs text-fg-soft"
                >
                  {r.name} · {r.count}
                </span>
              ))}
            </div>
          ) : null}
          {(convertInfo.unmatched_count ?? 0) > 0 ? (
            <p className="mt-2 text-xs text-faint">
              未匹配地区 {convertInfo.unmatched_count}：{" "}
              {(convertInfo.unmatched_samples ?? []).join("、")}
            </p>
          ) : null}
        </Card>
      ) : null}

      <Card
        title="Nikki 订阅地址"
        actions={
          <Button variant="ghost" onClick={copySub}>
            复制
          </Button>
        }
      >
        <code className="block break-all rounded bg-input px-3 py-2 text-sm text-accent-fg">
          {sub}
        </code>
        <p className="mt-2 text-xs text-fg-soft">
          多订阅聚合后由此地址输出；Nikki 填本机 http://127.0.0.1:8787/sub 即可。右上角「更新订阅」
          会写入 Nikki 并重载生效；「预览转换」仅查看节点统计，不会更新 Nikki。
        </p>
      </Card>
    </div>
  );
}

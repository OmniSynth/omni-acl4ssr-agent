/** AI 应用配置等场景：通知各页面重新拉取数据 */

export const CONFIG_RELOAD_EVENT = "omni:config-reload";

export function requestConfigReload() {
  window.dispatchEvent(new Event(CONFIG_RELOAD_EVENT));
}

export function pathForAiOps(ops: { op: string }[]): string {
  const score = { landing: 0, lan: 0, ruleset: 0, group: 0 };
  for (const o of ops) {
    const op = (o.op || "").toLowerCase();
    if (op.includes("landing")) score.landing += 1;
    else if (op.includes("lan_route")) score.lan += 1;
    else if (op.includes("ruleset")) score.ruleset += 1;
    else if (op.includes("group")) score.group += 1;
  }
  if (score.landing) return "/landings";
  if (score.lan) return "/lan-routes";
  if (score.ruleset) return "/rulesets";
  if (score.group) return "/groups";
  return "/";
}

export type GroupType = "select" | "url_test";
export type LandingType = "socks5" | "http";
export type GroupsMode = "managed" | "custom";

export interface Profile {
  name: string;
  /** @deprecated 兼容旧字段 */
  upstream_url?: string;
  upstream_urls: string[];
  default_group: string;
  enabled: boolean;
  user_agent: string;
}

export interface ProxyGroup {
  id: string;
  name: string;
  group_type: GroupType;
  filter: string;
  proxies: string[];
  url: string;
  interval: number;
  tolerance: number;
  lazy: boolean;
}

export interface RuleSet {
  id: string;
  name: string;
  group: string;
  rules: string;
  enabled: boolean;
}

export interface LandingProxy {
  id: string;
  name: string;
  landing_type: LandingType;
  server: string;
  port: number;
  username: string;
  password: string;
  dialer_proxy: string;
  enabled: boolean;
}

/** 局域网源 IP → 策略组/节点 */
export interface LanRoute {
  id: string;
  /** 备注（如「客厅电视」） */
  name: string;
  src: string;
  target: string;
  enabled: boolean;
}

/** OpenWrt DHCP 客户端（动态租约 / 静态绑定） */
export interface DhcpClient {
  hostname: string;
  ip: string;
  mac: string;
  static_lease: boolean;
}

export interface NikkiSubscription {
  section_id: string;
  name: string;
  url: string;
  last_update?: string | null;
  success?: boolean | null;
}

export interface NikkiUpdateResult {
  ok: boolean;
  message: string;
  section_ids: string[];
  reloaded: boolean;
}

export interface NikkiPanelInfo {
  ok: boolean;
  path: string;
  port: number;
  ui_path: string;
  secret?: string | null;
  message: string;
}

export interface AppConfig {
  profile: Profile;
  groups_mode?: GroupsMode;
  groups: ProxyGroup[];
  rulesets: RuleSet[];
  landings: LandingProxy[];
  lan_routes?: LanRoute[];
}

export interface RegionStat {
  id: string;
  name: string;
  count: number;
}

export interface ConvertResponse {
  ok: boolean;
  proxy_count: number;
  group_count: number;
  rule_count: number;
  yaml?: string | null;
  message: string;
  groups_mode?: GroupsMode | string;
  regions?: RegionStat[];
  unmatched_count?: number;
  unmatched_samples?: string[];
}

export type AiProvider = "gemini" | "deepseek" | string;

export interface AiSettingsPublic {
  provider?: AiProvider;
  has_api_key: boolean;
  api_key_masked: string;
  model: string;
  source: string;
  /** 0 = 跟随模型上限 */
  context_window?: number;
  context_window_choices?: number[];
  /** DeepSeek thinking / Gemini thinkingConfig */
  thinking_enabled?: boolean;
  /** 与 platform「累计消费金额」对齐（元） */
  deepseek_spent_sync?: number | null;
}

export interface AiModelInfo {
  id: string;
  display_name: string;
  /** free | paid | unknown */
  tier?: "free" | "paid" | "unknown" | string;
  tier_label?: string;
  input_token_limit?: number;
  output_token_limit?: number;
}

export interface AiUsage {
  provider?: string;
  day: string;
  requests_today: number;
  tokens_today: number;
  last_prompt_tokens: number;
  last_output_tokens: number;
  last_total_tokens: number;
  last_model: string;
  context_limit: number;
  context_used: number;
  context_pct: number;
  quota_rpm_hint?: number | null;
  quota_rpd_hint?: number | null;
  quota_note: string;
  quota_exhausted?: boolean;
  quota_blocked_model?: string;
  quota_retry_after_secs?: number;
  /** DeepSeek GET /user/balance */
  balance_available?: boolean | null;
  balance_currency?: string | null;
  balance_total?: string | null;
  balance_granted?: string | null;
  balance_topped_up?: string | null;
  /** DeepSeek：累计消费（估算） */
  balance_spent?: string | null;
  /** DeepSeek：累计消费 + 充值余额 */
  balance_quota_total?: string | null;
  /** DeepSeek：已消费 / 总额 × 100 */
  balance_quota_pct?: number | null;
}

export interface AiOp {
  op: string;
  id?: string | null;
  name?: string | null;
  proxies?: string[] | null;
  group?: string | null;
  rules?: string | null;
  enabled?: boolean | null;
  src?: string | null;
  target?: string | null;
  landing_type?: string | null;
  server?: string | null;
  port?: number | null;
  username?: string | null;
  password?: string | null;
  dialer_proxy?: string | null;
}

export interface AiAttachment {
  mime_type: string;
  data_base64: string;
  name?: string;
}

export interface AiAgentOption {
  id: string;
  label: string;
}

/** plan | ask | advice | error */
export type AiAgentKind = "plan" | "ask" | "advice" | "error" | string;

export interface AiPlanResponse {
  kind: AiAgentKind;
  summary: string;
  ops: AiOp[];
  preview: string[];
  options?: AiAgentOption[];
  steps?: string[];
  thinking?: string;
  usage?: AiUsage;
  raw?: string;
  chat_id?: string | null;
}

export type AiAgentStreamEvent =
  | { type: "status"; text: string }
  | { type: "thinking"; delta: string }
  | { type: "step"; text: string }
  | { type: "result"; result: AiPlanResponse }
  | { type: "error"; message: string }
  | { type: "done" };

export interface AiChatSummary {
  id: string;
  title: string;
  archived: boolean;
  created_at: string;
  updated_at: string;
  model: string;
  message_count: number;
  parent_id?: string | null;
}

export interface AiChatMessage {
  id: string;
  role: "user" | "assistant" | string;
  content: string;
  created_at: string;
  summary?: string | null;
  /** plan | ask | advice | error；旧消息缺省视为 plan */
  kind?: AiAgentKind | null;
  ops?: AiOp[] | null;
  preview?: string[] | null;
  options?: AiAgentOption[] | null;
  steps?: string[] | null;
  thinking?: string | null;
  prompt_tokens?: number | null;
  output_tokens?: number | null;
  total_tokens?: number | null;
  /** 已应用到配置的时间；有值则不可再点「应用变更」 */
  applied_at?: string | null;
}

export interface AiChatThread {
  id: string;
  title: string;
  archived: boolean;
  created_at: string;
  updated_at: string;
  model: string;
  messages: AiChatMessage[];
  parent_id?: string | null;
  /** 本对话最近一轮 Gemini prompt tokens（≈ 上下文占用） */
  last_prompt_tokens?: number;
  last_output_tokens?: number;
  last_total_tokens?: number;
  context_limit?: number;
}

export function normalizeProfile(p: Profile): Profile {
  const urls: string[] = [];
  for (const u of [...(p.upstream_urls || []), p.upstream_url || ""]) {
    const t = u.trim();
    if (t && !urls.includes(t)) urls.push(t);
  }
  return {
    ...p,
    upstream_urls: urls.length ? urls : [""],
    upstream_url: urls[0] || "",
  };
}

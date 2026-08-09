import type {
  AiAgentStreamEvent,
  AiAttachment,
  AiChatSummary,
  AiChatThread,
  AiModelInfo,
  AiOp,
  AiPlanResponse,
  AiSettingsPublic,
  AiUsage,
  AppConfig,
  ConvertResponse,
  GroupsMode,
  DhcpClient,
  LandingProxy,
  LanRoute,
  NikkiPanelInfo,
  NikkiUpdateResult,
  Profile,
  ProxyGroup,
  RuleSet,
} from "./types";

async function json<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(init?.headers || {}) },
    ...init,
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error((body as { message?: string }).message || `HTTP ${res.status}`);
  }
  return res.json();
}

export const api = {
  health: () => json<{ ok: boolean }>("/api/health"),
  getConfig: () => json<AppConfig>("/api/config"),
  putConfig: (data: AppConfig) =>
    json<AppConfig>("/api/config", { method: "PUT", body: JSON.stringify(data) }),
  putProfile: (profile: Profile) =>
    json<Profile>("/api/profile", { method: "PUT", body: JSON.stringify(profile) }),
  putGroups: (groups: ProxyGroup[]) =>
    json<ProxyGroup[]>("/api/groups", { method: "PUT", body: JSON.stringify(groups) }),
  getGroupsMode: () => json<{ groups_mode: GroupsMode }>("/api/groups-mode"),
  putGroupsMode: (groups_mode: GroupsMode) =>
    json<{ groups_mode: GroupsMode }>("/api/groups-mode", {
      method: "PUT",
      body: JSON.stringify({ groups_mode }),
    }),
  putRulesets: (rulesets: RuleSet[]) =>
    json<RuleSet[]>("/api/rulesets", { method: "PUT", body: JSON.stringify(rulesets) }),
  putLandings: (landings: LandingProxy[]) =>
    json<LandingProxy[]>("/api/landings", {
      method: "PUT",
      body: JSON.stringify(landings),
    }),
  putLanRoutes: (lan_routes: LanRoute[]) =>
    json<LanRoute[]>("/api/lan-routes", {
      method: "PUT",
      body: JSON.stringify(lan_routes),
    }),
  getDhcpClients: () => json<DhcpClient[]>("/api/dhcp-clients"),
  updateNikkiSubscription: (body?: { section_id?: string; reload?: boolean }) =>
    json<NikkiUpdateResult>("/api/nikki/update-subscription", {
      method: "POST",
      body: JSON.stringify(body || {}),
    }),
  getNikkiPanel: () => json<NikkiPanelInfo>("/api/nikki/panel"),
  convert: (includeYaml = false) =>
    json<ConvertResponse>("/api/convert", {
      method: "POST",
      body: JSON.stringify({ include_yaml: includeYaml }),
    }),
  getAiSettings: () => json<AiSettingsPublic>("/api/ai/settings"),
  putAiSettings: (body: {
    provider?: string;
    api_key: string;
    model: string;
    context_window?: number;
    thinking_enabled?: boolean;
    deepseek_spent_sync?: number | null;
  }) =>
    json<AiSettingsPublic>("/api/ai/settings", {
      method: "PUT",
      body: JSON.stringify(body),
    }),
  getAiModels: () => json<{ models: AiModelInfo[] }>("/api/ai/models"),
  getAiUsage: () => json<AiUsage>("/api/ai/usage"),
  aiPlan: (
    prompt: string,
    context_limit?: number,
    chatId?: string | null,
    attachments?: AiAttachment[],
    choiceId?: string | null,
  ) =>
    json<AiPlanResponse>("/api/ai/plan", {
      method: "POST",
      body: JSON.stringify({
        prompt,
        context_limit: context_limit || undefined,
        chat_id: chatId || undefined,
        attachments: attachments?.length ? attachments : undefined,
        choice_id: choiceId || undefined,
        stream: false,
      }),
    }),
  aiPlanStream: async (
    prompt: string,
    context_limit: number | undefined,
    chatId: string | null | undefined,
    attachments: AiAttachment[] | undefined,
    choiceId: string | null | undefined,
    onEvent: (ev: AiAgentStreamEvent) => void,
  ): Promise<AiPlanResponse> => {
    const res = await fetch("/api/ai/plan", {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
      body: JSON.stringify({
        prompt,
        context_limit: context_limit || undefined,
        chat_id: chatId || undefined,
        attachments: attachments?.length ? attachments : undefined,
        choice_id: choiceId || undefined,
        stream: true,
      }),
    });
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error((body as { message?: string }).message || `HTTP ${res.status}`);
    }
    if (!res.body) throw new Error("浏览器不支持流式响应");
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    let finalResult: AiPlanResponse | null = null;
    let streamError: string | null = null;

    const handleData = (data: string) => {
      if (!data || data === "[DONE]") return;
      let ev: AiAgentStreamEvent;
      try {
        ev = JSON.parse(data) as AiAgentStreamEvent;
      } catch {
        return;
      }
      onEvent(ev);
      if (ev.type === "result") finalResult = ev.result;
      if (ev.type === "error") streamError = ev.message;
    };

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      while (true) {
        const sep = buf.indexOf("\n\n");
        if (sep < 0) break;
        const frame = buf.slice(0, sep);
        buf = buf.slice(sep + 2);
        for (const line of frame.split("\n")) {
          const trimmed = line.trimEnd();
          if (trimmed.startsWith("data:")) {
            handleData(trimmed.slice(5).trimStart());
          }
        }
        // 让出事件循环，确保 React 能绘制中间态
        await Promise.resolve();
      }
    }
    if (buf.trim()) {
      for (const line of buf.split("\n")) {
        if (line.startsWith("data:")) handleData(line.slice(5).trimStart());
      }
    }
    if (streamError) throw new Error(streamError);
    if (!finalResult) throw new Error("流式响应未返回结果");
    return finalResult;
  },
  aiTranscribe: (mime_type: string, data_base64: string) =>
    json<{ text: string; usage: AiUsage }>("/api/ai/transcribe", {
      method: "POST",
      body: JSON.stringify({ mime_type, data_base64 }),
    }),
  aiApply: (ops: AiOp[], chatId?: string | null, messageId?: string | null) =>
    json<{
      ok: boolean;
      applied: string[];
      message: string;
      chat?: AiChatThread | null;
    }>("/api/ai/apply", {
      method: "POST",
      body: JSON.stringify({
        ops,
        chat_id: chatId || undefined,
        message_id: messageId || undefined,
      }),
    }),
  listAiChats: (includeArchived = false) =>
    json<{ chats: AiChatSummary[] }>(
      `/api/ai/chats?include_archived=${includeArchived ? "true" : "false"}&roots_only=true`,
    ),
  createAiChat: (title?: string) =>
    json<AiChatThread>("/api/ai/chats", {
      method: "POST",
      body: JSON.stringify({ title: title || undefined }),
    }),
  getAiChat: (id: string) => json<AiChatThread>(`/api/ai/chats/${encodeURIComponent(id)}`),
  patchAiChat: (id: string, body: { title?: string; archived?: boolean }) =>
    json<AiChatThread>(`/api/ai/chats/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteAiChat: (id: string) =>
    json<{ ok: boolean }>(`/api/ai/chats/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  branchAiChat: (id: string, messageId: string) =>
    json<AiChatThread>(`/api/ai/chats/${encodeURIComponent(id)}/branch`, {
      method: "POST",
      body: JSON.stringify({ message_id: messageId }),
    }),
};

export function newId(prefix: string): string {
  try {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
      return `${prefix}-${crypto.randomUUID().slice(0, 8)}`;
    }
  } catch {
    /* HTTP 非安全上下文可能无 randomUUID */
  }
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

export function subscriptionUrl(): string {
  return `${window.location.origin}/sub`;
}

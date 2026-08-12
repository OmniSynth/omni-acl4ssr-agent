use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::stream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt as _;

use crate::ai::{self, AiOp, AiSettings, AiStore};
use crate::ai_agent;
use crate::ai_chats::ChatStore;
use crate::countries::WorldCatalog;
use crate::dhcp::{self, DhcpClient};
use crate::engine::{self, YamlCache};
use crate::nikki::{self, NikkiPanelInfo, NikkiSubscription, NikkiUpdateResult};
use crate::model::{
    AppStateData, ConvertRequest, ConvertResponse, GroupsModeBody, LandingProxy, LanRoute, Profile,
    ProxyGroup, RuleSet,
};
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub http: reqwest::Client,
    pub cache: YamlCache,
    pub world: Arc<WorldCatalog>,
    pub ai: AiStore,
    pub chats: ChatStore,
}

type ApiResult<T> = Result<T, ApiError>;

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
    fn internal(err: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            ok: bool,
            message: String,
        }
        (
            self.status,
            Json(Body {
                ok: false,
                message: self.message,
            }),
        )
            .into_response()
    }
}

pub async fn health() -> impl IntoResponse {
    let listen = std::env::var("OMNI_LISTEN").unwrap_or_else(|_| "0.0.0.0:8787".into());
    let tls_listen = crate::tls::resolve_tls_listen(&listen);
    Json(serde_json::json!({
        "ok": true,
        "service": "omni-acl4ssr-agent",
        "listen": listen,
        "tls_listen": tls_listen,
    }))
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> Json<AppStateData> {
    Json(state.store.get().await)
}

pub async fn get_profile(State(state): State<Arc<AppState>>) -> Json<Profile> {
    Json(state.store.get().await.profile)
}

pub async fn put_profile(
    State(state): State<Arc<AppState>>,
    Json(mut profile): Json<Profile>,
) -> ApiResult<Json<Profile>> {
    profile.normalize();
    let data = state
        .store
        .update(|d| d.profile = profile)
        .await
        .map_err(ApiError::internal)?;
    state.cache.clear().await;
    Ok(Json(data.profile))
}

pub async fn get_groups(State(state): State<Arc<AppState>>) -> Json<Vec<ProxyGroup>> {
    Json(state.store.get().await.groups)
}

pub async fn put_groups(
    State(state): State<Arc<AppState>>,
    Json(groups): Json<Vec<ProxyGroup>>,
) -> ApiResult<Json<Vec<ProxyGroup>>> {
    let data = state
        .store
        .update(|d| d.groups = groups)
        .await
        .map_err(ApiError::internal)?;
    state.cache.clear().await;
    Ok(Json(data.groups))
}

pub async fn get_groups_mode(State(state): State<Arc<AppState>>) -> Json<GroupsModeBody> {
    Json(GroupsModeBody {
        groups_mode: state.store.get().await.groups_mode,
    })
}

pub async fn put_groups_mode(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GroupsModeBody>,
) -> ApiResult<Json<GroupsModeBody>> {
    let data = state
        .store
        .update(|d| {
            d.groups_mode = body.groups_mode;
            if matches!(body.groups_mode, crate::model::GroupsMode::Managed) {
                d.profile.default_group = crate::regions::NAME_DEFAULT.into();
            }
        })
        .await
        .map_err(ApiError::internal)?;
    state.cache.clear().await;
    Ok(Json(GroupsModeBody {
        groups_mode: data.groups_mode,
    }))
}

pub async fn get_rulesets(State(state): State<Arc<AppState>>) -> Json<Vec<RuleSet>> {
    Json(state.store.get().await.rulesets)
}

pub async fn put_rulesets(
    State(state): State<Arc<AppState>>,
    Json(rulesets): Json<Vec<RuleSet>>,
) -> ApiResult<Json<Vec<RuleSet>>> {
    let data = state
        .store
        .update(|d| d.rulesets = rulesets)
        .await
        .map_err(ApiError::internal)?;
    state.cache.clear().await;
    Ok(Json(data.rulesets))
}

pub async fn get_landings(State(state): State<Arc<AppState>>) -> Json<Vec<LandingProxy>> {
    Json(state.store.get().await.landings)
}

pub async fn put_landings(
    State(state): State<Arc<AppState>>,
    Json(landings): Json<Vec<LandingProxy>>,
) -> ApiResult<Json<Vec<LandingProxy>>> {
    let data = state
        .store
        .update(|d| d.landings = landings)
        .await
        .map_err(ApiError::internal)?;
    state.cache.clear().await;
    Ok(Json(data.landings))
}

pub async fn get_lan_routes(State(state): State<Arc<AppState>>) -> Json<Vec<LanRoute>> {
    Json(state.store.get().await.lan_routes)
}

pub async fn put_lan_routes(
    State(state): State<Arc<AppState>>,
    Json(lan_routes): Json<Vec<LanRoute>>,
) -> ApiResult<Json<Vec<LanRoute>>> {
    let data = state
        .store
        .update(|d| d.lan_routes = lan_routes)
        .await
        .map_err(ApiError::internal)?;
    state.cache.clear().await;
    Ok(Json(data.lan_routes))
}

/// OpenWrt DHCP 列表（动态租约 + 静态绑定），用于局域网分流选源。
pub async fn get_dhcp_clients() -> Json<Vec<DhcpClient>> {
    Json(dhcp::list_dhcp_clients().await)
}

pub async fn get_nikki_subscriptions() -> Json<Vec<NikkiSubscription>> {
    Json(nikki::list_subscriptions().await)
}

pub async fn get_nikki_panel() -> Json<NikkiPanelInfo> {
    Json(nikki::panel_info().await)
}

#[derive(Debug, Deserialize)]
pub struct NikkiUpdateBody {
    /// 指定 uci section；空则优先更新指向本机 /sub 的订阅
    #[serde(default)]
    pub section_id: Option<String>,
    /// 下载后是否 `/etc/init.d/nikki reload`（默认 true，使新订阅立即生效）
    #[serde(default = "default_true_bool")]
    pub reload: bool,
}

fn default_true_bool() -> bool {
    true
}

/// 更新本机 Nikki 订阅并默认重载。
/// 重载时临时 prefer=local，避免 start 在网络窗口里再次远程拉取。
pub async fn post_nikki_update_subscription(
    State(state): State<Arc<AppState>>,
    Json(body): Json<NikkiUpdateBody>,
) -> Json<NikkiUpdateResult> {
    // 先预热 /sub，避免 Nikki curl 时撞上冷转换 + 上游超时拿到不完整 YAML
    let data = state.store.get().await;
    if data.profile.enabled && !data.profile.urls().is_empty() {
        let extras = state.world.extras().await;
        match engine::convert(&data, &state.http, &extras).await {
            Ok(result) => {
                let key = engine::config_cache_key(&data, &extras);
                state
                    .cache
                    .set_full(
                        key,
                        result.yaml,
                        result.proxy_count,
                        result.fetch_warnings.len(),
                    )
                    .await;
                if !result.fetch_warnings.is_empty() {
                    tracing::warn!(
                        warnings = ?result.fetch_warnings,
                        proxies = result.proxy_count,
                        "预热 /sub 时部分上游失败"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "预热 /sub 失败，仍尝试更新 Nikki");
            }
        }
    }
    Json(nikki::update_subscriptions(body.section_id.as_deref(), body.reload).await)
}

pub async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(data): Json<AppStateData>,
) -> ApiResult<Json<AppStateData>> {
    state.store.replace(data).await.map_err(ApiError::internal)?;
    state.cache.clear().await;
    Ok(Json(state.store.get().await))
}

pub async fn convert(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConvertRequest>,
) -> ApiResult<Json<ConvertResponse>> {
    let data = state.store.get().await;
    if !data.profile.enabled {
        return Err(ApiError::bad("档案已禁用"));
    }
    let extras = state.world.extras().await;
    match engine::convert(&data, &state.http, &extras).await {
        Ok(result) => {
            let key = engine::config_cache_key(&data, &extras);
            state
                .cache
                .set_full(
                    key,
                    result.yaml.clone(),
                    result.proxy_count,
                    result.fetch_warnings.len(),
                )
                .await;
            let unmatched_samples: Vec<String> =
                result.unmatched.iter().take(12).cloned().collect();
            let unmatched_count = result.unmatched.len();
            let mut message = if matches!(result.groups_mode, crate::model::GroupsMode::Managed) {
                format!(
                    "转换成功（托管）：节点 {} / 组 {} / 规则 {}；识别 {} 个地区，未匹配 {}（补充国家库 {}）",
                    result.proxy_count,
                    result.group_count,
                    result.rule_count,
                    result.regions.iter().filter(|r| r.id != "other").count(),
                    unmatched_count,
                    extras.len()
                )
            } else {
                "转换成功".into()
            };
            if !result.fetch_warnings.is_empty() {
                message.push_str(&format!(
                    "；警告：{} 个上游未完整拉取",
                    result.fetch_warnings.len()
                ));
            }
            Ok(Json(ConvertResponse {
                ok: true,
                proxy_count: result.proxy_count,
                group_count: result.group_count,
                rule_count: result.rule_count,
                yaml: if req.include_yaml {
                    Some(result.yaml)
                } else {
                    None
                },
                message,
                groups_mode: result.groups_mode.as_str().into(),
                regions: result.regions,
                unmatched_count,
                unmatched_samples,
                fetch_warnings: result.fetch_warnings,
            }))
        }
        Err(err) => Ok(Json(ConvertResponse {
            ok: false,
            proxy_count: 0,
            group_count: 0,
            rule_count: 0,
            yaml: None,
            message: err.to_string(),
            groups_mode: data.groups_mode.as_str().into(),
            regions: vec![],
            unmatched_count: 0,
            unmatched_samples: vec![],
            fetch_warnings: vec![],
        })),
    }
}

pub async fn subscription(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let data = state.store.get().await;
    if !data.profile.enabled {
        return Err(ApiError::bad("档案已禁用"));
    }
    if data.profile.urls().is_empty() {
        return Err(ApiError::bad("未配置上游订阅 URL"));
    }
    let extras = state.world.extras().await;
    let key = engine::config_cache_key(&data, &extras);
    let yaml = if let Some(cached) = state.cache.get(&key, Duration::from_secs(60)).await {
        cached
    } else {
        let result = engine::convert(&data, &state.http, &extras)
            .await
            .map_err(ApiError::internal)?;
        state
            .cache
            .set_full(
                key,
                result.yaml.clone(),
                result.proxy_count,
                result.fetch_warnings.len(),
            )
            .await;
        result.yaml
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/yaml; charset=utf-8"),
            (
                header::HeaderName::from_static("profile-update-interval"),
                "12",
            ),
        ],
        yaml,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct AiPlanRequest {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub context_limit: Option<u64>,
    /// 续聊的对话 id；空则自动新建
    #[serde(default)]
    pub chat_id: Option<String>,
    /// 图片 / PDF / 文本 / 音频（Gemini inline_data）
    #[serde(default)]
    pub attachments: Vec<ai::AiAttachment>,
    /// 点选 ask 选项后回传
    #[serde(default)]
    pub choice_id: Option<String>,
    /// true：SSE 流式（思考过程 / 步骤 / 最终结果）
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Deserialize)]
pub struct AiApplyRequest {
    pub ops: Vec<AiOp>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AiTranscribeRequest {
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Deserialize)]
pub struct ListChatsQuery {
    /// true：含已归档；false：仅活跃
    #[serde(default)]
    pub include_archived: bool,
    /// true（默认）：只列主对话，分支不进历史侧栏
    #[serde(default = "default_true")]
    pub roots_only: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct CreateChatBody {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PatchChatBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct BranchChatBody {
    pub message_id: String,
}

pub async fn get_ai_settings(
    State(state): State<Arc<AppState>>,
) -> Json<ai::AiSettingsPublic> {
    Json(state.ai.public().await)
}

pub async fn put_ai_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiSettings>,
) -> ApiResult<Json<ai::AiSettingsPublic>> {
    let pub_settings = state.ai.save(body).await.map_err(ApiError::internal)?;
    Ok(Json(pub_settings))
}

pub async fn get_ai_models(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ai::AiModelsResponse>> {
    let provider = state.ai.effective_provider().await;
    let key = state.ai.effective_key().await;
    let models = if provider == "deepseek" {
        ai::list_deepseek_models(&state.http, &key).await
    } else {
        ai::list_gemini_models(&state.http, &key, state.ai.data_dir()).await
    }
    .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(models))
}

pub async fn get_ai_usage(State(state): State<Arc<AppState>>) -> Json<ai::AiUsagePublic> {
    Json(state.ai.usage_public_live(&state.http).await)
}

pub async fn ai_transcribe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AiTranscribeRequest>,
) -> ApiResult<Json<ai::AiTranscribeResponse>> {
    let provider = state.ai.effective_provider().await;
    if provider == "deepseek" {
        return Err(ApiError::bad(
            "语音转写接口仅支持 Gemini；日常听写走浏览器 Web Speech，不依赖此接口",
        ));
    }
    let key = state.ai.effective_key().await;
    let settings = state.ai.get().await;
    let out = ai::transcribe_with_gemini(
        &state.http,
        &key,
        &settings.model,
        &state.ai,
        &req.mime_type,
        &req.data_base64,
    )
    .await
    .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(out))
}

fn stored_user_prompt(user_prompt: &str, attachments: &[ai::AiAttachment]) -> String {
    let mut stored_prompt = user_prompt.to_string();
    if !attachments.is_empty() {
        let names: Vec<String> = attachments
            .iter()
            .map(|a| {
                let n = a.name.trim();
                if n.is_empty() {
                    a.mime_type.clone()
                } else {
                    n.to_string()
                }
            })
            .collect();
        let tag = format!("[附件: {}]", names.join("、"));
        if stored_prompt.is_empty() {
            stored_prompt = tag;
        } else {
            stored_prompt = format!("{stored_prompt}\n{tag}");
        }
    }
    stored_prompt
}

async fn persist_agent_turn(
    state: &AppState,
    chat_id: &str,
    stored_prompt: &str,
    plan: &mut ai_agent::AiAgentResponse,
    model: &str,
) -> Result<(), ApiError> {
    let history_content = serde_json::json!({
        "kind": plan.kind,
        "summary": plan.summary,
        "ops": plan.ops,
        "options": plan.options,
        "steps": plan.steps,
    })
    .to_string();

    state
        .chats
        .append_turn(
            chat_id,
            stored_prompt,
            &plan.summary,
            &history_content,
            &plan.kind,
            plan.ops.clone(),
            plan.preview.clone(),
            plan.options.clone(),
            plan.steps.clone(),
            &plan.thinking,
            Some(plan.usage.last_prompt_tokens),
            Some(plan.usage.last_output_tokens),
            Some(plan.usage.last_total_tokens),
            plan.usage.context_limit,
            model,
        )
        .await
        .map_err(ApiError::internal)?;
    plan.chat_id = Some(chat_id.to_string());
    Ok(())
}

pub async fn ai_plan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AiPlanRequest>,
) -> Result<Response, ApiError> {
    let key = state.ai.effective_key().await;
    let settings = state.ai.get().await;
    let provider = ai::normalize_provider(&settings.provider);
    let data = state.store.get().await;
    let thinking_enabled = settings.thinking_enabled;

    let chat_id = match req.chat_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => id.to_string(),
        None => state
            .chats
            .create(None, &settings.model)
            .await
            .map_err(ApiError::internal)?
            .id,
    };

    let history = state
        .chats
        .history_for_gemini(&chat_id)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;

    let context_limit = ai::resolve_context_limit(
        settings.context_window,
        &settings.model,
        req.context_limit.unwrap_or(0),
    );

    let mut user_prompt = req.prompt.trim().to_string();
    if let Some(cid) = req.choice_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let label = state
            .chats
            .resolve_choice_label(&chat_id, cid)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?;
        let choice_line = match label {
            Some(l) => format!("我选择了选项「{l}」（choice_id={cid}）"),
            None => format!("我选择了选项 choice_id={cid}"),
        };
        if user_prompt.is_empty() {
            user_prompt = choice_line;
        } else {
            user_prompt = format!("{choice_line}\n{user_prompt}");
        }
    }

    let stored_prompt = stored_user_prompt(&user_prompt, &req.attachments);
    let model = settings.model.clone();
    let attachments = req.attachments.clone();

    if !req.stream {
        let mut plan = ai_agent::run_agent(
            &provider,
            &state.http,
            &key,
            &model,
            &data,
            &state.ai,
            state.world.as_ref(),
            &user_prompt,
            context_limit,
            &history,
            &attachments,
            thinking_enabled,
            None,
        )
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
        persist_agent_turn(&state, &chat_id, &stored_prompt, &mut plan, &model).await?;
        return Ok(Json(plan).into_response());
    }

    let (tx, rx) = mpsc::unbounded_channel::<ai_agent::AgentEvent>();
    // 立刻推一条，尽快刷出首包，避免前端长时间空白
    let _ = tx.send(ai_agent::AgentEvent::Status {
        text: if thinking_enabled {
            "已收到，正在思考…".into()
        } else {
            "已收到，正在查阅配置…".into()
        },
    });
    let state_bg = state.clone();
    let chat_id_bg = chat_id.clone();
    let stored_prompt_bg = stored_prompt.clone();
    let model_bg = model.clone();
    let provider_bg = provider.clone();
    let key_bg = key.clone();
    let user_prompt_bg = user_prompt.clone();
    let history_bg = history.clone();

    tokio::spawn(async move {
        let result = ai_agent::run_agent(
            &provider_bg,
            &state_bg.http,
            &key_bg,
            &model_bg,
            &data,
            &state_bg.ai,
            state_bg.world.as_ref(),
            &user_prompt_bg,
            context_limit,
            &history_bg,
            &attachments,
            thinking_enabled,
            Some(tx.clone()),
        )
        .await;

        match result {
            Ok(mut plan) => {
                if let Err(e) =
                    persist_agent_turn(&state_bg, &chat_id_bg, &stored_prompt_bg, &mut plan, &model_bg)
                        .await
                {
                    let _ = tx.send(ai_agent::AgentEvent::Error {
                        message: e.message.clone(),
                    });
                    return;
                }
                let _ = tx.send(ai_agent::AgentEvent::Result { result: plan });
            }
            Err(e) => {
                let _ = tx.send(ai_agent::AgentEvent::Error {
                    message: e.to_string(),
                });
            }
        }
    });

    let stream = UnboundedReceiverStream::new(rx).map(|ev| {
        let data = serde_json::to_string(&ev).unwrap_or_else(|_| {
            "{\"type\":\"error\",\"message\":\"serialize failed\"}".into()
        });
        Ok::<Event, Infallible>(Event::default().data(data))
    });
    // 结束后再补一条 done，便于前端收尾
    let stream = stream.chain(stream::once(async {
        Ok::<Event, Infallible>(Event::default().data("{\"type\":\"done\"}"))
    }));

    let mut res = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(2))
                .text("ping"),
        )
        .into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(
        header::HeaderName::from_static("x-accel-buffering"),
        header::HeaderValue::from_static("no"),
    );
    Ok(res)
}

pub async fn list_ai_chats(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListChatsQuery>,
) -> Json<serde_json::Value> {
    let chats = state.chats.list(q.include_archived, q.roots_only).await;
    Json(serde_json::json!({ "chats": chats }))
}

pub async fn create_ai_chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateChatBody>,
) -> ApiResult<Json<crate::ai_chats::ChatThread>> {
    let settings = state.ai.get().await;
    let chat = state
        .chats
        .create(body.title, &settings.model)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(chat))
}

pub async fn get_ai_chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::ai_chats::ChatThread>> {
    let chat = state
        .chats
        .get(&id)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(chat))
}

pub async fn patch_ai_chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PatchChatBody>,
) -> ApiResult<Json<crate::ai_chats::ChatThread>> {
    let chat = state
        .chats
        .patch(&id, body.title, body.archived)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(chat))
}

pub async fn delete_ai_chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .chats
        .delete(&id)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn branch_ai_chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<BranchChatBody>,
) -> ApiResult<Json<crate::ai_chats::ChatThread>> {
    let mid = body.message_id.trim();
    if mid.is_empty() {
        return Err(ApiError::bad("message_id 不能为空"));
    }
    let chat = state
        .chats
        .branch(&id, mid)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(chat))
}

pub async fn ai_apply(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AiApplyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.ops.is_empty() {
        return Err(ApiError::bad("没有可应用的变更"));
    }
    let chat_id = req.chat_id.as_deref().unwrap_or("").trim();
    let message_id = req.message_id.as_deref().unwrap_or("").trim();
    if !chat_id.is_empty() && !message_id.is_empty() {
        // 先检查是否已应用，避免重复改配置
        let chat = state
            .chats
            .get(chat_id)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?;
        if let Some(msg) = chat.messages.iter().find(|m| m.id == message_id) {
            if msg.applied_at.is_some() {
                return Err(ApiError::bad("该方案已应用过"));
            }
        }
    }
    let mut data = state.store.get().await;
    let result = ai::apply_ops(&mut data, &req.ops).map_err(|e| ApiError::bad(e.to_string()))?;
    state
        .store
        .replace(data)
        .await
        .map_err(ApiError::internal)?;
    state.cache.clear().await;
    let chat = if !chat_id.is_empty() && !message_id.is_empty() {
        Some(
            state
                .chats
                .mark_applied(chat_id, message_id)
                .await
                .map_err(|e| ApiError::bad(e.to_string()))?,
        )
    } else {
        None
    };
    Ok(Json(serde_json::json!({
        "ok": true,
        "applied": result,
        "message": format!("已应用 {} 项变更", result.len()),
        "chat": chat,
    })))
}

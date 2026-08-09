//! AI 配置助手：自然语言 → 结构化 ops → 预览/应用。
//! 支持供应商：Google Gemini、DeepSeek（OpenAI 兼容 API）。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::model::{
    AppStateData, GroupType, GroupsMode, LandingProxy, LandingType, LanRoute, ProxyGroup,
    RuleSet,
};
use crate::regions::is_user_strategy_group;

const DEFAULT_GEMINI_MODEL: &str = "gemini-2.0-flash";
const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_MODEL: &str = DEFAULT_GEMINI_MODEL;
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const SETTINGS_FILE: &str = "ai.json";
const USAGE_FILE: &str = "ai-usage.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
    /// gemini | deepseek
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Gemini API Key（兼容旧字段名）
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub deepseek_api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// 上下文窗口偏好（tokens）。0 = 跟随模型上限。
    #[serde(default)]
    pub context_window: u64,
    /// 是否开启模型思考模式（DeepSeek thinking / Gemini thinkingConfig）
    #[serde(default)]
    pub thinking_enabled: bool,
    /// 与 platform「累计消费金额」对齐的基数（元）。
    /// 官方 `GET /user/balance` 不含该字段，可在此填入用量页数值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deepseek_spent_sync: Option<f64>,
}

fn default_provider() -> String {
    "gemini".into()
}

fn default_model() -> String {
    DEFAULT_MODEL.into()
}

pub fn normalize_provider(p: &str) -> String {
    match p.trim().to_ascii_lowercase().as_str() {
        "deepseek" => "deepseek".into(),
        _ => "gemini".into(),
    }
}

pub fn default_model_for(provider: &str) -> &'static str {
    if normalize_provider(provider) == "deepseek" {
        DEFAULT_DEEPSEEK_MODEL
    } else {
        DEFAULT_GEMINI_MODEL
    }
}

fn model_fits_provider(provider: &str, model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() {
        return false;
    }
    if normalize_provider(provider) == "deepseek" {
        m.starts_with("deepseek")
    } else {
        !m.starts_with("deepseek")
    }
}

/// 设置里可选的上下文档位（0 = 自动）。
pub const CONTEXT_WINDOW_CHOICES: &[u64] = &[
    0,
    32_768,
    65_536,
    131_072,
    262_144,
    524_288,
    1_048_576,
];

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            api_key: String::new(),
            deepseek_api_key: String::new(),
            model: default_model(),
            context_window: 0,
            thinking_enabled: false,
            deepseek_spent_sync: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AiSettingsPublic {
    pub provider: String,
    pub has_api_key: bool,
    pub api_key_masked: String,
    pub model: String,
    pub source: String,
    /// 0 = 跟随模型
    pub context_window: u64,
    pub context_window_choices: Vec<u64>,
    pub thinking_enabled: bool,
    /// DeepSeek：与 platform 累计消费对齐（元）；未设置时为 null
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepseek_spent_sync: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiUsageState {
    #[serde(default)]
    pub day: String,
    #[serde(default)]
    pub requests_today: u64,
    #[serde(default)]
    pub tokens_today: u64,
    #[serde(default)]
    pub last_prompt_tokens: u64,
    #[serde(default)]
    pub last_output_tokens: u64,
    #[serde(default)]
    pub last_total_tokens: u64,
    #[serde(default)]
    pub last_model: String,
    #[serde(default)]
    pub context_limit: u64,
    /// Unix 秒：此前视为该模型额度耗尽（来自 429）
    #[serde(default)]
    pub quota_blocked_until: u64,
    #[serde(default)]
    pub quota_blocked_model: String,
    /// DeepSeek：观测到的充值本金累计（含历次加充），用于估算累计消费
    #[serde(default)]
    pub deepseek_topped_up_deposited: f64,
    /// 上次观测到的充值余额
    #[serde(default)]
    pub deepseek_topped_up_last: f64,
    #[serde(default)]
    pub deepseek_topped_up_initialized: bool,
    /// DeepSeek：累计消费（元）= 余额池下降累计 + token 计价估算，并与 sync 取较大值
    #[serde(default)]
    pub deepseek_spent_cny: f64,
    /// 上次观测到的可用资金池（充值余额 + 赠金）
    #[serde(default)]
    pub deepseek_pool_last: f64,
    /// 按官方单价从本机调用累计的消费估算（元）
    #[serde(default)]
    pub deepseek_token_cost_cny: f64,
}

/// 按供应商分桶的用量文件（旧版扁平结构会迁移到 gemini）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AiUsageFile {
    #[serde(default)]
    providers: BTreeMap<String, AiUsageState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiUsagePublic {
    pub provider: String,
    pub day: String,
    pub requests_today: u64,
    pub tokens_today: u64,
    pub last_prompt_tokens: u64,
    pub last_output_tokens: u64,
    pub last_total_tokens: u64,
    pub last_model: String,
    pub context_limit: u64,
    pub context_used: u64,
    pub context_pct: f64,
    pub quota_rpm_hint: Option<u64>,
    pub quota_rpd_hint: Option<u64>,
    pub quota_note: String,
    /// 当前是否仍在 429 冷却中
    pub quota_exhausted: bool,
    /// 被限流的模型 id
    pub quota_blocked_model: String,
    /// 距可重试还剩秒数
    pub quota_retry_after_secs: u64,
    /// DeepSeek：`GET /user/balance` 是否足够调用
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_total: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_granted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_topped_up: Option<String>,
    /// DeepSeek：累计消费金额（余额扣减 + token 估算；可与 platform 对齐）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_spent: Option<String>,
    /// DeepSeek：额度预览分母 = 累计消费 + 当前充值余额
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_quota_total: Option<String>,
    /// DeepSeek：额度预览百分比 = spent / (spent + topped_up) × 100
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_quota_pct: Option<f64>,
}

#[derive(Clone)]
pub struct AiStore {
    path: PathBuf,
    usage_path: PathBuf,
    inner: Arc<RwLock<AiSettings>>,
    usage: Arc<RwLock<AiUsageFile>>,
}

impl AiStore {
    pub async fn open(data_dir: impl AsRef<Path>) -> Self {
        let dir = data_dir.as_ref();
        let path = dir.join(SETTINGS_FILE);
        let usage_path = dir.join(USAGE_FILE);
        let settings = load_settings(&path).unwrap_or_default();
        let usage = load_usage_file(&usage_path).unwrap_or_default();
        Self {
            path,
            usage_path,
            inner: Arc::new(RwLock::new(settings)),
            usage: Arc::new(RwLock::new(usage)),
        }
    }

    pub fn data_dir(&self) -> Option<&Path> {
        self.path.parent()
    }

    pub async fn usage_public(&self) -> AiUsagePublic {
        let provider = self.effective_provider().await;
        let file = self.usage.read().await;
        let mut u = file
            .providers
            .get(&provider)
            .cloned()
            .unwrap_or_default();
        roll_usage_day(&mut u);
        to_usage_public(&provider, &u)
    }

    /// 本地累计 +（DeepSeek）拉取官方余额接口。
    pub async fn usage_public_live(&self, http: &reqwest::Client) -> AiUsagePublic {
        let mut usage = self.usage_public().await;
        if usage.provider == "deepseek" {
            let key = self.effective_key().await;
            let spent_sync = self.get().await.deepseek_spent_sync;
            match fetch_deepseek_balance(http, &key).await {
                Ok(bal) => {
                    let mut file = self.usage.write().await;
                    let state = file.providers.entry("deepseek".into()).or_default();
                    apply_deepseek_balance(&mut usage, &bal, state, spent_sync);
                    let _ = save_usage_file(&self.usage_path, &file).await;
                }
                Err(e) => {
                    warn!(error = %e, "拉取 DeepSeek 余额失败，沿用本机累计用量");
                    if usage.quota_note.contains("本机今日累计") {
                        usage.quota_note =
                            format!("本轮 token 来自接口；余额拉取失败：{e}");
                    }
                }
            }
        }
        usage
    }

    pub async fn record_usage(
        &self,
        model: &str,
        prompt_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        context_limit: u64,
    ) -> AiUsagePublic {
        self.record_usage_ex(
            model,
            prompt_tokens,
            output_tokens,
            total_tokens,
            context_limit,
            None,
            None,
        )
        .await
    }

    pub async fn record_usage_ex(
        &self,
        model: &str,
        prompt_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        context_limit: u64,
        cache_hit_tokens: Option<u64>,
        cache_miss_tokens: Option<u64>,
    ) -> AiUsagePublic {
        let provider = provider_for_model(model);
        let mut file = self.usage.write().await;
        let u = file.providers.entry(provider.clone()).or_default();
        roll_usage_day(u);
        u.requests_today = u.requests_today.saturating_add(1);
        u.tokens_today = u.tokens_today.saturating_add(total_tokens.max(prompt_tokens + output_tokens));
        u.last_prompt_tokens = prompt_tokens;
        u.last_output_tokens = output_tokens;
        u.last_total_tokens = if total_tokens > 0 {
            total_tokens
        } else {
            prompt_tokens.saturating_add(output_tokens)
        };
        u.last_model = model.to_string();
        if context_limit > 0 {
            u.context_limit = context_limit;
        }
        if provider == "deepseek" {
            let (hit, miss) = match (cache_hit_tokens, cache_miss_tokens) {
                (Some(h), Some(m)) => (h, m),
                (Some(h), None) => (h, prompt_tokens.saturating_sub(h)),
                (None, Some(m)) => (prompt_tokens.saturating_sub(m), m),
                (None, None) => (0, prompt_tokens),
            };
            let cost = estimate_deepseek_cost_cny(model, hit, miss, output_tokens);
            u.deepseek_token_cost_cny = (u.deepseek_token_cost_cny + cost).max(0.0);
            u.deepseek_spent_cny = u
                .deepseek_spent_cny
                .max(u.deepseek_token_cost_cny)
                .max(0.0);
        }
        // 成功调用后清除该模型的额度耗尽标记
        if !u.quota_blocked_model.is_empty()
            && (u.quota_blocked_model == model || model_matches_block(&u.quota_blocked_model, model))
        {
            u.quota_blocked_until = 0;
            u.quota_blocked_model.clear();
        }
        let out = to_usage_public(&provider, u);
        let _ = save_usage_file(&self.usage_path, &file).await;
        out
    }

    pub async fn mark_quota_exhausted(&self, model: &str, retry_after_secs: u64) -> AiUsagePublic {
        let provider = provider_for_model(model);
        let mut file = self.usage.write().await;
        let u = file.providers.entry(provider.clone()).or_default();
        roll_usage_day(u);
        let now = now_unix();
        let wait = retry_after_secs.max(15).min(3600);
        u.quota_blocked_until = now.saturating_add(wait);
        u.quota_blocked_model = model.trim().to_string();
        if u.last_model.trim().is_empty() {
            u.last_model = u.quota_blocked_model.clone();
        }
        let out = to_usage_public(&provider, u);
        let _ = save_usage_file(&self.usage_path, &file).await;
        out
    }

    pub async fn get(&self) -> AiSettings {
        self.inner.read().await.clone()
    }

    pub async fn public(&self) -> AiSettingsPublic {
        let s = self.get().await;
        let provider = normalize_provider(&s.provider);
        let (key, source) = self.resolve_key_for(&provider, &s);
        let model = if s.model.trim().is_empty() || !model_fits_provider(&provider, &s.model) {
            default_model_for(&provider).into()
        } else {
            s.model
        };
        AiSettingsPublic {
            provider,
            has_api_key: !key.trim().is_empty(),
            api_key_masked: mask_key(&key),
            model,
            source: source.into(),
            context_window: normalize_context_window(s.context_window),
            context_window_choices: CONTEXT_WINDOW_CHOICES.to_vec(),
            thinking_enabled: s.thinking_enabled,
            deepseek_spent_sync: s.deepseek_spent_sync,
        }
    }

    fn resolve_key_for(&self, provider: &str, s: &AiSettings) -> (String, &'static str) {
        if provider == "deepseek" {
            if let Ok(k) = std::env::var("OMNI_DEEPSEEK_API_KEY") {
                if !k.trim().is_empty() {
                    return (k.trim().to_string(), "env");
                }
            }
            (s.deepseek_api_key.trim().to_string(), "file")
        } else if let Ok(k) = std::env::var("OMNI_GEMINI_API_KEY") {
            if !k.trim().is_empty() {
                return (k.trim().to_string(), "env");
            } else {
                (s.api_key.trim().to_string(), "file")
            }
        } else {
            (s.api_key.trim().to_string(), "file")
        }
    }

    pub async fn effective_key(&self) -> String {
        let s = self.get().await;
        let provider = normalize_provider(&s.provider);
        self.resolve_key_for(&provider, &s).0
    }

    pub async fn effective_provider(&self) -> String {
        normalize_provider(&self.get().await.provider)
    }

    pub async fn save(&self, mut next: AiSettings) -> Result<AiSettingsPublic> {
        next.provider = normalize_provider(&next.provider);
        next.context_window = normalize_context_window(next.context_window);
        let cur = self.get().await;

        // 前端只编辑「当前供应商」的 Key，放在 api_key 字段里传来
        let incoming = next.api_key.clone();
        let incoming_ds = next.deepseek_api_key.clone();
        next.api_key = cur.api_key;
        next.deepseek_api_key = cur.deepseek_api_key;

        if !incoming.trim().is_empty() && !looks_masked(&incoming) {
            if next.provider == "deepseek" {
                next.deepseek_api_key = incoming.trim().to_string();
            } else {
                next.api_key = incoming.trim().to_string();
            }
        }
        // 兼容显式传 deepseek_api_key
        if !incoming_ds.trim().is_empty() && !looks_masked(&incoming_ds) {
            next.deepseek_api_key = incoming_ds.trim().to_string();
        }

        if next.model.trim().is_empty() || !model_fits_provider(&next.provider, &next.model) {
            next.model = default_model_for(&next.provider).into();
        }

        // 未传该字段时保留原值；传负数表示清除对齐基数
        next.deepseek_spent_sync = match next.deepseek_spent_sync {
            Some(v) if v < 0.0 => None,
            Some(v) => Some((v * 100.0).round() / 100.0),
            None => cur.deepseek_spent_sync,
        };

        let pretty = serde_json::to_string_pretty(&next).context("序列化 ai.json 失败")?;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(&self.path, pretty)
            .await
            .with_context(|| format!("写入 {} 失败", self.path.display()))?;
        *self.inner.write().await = next;
        Ok(self.public().await)
    }
}

fn normalize_context_window(v: u64) -> u64 {
    if v == 0 {
        return 0;
    }
    CONTEXT_WINDOW_CHOICES
        .iter()
        .copied()
        .filter(|&c| c > 0)
        .min_by_key(|&c| c.abs_diff(v))
        .unwrap_or(131_072)
}

/// 用户偏好与模型上限取较小值；pref=0 表示跟随模型。
pub fn resolve_context_limit(pref: u64, model: &str, model_max_hint: u64) -> u64 {
    let model_max = if model_max_hint > 0 {
        model_max_hint
    } else {
        default_context_limit(model)
    };
    let pref = normalize_context_window(pref);
    if pref == 0 {
        model_max
    } else {
        pref.min(model_max)
    }
}

fn load_settings(path: &Path) -> Option<AiSettings> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn provider_for_model(model: &str) -> String {
    if model.trim().to_ascii_lowercase().starts_with("deepseek") {
        "deepseek".into()
    } else {
        "gemini".into()
    }
}

fn load_usage_file(path: &Path) -> Option<AiUsageFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if v.get("providers").is_some() {
        return serde_json::from_value(v).ok();
    }
    // 旧版扁平结构 → 归入 gemini（历史用量几乎都是 Gemini）
    let legacy: AiUsageState = serde_json::from_value(v).ok()?;
    let mut providers = BTreeMap::new();
    providers.insert("gemini".into(), legacy);
    Some(AiUsageFile { providers })
}

async fn save_usage_file(path: &Path, usage: &AiUsageFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let pretty = serde_json::to_string_pretty(usage).context("序列化 ai-usage.json 失败")?;
    tokio::fs::write(path, pretty)
        .await
        .with_context(|| format!("写入 {} 失败", path.display()))?;
    Ok(())
}

fn today_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn roll_usage_day(u: &mut AiUsageState) {
    let today = today_utc();
    if u.day != today {
        u.day = today;
        u.requests_today = 0;
        u.tokens_today = 0;
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn model_matches_block(blocked: &str, model: &str) -> bool {
    let a = blocked.trim().to_ascii_lowercase();
    let b = model.trim().to_ascii_lowercase();
    !a.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

fn to_usage_public(provider: &str, u: &AiUsageState) -> AiUsagePublic {
    let provider = normalize_provider(provider);
    let context_used = if u.last_total_tokens > 0 {
        u.last_total_tokens
    } else {
        u.last_prompt_tokens.saturating_add(u.last_output_tokens)
    };
    let context_limit = if u.context_limit > 0 {
        u.context_limit
    } else {
        default_context_limit(&u.last_model)
    };
    let context_pct = if context_limit > 0 {
        (context_used as f64 / context_limit as f64) * 100.0
    } else {
        0.0
    };
    let hint_model = if !u.quota_blocked_model.trim().is_empty() {
        u.quota_blocked_model.clone()
    } else if !u.last_model.trim().is_empty() {
        u.last_model.clone()
    } else if provider == "deepseek" {
        DEFAULT_DEEPSEEK_MODEL.to_string()
    } else {
        String::new()
    };
    let (quota_rpm_hint, quota_rpd_hint) = free_tier_quota_hints(&hint_model);
    let now = now_unix();
    let retry_after = u.quota_blocked_until.saturating_sub(now);
    let quota_exhausted = retry_after > 0 && !u.quota_blocked_model.trim().is_empty();
    let quota_note = if quota_exhausted {
        if provider == "deepseek" {
            format!(
                "DeepSeek 模型 {} 限流，约 {} 秒后可重试；可换 deepseek-v4-flash",
                u.quota_blocked_model, retry_after
            )
        } else {
            format!(
                "模型 {} 额度暂时用尽，约 {} 秒后可重试；也可换 flash 模型",
                u.quota_blocked_model, retry_after
            )
        }
    } else if provider == "deepseek" {
        "用量为当前供应商（DeepSeek）本机今日累计；按量计费，RPM 为并发参考".into()
    } else {
        "用量为当前供应商（Gemini）本机今日累计；免费档 RPM/RPD 为参考值".into()
    };
    AiUsagePublic {
        provider,
        day: u.day.clone(),
        requests_today: u.requests_today,
        tokens_today: u.tokens_today,
        last_prompt_tokens: u.last_prompt_tokens,
        last_output_tokens: u.last_output_tokens,
        last_total_tokens: u.last_total_tokens,
        last_model: u.last_model.clone(),
        context_limit,
        context_used,
        context_pct: (context_pct * 10.0).round() / 10.0,
        quota_rpm_hint,
        quota_rpd_hint,
        quota_note,
        quota_exhausted,
        quota_blocked_model: if quota_exhausted {
            u.quota_blocked_model.clone()
        } else {
            String::new()
        },
        quota_retry_after_secs: retry_after,
        balance_available: None,
        balance_currency: None,
        balance_total: None,
        balance_granted: None,
        balance_topped_up: None,
        balance_spent: None,
        balance_quota_total: None,
        balance_quota_pct: None,
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekBalanceResponse {
    #[serde(default)]
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<DeepSeekBalanceInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekBalanceInfo {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    total_balance: String,
    #[serde(default)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

/// 文档：https://api-docs.deepseek.com/api/get-user-balance
async fn fetch_deepseek_balance(
    http: &reqwest::Client,
    api_key: &str,
) -> Result<DeepSeekBalanceResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 DeepSeek API Key");
    }
    let url = format!("{DEEPSEEK_BASE_URL}/user/balance");
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("请求 DeepSeek 余额失败")?;
    let status = resp.status();
    let bytes = resp.bytes().await.context("读取 DeepSeek 余额失败")?;
    if !status.is_success() {
        let msg = String::from_utf8_lossy(&bytes);
        bail!("DeepSeek 余额 HTTP {status}: {msg}");
    }
    serde_json::from_slice(&bytes).context("解析 DeepSeek 余额失败")
}

fn format_balance_amount(currency: &str, amount: &str) -> String {
    let cur = currency.trim().to_ascii_uppercase();
    let amt = amount.trim();
    if cur == "USD" {
        format!("${amt}")
    } else if cur == "CNY" || cur.is_empty() {
        format!("¥{amt}")
    } else {
        format!("{amt} {cur}")
    }
}

fn format_balance_f64(currency: &str, amount: f64) -> String {
    format_balance_amount(currency, &format!("{amount:.2}"))
}

fn parse_balance_f64(raw: &str) -> f64 {
    raw.trim()
        .trim_start_matches(['¥', '$'])
        .parse::<f64>()
        .unwrap_or(0.0)
}

/// 官方单价（元 / 百万 tokens）：https://api-docs.deepseek.com/zh-cn/quick_start/pricing
fn estimate_deepseek_cost_cny(
    model: &str,
    cache_hit: u64,
    cache_miss: u64,
    output_tokens: u64,
) -> f64 {
    let m = model.to_ascii_lowercase();
    let (hit_per_m, miss_per_m, out_per_m) = if m.contains("pro") {
        (0.025, 3.0, 6.0)
    } else {
        // flash / chat / reasoner
        (0.02, 1.0, 2.0)
    };
    (cache_hit as f64) * hit_per_m / 1_000_000.0
        + (cache_miss as f64) * miss_per_m / 1_000_000.0
        + (output_tokens as f64) * out_per_m / 1_000_000.0
}

/// 累计消费：余额池（充值+赠金）下降额 + token 估算，并可与 platform 对齐基数取较大值。
/// 公开 `/user/balance` 不含「累计消费金额」，只能这样逼近。
fn update_deepseek_spend_tracker(
    state: &mut AiUsageState,
    topped_up: f64,
    granted: f64,
    spent_sync: Option<f64>,
) -> (f64, f64, f64) {
    const EPS: f64 = 0.000_1;
    let topped = topped_up.max(0.0);
    let granted = granted.max(0.0);
    let pool = topped + granted;
    let sync = spent_sync.unwrap_or(0.0).max(0.0);

    // 先贴齐 platform 基数，再叠加后续余额下降，避免 max(sync) 吃掉增量
    if sync > state.deepseek_spent_cny + EPS {
        state.deepseek_spent_cny = sync;
    }

    if !state.deepseek_topped_up_initialized {
        state.deepseek_topped_up_deposited = topped;
        state.deepseek_topped_up_last = topped;
        state.deepseek_pool_last = pool;
        state.deepseek_topped_up_initialized = true;
    } else {
        // 旧版只有 topped_up_last、尚无 pool_last 时迁移，避免把整笔余额当成「下降」
        if state.deepseek_pool_last <= EPS && state.deepseek_topped_up_last > EPS {
            state.deepseek_pool_last = state.deepseek_topped_up_last;
        }
        if topped > state.deepseek_topped_up_last + EPS {
            state.deepseek_topped_up_deposited += topped - state.deepseek_topped_up_last;
        }
        state.deepseek_topped_up_last = topped;
        state.deepseek_topped_up_deposited = state.deepseek_topped_up_deposited.max(topped);

        if state.deepseek_pool_last > EPS && pool < state.deepseek_pool_last - EPS {
            state.deepseek_spent_cny += state.deepseek_pool_last - pool;
        }
        state.deepseek_pool_last = pool;
    }

    let spent = state
        .deepseek_spent_cny
        .max(state.deepseek_token_cost_cny)
        .max(0.0);
    state.deepseek_spent_cny = spent;
    let total = spent + topped;
    (spent, topped, total)
}

fn apply_deepseek_balance(
    usage: &mut AiUsagePublic,
    bal: &DeepSeekBalanceResponse,
    state: &mut AiUsageState,
    spent_sync: Option<f64>,
) {
    let info = bal
        .balance_infos
        .iter()
        .find(|b| {
            let c = b.currency.trim().to_ascii_uppercase();
            c == "CNY" || c == "USD"
        })
        .or_else(|| bal.balance_infos.first());
    usage.balance_available = Some(bal.is_available);
    if let Some(info) = info {
        usage.balance_currency = Some(info.currency.clone());
        usage.balance_total = Some(info.total_balance.clone());
        usage.balance_granted = Some(info.granted_balance.clone());
        usage.balance_topped_up = Some(info.topped_up_balance.clone());

        let topped = parse_balance_f64(&info.topped_up_balance);
        let granted = parse_balance_f64(&info.granted_balance);
        let (spent, _remaining, quota_total) =
            update_deepseek_spend_tracker(state, topped, granted, spent_sync);
        usage.balance_spent = Some(format!("{spent:.2}"));
        usage.balance_quota_total = Some(format!("{quota_total:.2}"));
        usage.balance_quota_pct = Some(if quota_total > 0.0 {
            ((spent / quota_total) * 1000.0).round() / 10.0
        } else if !bal.is_available {
            100.0
        } else {
            0.0
        });

        let spent_s = format_balance_f64(&info.currency, spent);
        let total_s = format_balance_f64(&info.currency, quota_total);
        let topped_s = format_balance_amount(&info.currency, &info.topped_up_balance);
        let avail_s = format_balance_amount(&info.currency, &info.total_balance);

        if !bal.is_available {
            usage.quota_exhausted = true;
            if usage.quota_blocked_model.is_empty() {
                usage.quota_note = format!(
                    "DeepSeek 余额不足（可用 {avail_s}）· 已消费 {spent_s} / 总额 {total_s}"
                );
            }
        } else if !usage.quota_exhausted {
            usage.quota_note = format!(
                "DeepSeek 已消费 {spent_s} / 总额 {total_s}（充值余额 {topped_s}）"
            );
        }
    } else if !bal.is_available {
        usage.quota_exhausted = true;
        if usage.quota_blocked_model.is_empty() {
            usage.quota_note = "DeepSeek 余额不足，请到 platform.deepseek.com 充值".into();
        }
    } else if !usage.quota_exhausted {
        usage.quota_note = "DeepSeek 余额可用 · 本轮 token 来自接口".into();
    }
}

fn default_context_limit(model: &str) -> u64 {
    let m = model.to_ascii_lowercase();
    if m.starts_with("deepseek") {
        1_048_576
    } else if m.contains("1.5") {
        1_048_576
    } else if m.contains("flash") || m.contains("pro") {
        1_048_576
    } else {
        128_000
    }
}

/// 免费档常见速率参考（会随官方调整，仅作 UI 提示）。
fn free_tier_quota_hints(model: &str) -> (Option<u64>, Option<u64>) {
    let m = model.to_ascii_lowercase();
    if m.is_empty() {
        return (Some(15), Some(1500));
    }
    // DeepSeek 按量计费 + 并发限速，无 Google 式 RPD；UI 用并发上限作参考
    if m.starts_with("deepseek") {
        if m.contains("pro") {
            return (Some(500), None);
        }
        return (Some(2500), None);
    }
    if m.contains("robotics") || (m.contains("preview") && !m.contains("flash")) {
        (Some(5), Some(20))
    } else if m.contains("flash-lite") {
        (Some(30), Some(1500))
    } else if m.contains("flash") {
        (Some(15), Some(1500))
    } else if m.contains("pro") {
        (Some(2), Some(50))
    } else {
        (Some(15), Some(1500))
    }
}

fn mask_key(key: &str) -> String {
    let t = key.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.len() <= 8 {
        return "****".into();
    }
    format!("{}…{}", &t[..4], &t[t.len().saturating_sub(4)..])
}

fn looks_masked(key: &str) -> bool {
    let t = key.trim();
    t.contains('…') || t.contains("****") || t.chars().all(|c| c == '*')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiOp {
    pub op: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub proxies: Option<Vec<String>>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub rules: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    /// socks5 | http（落地代理）
    #[serde(default)]
    pub landing_type: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// 落地前置策略组/节点（dialer-proxy）
    #[serde(default)]
    pub dialer_proxy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiPlan {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub ops: Vec<AiOp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiPlanResponse {
    pub summary: String,
    pub ops: Vec<AiOp>,
    pub preview: Vec<String>,
    pub usage: AiUsagePublic,
    /// 模型原始 JSON 文本（供对话历史持久化）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    error: Option<GeminiError>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: u64,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: u64,
    #[serde(rename = "totalTokenCount", default)]
    total_token_count: u64,
    #[serde(rename = "thoughtsTokenCount", default)]
    thoughts_token_count: u64,
}

#[derive(Debug, Deserialize)]
struct GeminiError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    parts: Option<Vec<GeminiPart>>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    text: Option<String>,
}

/// 多模态附件（Gemini inline_data，适合较小文件）。
#[derive(Debug, Clone, Deserialize)]
pub struct AiAttachment {
    pub mime_type: String,
    pub data_base64: String,
    #[serde(default)]
    pub name: String,
}

const MAX_ATTACHMENTS: usize = 5;
const MAX_ATTACHMENT_BYTES: usize = 4 * 1024 * 1024;

fn normalize_mime(mime: &str) -> String {
    let m = mime.trim().to_ascii_lowercase();
    match m.as_str() {
        "audio/mp3" => "audio/mpeg".into(),
        "audio/x-wav" => "audio/wav".into(),
        other => other.into(),
    }
}

fn mime_allowed(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    )
}

fn audio_mime_allowed(mime: &str) -> bool {
    matches!(
        mime,
        "audio/wav"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/aac"
            | "audio/flac"
            | "audio/webm"
            | "audio/mp4"
            | "audio/x-m4a"
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct AiTranscribeResponse {
    pub text: String,
    pub usage: AiUsagePublic,
}

/// 语音转写（不走配置方案 system prompt）。只要麦克风开着前端就会持续分片调用。
pub async fn transcribe_with_gemini(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    store: &AiStore,
    mime_type: &str,
    data_base64: &str,
) -> Result<AiTranscribeResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 Gemini API Key。请到 https://aistudio.google.com/apikey 免费创建后填入设置。");
    }
    let mime = normalize_mime(mime_type);
    if !audio_mime_allowed(&mime) {
        bail!("不支持的音频类型：{mime_type}");
    }
    let bytes = decode_attachment_b64(data_base64)?;
    if bytes.is_empty() {
        bail!("音频内容为空");
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        bail!(
            "音频过大（上限 {}MB）",
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        );
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let model = if model.trim().is_empty() {
        DEFAULT_MODEL
    } else {
        model.trim()
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={}",
        urlencoding_key(api_key)
    );

    let body = serde_json::json!({
        "contents": [{
            "role": "user",
            "parts": [
                {
                    "text": "请将这段音频精确转写为文字。只输出转写正文，不要解释、前后缀或引号。若无有效人声则输出空。"
                },
                {
                    "inline_data": {
                        "mime_type": mime,
                        "data": b64
                    }
                }
            ]
        }],
        "generationConfig": {
            "temperature": 0.1,
            "maxOutputTokens": 1024
        }
    });

    let resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(45))
        .json(&body)
        .send()
        .await
        .context("请求 Gemini 转写失败（请检查路由器能否访问 Google API）")?;

    let status = resp.status();
    let raw = resp.bytes().await.context("读取 Gemini 转写响应失败")?;
    let parsed: GeminiResponse =
        serde_json::from_slice(&raw).unwrap_or(GeminiResponse {
            candidates: None,
            error: None,
            usage_metadata: None,
        });

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| String::from_utf8_lossy(&raw).to_string());
        let code = status.as_u16();
        if is_gemini_quota_error(code, &msg) {
            let _ = store
                .mark_quota_exhausted(model, parse_gemini_retry_secs(&msg))
                .await;
        }
        bail!("{}", format_gemini_api_error(code, model, &msg));
    }

    let text = parsed
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .and_then(|c| c.parts.as_ref())
        .and_then(|p| p.first())
        .and_then(|p| p.text.as_ref())
        .cloned()
        .unwrap_or_default();

    let text = text
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '“' || c == '”')
        .to_string();

    let meta = parsed.usage_metadata.unwrap_or_default();
    let prompt_tokens = meta.prompt_token_count;
    let output_tokens = meta
        .candidates_token_count
        .saturating_add(meta.thoughts_token_count);
    let total_tokens = if meta.total_token_count > 0 {
        meta.total_token_count
    } else {
        prompt_tokens.saturating_add(output_tokens)
    };
    let usage = store
        .record_usage(
            model,
            prompt_tokens,
            output_tokens,
            total_tokens,
            default_context_limit(model),
        )
        .await;

    Ok(AiTranscribeResponse { text, usage })
}

fn decode_attachment_b64(raw: &str) -> Result<Vec<u8>> {
    let s = raw.trim();
    let s = if let Some(i) = s.find("base64,") {
        &s[i + 7..]
    } else {
        s
    };
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(s))
        .context("附件 Base64 无效")
}

pub(crate) fn validate_attachments(
    attachments: &[AiAttachment],
) -> Result<Vec<(String, String, String)>> {
    if attachments.len() > MAX_ATTACHMENTS {
        bail!("最多上传 {MAX_ATTACHMENTS} 个附件");
    }
    let mut out = Vec::with_capacity(attachments.len());
    for a in attachments {
        let mime = normalize_mime(&a.mime_type);
        if !mime_allowed(&mime) {
            bail!("不支持的附件类型：{}", a.mime_type);
        }
        let bytes = decode_attachment_b64(&a.data_base64)?;
        if bytes.is_empty() {
            bail!("附件内容为空");
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            bail!(
                "附件「{}」过大（上限 {}MB）",
                if a.name.trim().is_empty() {
                    &mime
                } else {
                    a.name.trim()
                },
                MAX_ATTACHMENT_BYTES / (1024 * 1024)
            );
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let name = a.name.trim().to_string();
        out.push((mime, b64, name));
    }
    Ok(out)
}

/// `history`：既有多轮 `(role, text)`，role 为 `user` / `model`（不含本轮用户输入）。
pub async fn plan_with_gemini(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    data: &AppStateData,
    user_prompt: &str,
    store: &AiStore,
    context_limit: u64,
    history: &[(String, String)],
    attachments: &[AiAttachment],
) -> Result<AiPlanResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 Gemini API Key。请到 https://aistudio.google.com/apikey 免费创建后填入设置。");
    }
    let prompt = user_prompt.trim();
    let media = validate_attachments(attachments)?;
    if prompt.is_empty() && media.is_empty() {
        bail!("请输入需求描述，或上传/录制附件");
    }

    let model = if model.trim().is_empty() {
        DEFAULT_MODEL
    } else {
        model.trim()
    };

    let context = build_context(data);
    let system = SYSTEM_PROMPT;
    let need = if prompt.is_empty() {
        "（用户未写文字，请根据附件中的截图理解需求）"
    } else {
        prompt
    };
    let user_text = format!(
        "当前配置摘要（JSON）：\n{context}\n\n用户需求：\n{need}\n\n请只输出一个 JSON 对象，不要 Markdown。"
    );

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={}",
        urlencoding_key(api_key)
    );

    let mut contents: Vec<serde_json::Value> = Vec::with_capacity(history.len() + 1);
    for (role, text) in history {
        let role = match role.as_str() {
            "model" | "assistant" => "model",
            _ => "user",
        };
        if text.trim().is_empty() {
            continue;
        }
        contents.push(serde_json::json!({
            "role": role,
            "parts": [{ "text": text }]
        }));
    }

    let mut parts: Vec<serde_json::Value> = Vec::with_capacity(1 + media.len());
    parts.push(serde_json::json!({ "text": user_text }));
    for (mime, b64, _) in &media {
        parts.push(serde_json::json!({
            "inline_data": {
                "mime_type": mime,
                "data": b64
            }
        }));
    }
    contents.push(serde_json::json!({
        "role": "user",
        "parts": parts
    }));

    let body = serde_json::json!({
        "systemInstruction": {
            "parts": [{ "text": system }]
        },
        "contents": contents,
        "generationConfig": {
            "temperature": 0.2,
            "responseMimeType": "application/json"
        }
    });

    let timeout_secs = if media.is_empty() { 45 } else { 90 };
    let resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(timeout_secs))
        .json(&body)
        .send()
        .await
        .context("请求 Gemini 失败（请检查路由器能否访问 Google API）")?;

    let status = resp.status();
    let bytes = resp.bytes().await.context("读取 Gemini 响应失败")?;
    let parsed: GeminiResponse =
        serde_json::from_slice(&bytes).unwrap_or(GeminiResponse {
            candidates: None,
            error: None,
            usage_metadata: None,
        });

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).to_string());
        let code = status.as_u16();
        if is_gemini_quota_error(code, &msg) {
            let _ = store
                .mark_quota_exhausted(model, parse_gemini_retry_secs(&msg))
                .await;
        }
        bail!("{}", format_gemini_api_error(code, model, &msg));
    }

    let text = parsed
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .and_then(|c| c.parts.as_ref())
        .and_then(|p| p.first())
        .and_then(|p| p.text.as_ref())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Gemini 未返回文本"))?;

    let meta = parsed.usage_metadata.unwrap_or_default();
    let prompt_tokens = meta.prompt_token_count;
    let output_tokens = meta
        .candidates_token_count
        .saturating_add(meta.thoughts_token_count);
    let total_tokens = if meta.total_token_count > 0 {
        meta.total_token_count
    } else {
        prompt_tokens.saturating_add(output_tokens)
    };
    let limit = if context_limit > 0 {
        context_limit
    } else {
        default_context_limit(model)
    };
    let usage = store
        .record_usage(model, prompt_tokens, output_tokens, total_tokens, limit)
        .await;

    let plan = parse_plan_json(&text)?;
    let ops = prepare_ops(data, &plan.ops).context("模型方案与当前配置不一致，已拒绝")?;
    let preview = preview_ops(&ops);
    info!(
        ops = ops.len(),
        prompt_tokens,
        output_tokens,
        total_tokens,
        "Gemini 配置方案已生成"
    );
    Ok(AiPlanResponse {
        summary: if plan.summary.trim().is_empty() {
            "已生成配置变更方案".into()
        } else {
            plan.summary
        },
        ops,
        preview,
        usage,
        raw: text,
        chat_id: None,
    })
}

/// 按当前供应商分发配置方案生成。
pub async fn plan_with_provider(
    provider: &str,
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    data: &AppStateData,
    user_prompt: &str,
    store: &AiStore,
    context_limit: u64,
    history: &[(String, String)],
    attachments: &[AiAttachment],
) -> Result<AiPlanResponse> {
    if normalize_provider(provider) == "deepseek" {
        plan_with_deepseek(
            http,
            api_key,
            model,
            data,
            user_prompt,
            store,
            context_limit,
            history,
            attachments,
        )
        .await
    } else {
        plan_with_gemini(
            http,
            api_key,
            model,
            data,
            user_prompt,
            store,
            context_limit,
            history,
            attachments,
        )
        .await
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    choices: Option<Vec<OpenAiChoice>>,
    usage: Option<OpenAiUsage>,
    error: Option<OpenAiError>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiMessage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct OpenAiError {
    message: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    r#type: Option<String>,
}

/// DeepSeek Chat Completions（OpenAI 兼容），文档：https://api-docs.deepseek.com/zh-cn/
pub async fn plan_with_deepseek(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    data: &AppStateData,
    user_prompt: &str,
    store: &AiStore,
    context_limit: u64,
    history: &[(String, String)],
    attachments: &[AiAttachment],
) -> Result<AiPlanResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 DeepSeek API Key。请到 https://platform.deepseek.com/api_keys 创建后填入设置。");
    }
    if !attachments.is_empty() {
        bail!("DeepSeek 方案生成暂不支持图片附件，请去掉附件或改用 Gemini。");
    }
    let prompt = user_prompt.trim();
    if prompt.is_empty() {
        bail!("请输入需求描述");
    }

    let model = if model.trim().is_empty() || !model_fits_provider("deepseek", model) {
        DEFAULT_DEEPSEEK_MODEL
    } else {
        model.trim()
    };

    let context = build_context(data);
    let user_text = format!(
        "当前配置摘要（JSON）：\n{context}\n\n用户需求：\n{prompt}\n\n请只输出一个 JSON 对象，不要 Markdown。"
    );

    let mut messages: Vec<serde_json::Value> = Vec::with_capacity(history.len() + 2);
    messages.push(serde_json::json!({
        "role": "system",
        "content": SYSTEM_PROMPT
    }));
    for (role, text) in history {
        let role = match role.as_str() {
            "model" | "assistant" => "assistant",
            _ => "user",
        };
        if text.trim().is_empty() {
            continue;
        }
        messages.push(serde_json::json!({
            "role": role,
            "content": text
        }));
    }
    messages.push(serde_json::json!({
        "role": "user",
        "content": user_text
    }));

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": 0.2,
        "response_format": { "type": "json_object" },
        "thinking": { "type": "disabled" },
        "stream": false
    });

    let url = format!("{DEEPSEEK_BASE_URL}/chat/completions");
    let resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .timeout(Duration::from_secs(90))
        .json(&body)
        .send()
        .await
        .context("请求 DeepSeek 失败（请检查路由器能否访问 api.deepseek.com）")?;

    let status = resp.status();
    let bytes = resp.bytes().await.context("读取 DeepSeek 响应失败")?;
    let parsed: OpenAiChatResponse =
        serde_json::from_slice(&bytes).unwrap_or(OpenAiChatResponse {
            choices: None,
            usage: None,
            error: None,
        });

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).to_string());
        let code = status.as_u16();
        if code == 429 {
            let _ = store.mark_quota_exhausted(model, 60).await;
            bail!(
                "DeepSeek 请求过于频繁或并发已满（模型 {model}）。请稍后重试，或在设置中换 deepseek-v4-flash。"
            );
        }
        if code == 401 || code == 403 {
            bail!("DeepSeek API Key 无效或无权访问：{msg}");
        }
        if code == 402 {
            bail!("DeepSeek 余额不足，请到 platform.deepseek.com 充值后再试。");
        }
        bail!("DeepSeek HTTP {code}: {msg}");
    }

    let text = parsed
        .choices
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.message.as_ref())
        .and_then(|m| m.content.as_ref())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("DeepSeek 未返回文本"))?;

    let usage_raw = parsed.usage.unwrap_or_default();
    let prompt_tokens = usage_raw.prompt_tokens;
    let output_tokens = usage_raw.completion_tokens;
    let total_tokens = if usage_raw.total_tokens > 0 {
        usage_raw.total_tokens
    } else {
        prompt_tokens.saturating_add(output_tokens)
    };
    let limit = if context_limit > 0 {
        context_limit
    } else {
        default_context_limit(model)
    };
    let _ = store
        .record_usage(model, prompt_tokens, output_tokens, total_tokens, limit)
        .await;
    let usage = store.usage_public_live(http).await;

    let plan = parse_plan_json(&text)?;
    let ops = prepare_ops(data, &plan.ops).context("模型方案与当前配置不一致，已拒绝")?;
    let preview = preview_ops(&ops);
    info!(
        ops = ops.len(),
        prompt_tokens,
        output_tokens,
        total_tokens,
        "DeepSeek 配置方案已生成"
    );
    Ok(AiPlanResponse {
        summary: if plan.summary.trim().is_empty() {
            "已生成配置变更方案".into()
        } else {
            plan.summary
        },
        ops,
        preview,
        usage,
        raw: text,
        chat_id: None,
    })
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Option<Vec<OpenAiModelItem>>,
    error: Option<OpenAiError>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelItem {
    id: Option<String>,
}

fn deepseek_model_meta(id: &str) -> (String, u64, u64) {
    let id_l = id.to_ascii_lowercase();
    let display = if id_l == "deepseek-v4-flash" {
        "DeepSeek V4 Flash".into()
    } else if id_l == "deepseek-v4-pro" {
        "DeepSeek V4 Pro".into()
    } else if id_l == "deepseek-chat" {
        "DeepSeek Chat".into()
    } else if id_l == "deepseek-reasoner" {
        "DeepSeek Reasoner".into()
    } else {
        id.to_string()
    };
    // 官方文档：V4 上下文 1M、输出最大 384K；其它别名沿用同档
    (display, 1_048_576, 384_000)
}

fn fallback_deepseek_models() -> Vec<AiModelInfo> {
    ["deepseek-v4-flash", "deepseek-v4-pro"]
        .into_iter()
        .map(|id| {
            let (display_name, input_token_limit, output_token_limit) = deepseek_model_meta(id);
            AiModelInfo {
                id: id.into(),
                display_name,
                tier: "paid".into(),
                tier_label: "按量".into(),
                input_token_limit,
                output_token_limit,
            }
        })
        .collect()
}

/// 从 DeepSeek `GET /models` 拉取可用模型（OpenAI 兼容）。
/// 文档：https://api-docs.deepseek.com/api/list-models
pub async fn list_deepseek_models(
    http: &reqwest::Client,
    api_key: &str,
) -> Result<AiModelsResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 DeepSeek API Key，无法拉取模型列表");
    }
    let url = format!("{DEEPSEEK_BASE_URL}/models");
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("请求 DeepSeek 模型列表失败（请检查路由器能否访问 api.deepseek.com）")?;
    let status = resp.status();
    let bytes = resp.bytes().await.context("读取 DeepSeek 模型列表失败")?;
    let parsed: OpenAiModelsResponse =
        serde_json::from_slice(&bytes).unwrap_or(OpenAiModelsResponse {
            data: None,
            error: None,
        });
    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).to_string());
        bail!("DeepSeek 模型列表 HTTP {status}: {msg}");
    }

    let mut models: Vec<AiModelInfo> = parsed
        .data
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            let id = m.id.unwrap_or_default().trim().to_string();
            if id.is_empty() {
                return None;
            }
            let id_l = id.to_ascii_lowercase();
            if !id_l.starts_with("deepseek") {
                return None;
            }
            let (display_name, input_token_limit, output_token_limit) = deepseek_model_meta(&id);
            Some(AiModelInfo {
                id,
                display_name,
                tier: "paid".into(),
                tier_label: "按量".into(),
                input_token_limit,
                output_token_limit,
            })
        })
        .collect();

    if models.is_empty() {
        warn!("DeepSeek /models 返回空列表，使用内置回退");
        models = fallback_deepseek_models();
    } else {
        models.sort_by(|a, b| {
            let rank = |id: &str| -> u8 {
                let m = id.to_ascii_lowercase();
                if m.contains("flash") {
                    0
                } else if m.contains("pro") {
                    1
                } else if m.contains("chat") {
                    2
                } else if m.contains("reasoner") {
                    3
                } else {
                    4
                }
            };
            rank(&a.id)
                .cmp(&rank(&b.id))
                .then_with(|| a.display_name.cmp(&b.display_name))
        });
    }

    Ok(AiModelsResponse { models })
}

fn parse_gemini_retry_secs(raw: &str) -> u64 {
    raw.split("Please retry in ")
        .nth(1)
        .and_then(|s| s.split('s').next())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|secs| secs.ceil() as u64)
        .unwrap_or(60)
        .clamp(15, 3600)
}

fn is_gemini_quota_error(status: u16, raw: &str) -> bool {
    status == 429 || raw.contains("Too Many Requests") || raw.contains("RESOURCE_EXHAUSTED")
}

fn format_gemini_api_error(status: u16, model: &str, raw: &str) -> String {
    let model = if model.trim().is_empty() {
        DEFAULT_MODEL
    } else {
        model.trim()
    };
    if is_gemini_quota_error(status, raw) {
        let retry = parse_gemini_retry_secs(raw);
        let preview_hint = if model.contains("preview") || model.contains("robotics") {
            "该预览/专用模型免费额度很低；"
        } else {
            ""
        };
        return format!(
            "Gemini 免费额度已用尽（模型 {model}）。{preview_hint}约 {retry} 秒后可重试。建议换成 gemini-2.0-flash 或 gemini-2.5-flash 后再发（与提示词无关）。"
        );
    }
    format!("Gemini HTTP {status}: {raw}")
}

fn urlencoding_key(key: &str) -> String {
    // API key 通常为安全字符；简单编码
    key.trim()
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u8),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct AiModelInfo {
    /// 调用用 id（不含 models/ 前缀）
    pub id: String,
    pub display_name: String,
    /// free | paid | unknown —— 相对「免费 API Key / 官方 Free Tier」
    pub tier: String,
    /// 角标文案：免费 / 付费 / 未知
    pub tier_label: String,
    #[serde(default)]
    pub input_token_limit: u64,
    #[serde(default)]
    pub output_token_limit: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiModelsResponse {
    pub models: Vec<AiModelInfo>,
}

#[derive(Debug, Deserialize)]
struct GeminiModelsResponse {
    models: Option<Vec<GeminiModel>>,
    error: Option<GeminiError>,
}

#[derive(Debug, Deserialize)]
struct GeminiModel {
    name: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "supportedGenerationMethods")]
    supported_generation_methods: Option<Vec<String>>,
    #[serde(rename = "inputTokenLimit", default)]
    input_token_limit: u64,
    #[serde(rename = "outputTokenLimit", default)]
    output_token_limit: u64,
}

const PRICING_CACHE_FILE: &str = "gemini-pricing-cache.json";
const PRICING_URL: &str = "https://ai.google.dev/gemini-api/docs/pricing";
const PRICING_TTL_SECS: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PricingCache {
    fetched_at: u64,
    /// model_id -> true=免费 Key 可用（Free Tier Input free of charge）
    free_tier: std::collections::HashMap<String, bool>,
}

/// 内置对照（官方定价页摘要；网络不可达时仍可标注）。
fn builtin_tier_map() -> std::collections::HashMap<String, bool> {
    let free = [
        "gemini-3.6-flash",
        "gemini-3.5-flash",
        "gemini-3.5-flash-lite",
        "gemini-3.5-live-translate-preview",
        "gemini-3.1-flash-lite",
        "gemini-3.1-flash-live-preview",
        "gemini-3.1-flash-tts-preview",
        "gemini-3-flash-preview",
        "gemini-2.5-pro",
        "gemini-2.5-flash",
        "gemini-2.5-flash-lite",
        "gemini-2.5-flash-lite-preview-09-2025",
        "gemini-2.5-flash-native-audio-preview-12-2025",
        "gemini-2.5-flash-preview-tts",
        "gemini-2.0-flash",
        "gemini-2.0-flash-lite",
        "gemini-1.5-flash",
        "gemini-1.5-pro",
        "gemini-robotics-er-1.6-preview",
        "gemma-3-27b-it",
        "gemma-3-12b-it",
        "gemma-3-4b-it",
        "gemma-3-1b-it",
    ];
    let paid = [
        "gemini-omni-flash-preview",
        "gemini-3.1-pro-preview",
        "gemini-3-pro-preview",
        "gemini-3.1-flash-image",
        "gemini-3.1-flash-image-preview",
        "gemini-3.1-flash-lite-image",
        "gemini-3-pro-image",
        "gemini-3-pro-image-preview",
        "gemini-2.5-flash-image",
        "gemini-2.5-flash-image-preview",
        "gemini-2.5-pro-preview-tts",
        "gemini-2.5-computer-use-preview-10-2025",
        "gemini-2.5-computer-use-preview",
    ];
    let mut m = std::collections::HashMap::new();
    for id in free {
        m.insert(id.to_string(), true);
    }
    for id in paid {
        m.insert(id.to_string(), false);
    }
    m
}

fn tier_strings(free: Option<bool>) -> (String, String) {
    match free {
        Some(true) => ("free".into(), "免费".into()),
        Some(false) => ("paid".into(), "付费".into()),
        None => ("unknown".into(), "未知".into()),
    }
}

fn lookup_free_tier(id: &str, catalog: &std::collections::HashMap<String, bool>) -> Option<bool> {
    if let Some(v) = catalog.get(id) {
        return Some(*v);
    }
    // 最长前缀：gemini-2.0-flash-001 → gemini-2.0-flash
    let mut best: Option<(&str, bool)> = None;
    for (k, v) in catalog {
        if id == k || id.starts_with(&format!("{k}-")) {
            let len = k.len();
            if best.map(|(bk, _)| len > bk.len()).unwrap_or(true) {
                best = Some((k.as_str(), *v));
            }
        }
    }
    if let Some((_, v)) = best {
        return Some(v);
    }
    // 启发式：图像/计算机使用类多为付费；常见 flash 偏免费
    let lower = id.to_ascii_lowercase();
    if lower.contains("image") || lower.contains("computer-use") || lower.contains("imagen") {
        return Some(false);
    }
    if lower.contains("flash-lite") || (lower.contains("flash") && !lower.contains("omni")) {
        return Some(true);
    }
    None
}

fn parse_pricing_tiers(body: &str) -> std::collections::HashMap<String, bool> {
    // 在每个 `model-id` 后找最近的 Standard「Input price」单元格（Free of charge / Not available）
    let mut out = std::collections::HashMap::new();
    let re_id = regex::Regex::new(r"`((?:gemini|gemma|lyria)-[a-z0-9.\-]+)`").ok();
    let Some(re_id) = re_id else {
        return out;
    };
    let ids: Vec<(usize, String)> = re_id
        .captures_iter(body)
        .filter_map(|c| {
            let whole = c.get(0)?;
            let id = c.get(1)?.as_str().to_string();
            Some((whole.start(), id))
        })
        .collect();
    for (i, (start, id)) in ids.iter().enumerate() {
        let end = ids.get(i + 1).map(|(s, _)| *s).unwrap_or(body.len());
        let chunk = &body[*start..end];
        // 优先 Standard 段
        let std_start = chunk.find("### Standard").or_else(|| chunk.find(">Standard<"));
        let search = if let Some(s) = std_start {
            &chunk[s..]
        } else {
            chunk
        };
        let lower = search.to_ascii_lowercase();
        // 找 input price 行附近
        let idx = lower
            .find("input price")
            .or_else(|| lower.find("| input price"));
        let Some(idx) = idx else {
            continue;
        };
        let window = &lower[idx..idx.saturating_add(220).min(lower.len())];
        if window.contains("free of charge") {
            out.insert(id.clone(), true);
        } else if window.contains("not available") {
            out.insert(id.clone(), false);
        }
    }
    out
}

async fn load_tier_catalog(http: &reqwest::Client, cache_dir: Option<&Path>) -> PricingCache {
    let path = cache_dir.map(|d| d.join(PRICING_CACHE_FILE));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(ref p) = path {
        if let Ok(raw) = tokio::fs::read_to_string(p).await {
            if let Ok(c) = serde_json::from_str::<PricingCache>(&raw) {
                if now.saturating_sub(c.fetched_at) < PRICING_TTL_SECS && !c.free_tier.is_empty() {
                    return c;
                }
            }
        }
    }

    let mut catalog = builtin_tier_map();
    match http
        .get(PRICING_URL)
        .header("User-Agent", "omni-acl4ssr-agent/0.1")
        .timeout(Duration::from_secs(20))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(text) = resp.text().await {
                let parsed = parse_pricing_tiers(&text);
                if !parsed.is_empty() {
                    for (k, v) in parsed {
                        catalog.insert(k, v);
                    }
                    info!(n = catalog.len(), "已刷新 Gemini 定价免费/付费对照");
                }
            }
        }
        Ok(resp) => warn!(status = %resp.status(), "拉取 Gemini 定价页失败，使用内置对照"),
        Err(e) => warn!(error = %e, "拉取 Gemini 定价页失败，使用内置对照"),
    }

    let cache = PricingCache {
        fetched_at: now,
        free_tier: catalog,
    };
    if let Some(ref p) = path {
        if let Ok(pretty) = serde_json::to_string_pretty(&cache) {
            let _ = tokio::fs::write(p, pretty).await;
        }
    }
    cache
}

/// 从 Gemini ListModels 拉取支持 generateContent 的模型，并标注免费/付费。
pub async fn list_gemini_models(
    http: &reqwest::Client,
    api_key: &str,
    cache_dir: Option<&Path>,
) -> Result<AiModelsResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 Gemini API Key，无法拉取模型列表");
    }
    let tiers = load_tier_catalog(http, cache_dir).await;

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models?key={}&pageSize=100",
        urlencoding_key(api_key)
    );
    let resp = http
        .get(&url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("请求 Gemini 模型列表失败")?;
    let status = resp.status();
    let bytes = resp.bytes().await.context("读取模型列表失败")?;
    let parsed: GeminiModelsResponse =
        serde_json::from_slice(&bytes).unwrap_or(GeminiModelsResponse {
            models: None,
            error: None,
        });
    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).to_string());
        bail!("Gemini 模型列表 HTTP {status}: {msg}");
    }

    let mut models: Vec<AiModelInfo> = parsed
        .models
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            m.supported_generation_methods
                .as_ref()
                .map(|methods| methods.iter().any(|x| x == "generateContent"))
                .unwrap_or(false)
        })
        .filter_map(|m| {
            let raw = m.name.unwrap_or_default();
            let id = raw.strip_prefix("models/").unwrap_or(&raw).trim().to_string();
            if id.is_empty() {
                return None;
            }
            // 排除明显非对话/嵌入类（若名称含 embedding）
            if id.to_ascii_lowercase().contains("embedding") {
                return None;
            }
            let display_name = m
                .display_name
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| id.clone());
            let free = lookup_free_tier(&id, &tiers.free_tier);
            let (tier, tier_label) = tier_strings(free);
            let input_token_limit = if m.input_token_limit > 0 {
                m.input_token_limit
            } else {
                default_context_limit(&id)
            };
            Some(AiModelInfo {
                id,
                display_name,
                tier,
                tier_label,
                input_token_limit,
                output_token_limit: m.output_token_limit,
            })
        })
        .collect();

    models.sort_by(|a, b| {
        let ra = match a.tier.as_str() {
            "free" => 0,
            "unknown" => 1,
            _ => 2,
        };
        let rb = match b.tier.as_str() {
            "free" => 0,
            "unknown" => 1,
            _ => 2,
        };
        ra.cmp(&rb).then_with(|| a.display_name.cmp(&b.display_name))
    });
    if models.is_empty() {
        let (tier, tier_label) = tier_strings(Some(true));
        models.push(AiModelInfo {
            id: DEFAULT_MODEL.into(),
            display_name: "Gemini 2.0 Flash".into(),
            tier,
            tier_label,
            input_token_limit: default_context_limit(DEFAULT_MODEL),
            output_token_limit: 8192,
        });
    }
    Ok(AiModelsResponse { models })
}

fn parse_plan_json(text: &str) -> Result<AiPlan> {
    let trimmed = text.trim();
    let json_str = if let Some(start) = trimmed.find('{') {
        let end = trimmed.rfind('}').unwrap_or(trimmed.len() - 1);
        &trimmed[start..=end]
    } else {
        trimmed
    };
    serde_json::from_str(json_str).with_context(|| format!("解析 Gemini JSON 失败: {json_str}"))
}

pub fn build_context(data: &AppStateData) -> String {
    let user_groups: Vec<_> = data
        .groups
        .iter()
        .filter(|g| is_user_strategy_group(g))
        .map(|g| {
            serde_json::json!({
                "id": g.id,
                "name": g.name,
                "proxies": g.proxies,
            })
        })
        .collect();
    let rulesets: Vec<_> = data
        .rulesets
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "name": r.name,
                "group": r.group,
                "enabled": r.enabled,
                "rules": r.rules,
            })
        })
        .collect();
    let lan: Vec<_> = data
        .lan_routes
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "name": r.name,
                "src": r.src,
                "target": r.target,
                "enabled": r.enabled,
            })
        })
        .collect();
    let landings: Vec<_> = data
        .landings
        .iter()
        .map(|l| {
            serde_json::json!({
                "id": l.id,
                "name": l.name,
                "landing_type": l.landing_type.as_clash(),
                "server": l.server,
                "port": l.port,
                "username": l.username,
                "password_set": !l.password.trim().is_empty(),
                "dialer_proxy": l.dialer_proxy,
                "enabled": l.enabled,
            })
        })
        .collect();
    serde_json::json!({
        "groups_mode": data.groups_mode.as_str(),
        "user_strategy_groups": user_groups,
        "rulesets": rulesets,
        "lan_routes": lan,
        "landings": landings,
        "known_region_group_names_hint": [
            "🇭🇰 香港","🇹🇼 台湾","🇯🇵 日本","🇸🇬 新加坡","🇰🇷 韩国","🇺🇸 美国",
            "🇬🇧 英国","🇩🇪 德国","🇫🇷 法国","🇨🇦 加拿大","🇦🇺 澳大利亚","🌐 其他",
            "🤖 AI","💰 币安","📺 奈飞","⛓ 链路","🚀 默认","DIRECT","REJECT"
        ]
    })
    .to_string()
}

const SYSTEM_PROMPT: &str = r#"你是 omni-acl4ssr-agent（OpenWrt 本地 Mihomo 订阅转换）的配置助手。
根据用户需求，输出对「用户策略组 / 规则集 / 局域网分流 / 落地代理」的变更计划。可在一次回复中组合多种 ops。

硬性规则：
1. 只输出一个 JSON 对象，字段：summary (string), ops (array)。不要 Markdown 代码块。
2. 不要修改国家/地区自动托管组（带 filter 的 url-test 组）；不要删除 g-default、g-chain。
3. groups_mode=managed 时，用户策略组是没有 filter 的 select 组（如 AI/币安/奈飞或用户自建）；proxies[] 应使用配置摘要里 known_region_group_names_hint 的精确名称（含 emoji），并按用户要求的先后顺序排列。
4. 规则集 rules 为多行文本，每行一条 Clash 规则载荷（通常不含策略名），如 DOMAIN-SUFFIX,tradingview.com；策略组写在 group 字段（一般对应用户策略组名）。
5. 局域网分流 src 为 IP 或 CIDR；target 为策略组名或 DIRECT/REJECT。
6. 落地代理为 SOCKS5/HTTP 节点：landing_type 为 socks5 或 http；dialer_proxy 可选（前置策略组/节点名，空表示独立落地）。摘要里 password_set=true 表示已有密码；更新时未给出 password 字段则保留原密码。
7. 删除/更新时尽量使用配置摘要里已有的 id；若只有名称也可填 name。
8. 所有 proxies[] / group / target / dialer_proxy 必须是：配置摘要里已有的精确组名、本批 ops 里即将 add_group 的 name，或 DIRECT/REJECT。地区组必须带 emoji 前缀（如 🇭🇰 香港），禁止虚构不存在的组名或 id。
9. 若摘要里已有同名项，用 update_*；没有则用 add_*。禁止对不存在的 id 做 update/delete。
10. 没有需要变更时 ops 可为 []，summary 说明原因。服务端会校验整批 ops 能否在真实配置上顺序执行；校验失败不会把虚假方案交给用户。

示例（TradingView）：应产出 add_group（proxies 用摘要中的 🇭🇰 香港/🇹🇼 台湾/🇯🇵 日本/🇰🇷 韩国）+ add_ruleset（group 为新组名，rules 含 DOMAIN-SUFFIX,tradingview.com）；已有同名则 update_*。

ops.op 取值与字段：
- add_group: name, proxies[]
- update_group: id 或 name, 可选 name/proxies
- delete_group: id 或 name
- add_ruleset: name, group, rules, 可选 enabled
- update_ruleset: id 或 name, 可选 name/group/rules/enabled
- delete_ruleset: id 或 name
- add_lan_route: src, target, 可选 name/enabled
- update_lan_route: id 或 name, 可选 name/src/target/enabled
- delete_lan_route: id 或 name
- add_landing: name, server, 可选 landing_type/port/username/password/dialer_proxy/enabled
- update_landing: id 或 name, 可选 name/landing_type/server/port/username/password/dialer_proxy/enabled
- delete_landing: id 或 name
"#;

const REGION_NAME_HINTS: &[&str] = &[
    "🇭🇰 香港",
    "🇹🇼 台湾",
    "🇯🇵 日本",
    "🇸🇬 新加坡",
    "🇰🇷 韩国",
    "🇺🇸 美国",
    "🇬🇧 英国",
    "🇩🇪 德国",
    "🇫🇷 法国",
    "🇨🇦 加拿大",
    "🇦🇺 澳大利亚",
    "🌐 其他",
    "🤖 AI",
    "💰 币安",
    "📺 奈飞",
    "⛓ 链路",
    "🚀 默认",
];

fn op_rank(op: &str) -> u8 {
    match op {
        "add_group" | "update_group" => 0,
        "delete_group" => 1,
        "add_ruleset" | "update_ruleset" => 2,
        "delete_ruleset" => 3,
        "add_lan_route" | "update_lan_route" => 4,
        "delete_lan_route" => 5,
        "add_landing" | "update_landing" => 6,
        "delete_landing" => 7,
        _ => 9,
    }
}

fn core_group_label(s: &str) -> String {
    s.chars()
        .filter(|c| {
            c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c) || *c == '_'
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn fuzzy_match_name<'a>(raw: &str, names: impl Iterator<Item = &'a str>) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let list: Vec<&str> = names.collect();
    if let Some(n) = list.iter().find(|n| **n == t) {
        return Some((*n).to_string());
    }
    let core = core_group_label(t);
    if core.is_empty() {
        return None;
    }
    let hits: Vec<&str> = list
        .iter()
        .copied()
        .filter(|n| {
            let c = core_group_label(n);
            !c.is_empty() && (c == core || c.contains(&core) || core.contains(&c))
        })
        .collect();
    if hits.len() == 1 {
        Some(hits[0].to_string())
    } else {
        None
    }
}

/// 将策略组/节点引用规范为真实可用名称。
fn resolve_proxy_ref(
    data: &AppStateData,
    pending_groups: &HashSet<String>,
    raw: &str,
) -> Result<String> {
    let t = raw.trim();
    if t.is_empty() {
        bail!("策略组/节点引用为空");
    }
    if t.eq_ignore_ascii_case("direct") {
        return Ok("DIRECT".into());
    }
    if t.eq_ignore_ascii_case("reject") {
        return Ok("REJECT".into());
    }
    if pending_groups.iter().any(|p| p == t) {
        return Ok(t.to_string());
    }
    if let Some(g) = data.groups.iter().find(|g| g.name == t) {
        return Ok(g.name.clone());
    }
    if let Some(n) = fuzzy_match_name(t, data.groups.iter().map(|g| g.name.as_str())) {
        return Ok(n);
    }
    if let Some(n) = fuzzy_match_name(t, pending_groups.iter().map(|s| s.as_str())) {
        return Ok(n);
    }
    if let Some(n) = fuzzy_match_name(t, REGION_NAME_HINTS.iter().copied()) {
        if let Some(g) = data.groups.iter().find(|g| g.name == n) {
            return Ok(g.name.clone());
        }
        // 托管地区组可能因无节点暂未出现在 groups；proxies 仍允许写标准名
        return Ok(n);
    }
    bail!(
        "无效引用「{t}」。须为当前配置中的组名、本批将新增的组名，或 DIRECT/REJECT（地区组如 🇭🇰 香港）"
    );
}

fn group_exists(data: &AppStateData, op: &AiOp) -> bool {
    resolve_group(data, op).is_ok()
}

fn ruleset_exists(data: &AppStateData, op: &AiOp) -> bool {
    resolve_ruleset(data, op).is_ok()
}

fn lan_exists(data: &AppStateData, op: &AiOp) -> bool {
    resolve_lan(data, op).is_ok()
}

fn landing_exists(data: &AppStateData, op: &AiOp) -> bool {
    resolve_landing(data, op).is_ok()
}

fn normalize_one_op(
    data: &AppStateData,
    pending_groups: &HashSet<String>,
    op: &AiOp,
) -> Result<AiOp> {
    let mut op = op.clone();
    // 丢弃虚构 id，改为按 name/src 匹配
    if let Some(id) = op.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let exists = match op.op.as_str() {
            "update_group" | "delete_group" => data.groups.iter().any(|g| g.id == id),
            "update_ruleset" | "delete_ruleset" => data.rulesets.iter().any(|r| r.id == id),
            "update_lan_route" | "delete_lan_route" => data.lan_routes.iter().any(|r| r.id == id),
            "update_landing" | "delete_landing" => data.landings.iter().any(|l| l.id == id),
            _ => true,
        };
        if !exists {
            op.id = None;
        }
    }

    match op.op.as_str() {
        "update_group" if !group_exists(data, &op) => {
            if op.name.as_deref().unwrap_or("").trim().is_empty() {
                bail!("update_group 指向不存在的策略组，且缺少可新建的 name");
            }
            op.op = "add_group".into();
            op.id = None;
        }
        "update_ruleset" if !ruleset_exists(data, &op) => {
            if op.name.as_deref().unwrap_or("").trim().is_empty()
                || op.group.as_deref().unwrap_or("").trim().is_empty()
                || op.rules.as_deref().unwrap_or("").trim().is_empty()
            {
                bail!("update_ruleset 指向不存在的规则集，且缺少 add_ruleset 所需字段");
            }
            op.op = "add_ruleset".into();
            op.id = None;
        }
        "update_lan_route" if !lan_exists(data, &op) => {
            if op.src.as_deref().unwrap_or("").trim().is_empty()
                || op.target.as_deref().unwrap_or("").trim().is_empty()
            {
                bail!("update_lan_route 指向不存在的分流，且缺少 src/target");
            }
            op.op = "add_lan_route".into();
            op.id = None;
        }
        "update_landing" if !landing_exists(data, &op) => {
            if op.name.as_deref().unwrap_or("").trim().is_empty()
                || op.server.as_deref().unwrap_or("").trim().is_empty()
            {
                bail!("update_landing 指向不存在的落地，且缺少 name/server");
            }
            op.op = "add_landing".into();
            op.id = None;
        }
        _ => {}
    }

    if let Some(px) = op.proxies.take() {
        let mut out = Vec::with_capacity(px.len());
        for p in px {
            out.push(resolve_proxy_ref(data, pending_groups, &p)?);
        }
        op.proxies = Some(out);
    }
    if let Some(g) = op.group.take() {
        op.group = Some(resolve_proxy_ref(data, pending_groups, &g)?);
    }
    if let Some(t) = op.target.take() {
        op.target = Some(resolve_proxy_ref(data, pending_groups, &t)?);
    }
    if let Some(d) = op.dialer_proxy.take() {
        if d.trim().is_empty() {
            op.dialer_proxy = Some(String::new());
        } else {
            op.dialer_proxy = Some(resolve_proxy_ref(data, pending_groups, &d)?);
        }
    }
    Ok(op)
}

fn normalize_ops(data: &AppStateData, ops: &[AiOp]) -> Result<Vec<AiOp>> {
    let mut indexed: Vec<(usize, AiOp)> = ops.iter().cloned().enumerate().collect();
    indexed.sort_by_key(|(i, op)| (op_rank(&op.op), *i));
    let mut pending = HashSet::new();
    let mut out = Vec::with_capacity(indexed.len());
    for (_, op) in indexed {
        let nop = normalize_one_op(data, &pending, &op)?;
        if nop.op == "add_group" {
            if let Some(n) = nop.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                pending.insert(n.to_string());
            }
        }
        out.push(nop);
    }
    Ok(out)
}

fn dry_run_ops(data: &AppStateData, ops: &[AiOp]) -> Result<()> {
    let mut sim = data.clone();
    let _ = collapse_duplicate_config(&mut sim);
    for (i, op) in ops.iter().enumerate() {
        apply_one(&mut sim, op)
            .with_context(|| format!("ops[{}] 无法在当前配置上执行", i + 1))?;
    }
    Ok(())
}

/// 规范化 + 顺序模拟校验；返回可安全应用的 ops。
pub fn prepare_ops(data: &AppStateData, ops: &[AiOp]) -> Result<Vec<AiOp>> {
    if ops.is_empty() {
        return Ok(vec![]);
    }
    let ops = normalize_ops(data, ops)?;
    dry_run_ops(data, &ops)?;
    Ok(ops)
}

#[allow(dead_code)]
pub fn validate_ops(data: &AppStateData, ops: &[AiOp]) -> Result<()> {
    prepare_ops(data, ops).map(|_| ())
}

fn parse_landing_type(raw: &str) -> Result<LandingType> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "socks5" | "socks" => Ok(LandingType::Socks5),
        "http" | "https" => Ok(LandingType::Http),
        other => bail!("不支持的落地类型: {other}（仅 socks5/http）"),
    }
}

fn looks_like_src(src: &str) -> bool {
    if src.contains('/') {
        return true;
    }
    let parts: Vec<&str> = src.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) {
        return true;
    }
    src.contains(':') // ipv6 rough
}

fn resolve_group<'a>(data: &'a AppStateData, op: &AiOp) -> Result<&'a ProxyGroup> {
    if let Some(id) = op.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(g) = data.groups.iter().find(|g| g.id == id) {
            return Ok(g);
        }
    }
    if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(g) = data.groups.iter().find(|g| g.name == name) {
            return Ok(g);
        }
        if let Some(n) = fuzzy_match_name(name, data.groups.iter().map(|g| g.name.as_str())) {
            if let Some(g) = data.groups.iter().find(|g| g.name == n) {
                return Ok(g);
            }
        }
    }
    let hint = op
        .id
        .as_deref()
        .or(op.name.as_deref())
        .unwrap_or("")
        .trim();
    if hint.is_empty() {
        bail!("未找到策略组（缺少 id/name）");
    }
    bail!("未找到策略组「{hint}」")
}

fn resolve_ruleset<'a>(data: &'a AppStateData, op: &AiOp) -> Result<&'a RuleSet> {
    if let Some(id) = op.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(r) = data.rulesets.iter().find(|r| r.id == id) {
            return Ok(r);
        }
    }
    if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(r) = data.rulesets.iter().find(|r| r.name == name) {
            return Ok(r);
        }
    }
    bail!("未找到规则集")
}

fn resolve_lan<'a>(data: &'a AppStateData, op: &AiOp) -> Result<&'a LanRoute> {
    if let Some(id) = op.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(r) = data.lan_routes.iter().find(|r| r.id == id) {
            return Ok(r);
        }
    }
    if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(r) = data.lan_routes.iter().find(|r| r.name == name) {
            return Ok(r);
        }
    }
    if let Some(src) = op.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(r) = data.lan_routes.iter().find(|r| r.src == src) {
            return Ok(r);
        }
    }
    bail!("未找到局域网分流")
}

fn resolve_landing<'a>(data: &'a AppStateData, op: &AiOp) -> Result<&'a LandingProxy> {
    if let Some(id) = op.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(l) = data.landings.iter().find(|l| l.id == id) {
            return Ok(l);
        }
    }
    if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(l) = data.landings.iter().find(|l| l.name == name) {
            return Ok(l);
        }
    }
    bail!("未找到落地代理")
}

pub fn preview_ops(ops: &[AiOp]) -> Vec<String> {
    ops.iter()
        .map(|op| match op.op.as_str() {
            "add_group" => format!(
                "新增策略组「{}」成员 {}",
                op.name.as_deref().unwrap_or("?"),
                op.proxies
                    .as_ref()
                    .map(|p| p.join(", "))
                    .unwrap_or_default()
            ),
            "update_group" => format!(
                "更新策略组 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .unwrap_or("?")
            ),
            "delete_group" => format!(
                "删除策略组 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .unwrap_or("?")
            ),
            "add_ruleset" => format!(
                "新增规则集「{}」→ {}",
                op.name.as_deref().unwrap_or("?"),
                op.group.as_deref().unwrap_or("?")
            ),
            "update_ruleset" => format!(
                "更新规则集 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .unwrap_or("?")
            ),
            "delete_ruleset" => format!(
                "删除规则集 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .unwrap_or("?")
            ),
            "add_lan_route" => format!(
                "新增局域网 {} → {}",
                op.src.as_deref().unwrap_or("?"),
                op.target.as_deref().unwrap_or("?")
            ),
            "update_lan_route" => format!(
                "更新局域网分流 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .or(op.src.as_deref())
                    .unwrap_or("?")
            ),
            "delete_lan_route" => format!(
                "删除局域网分流 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .or(op.src.as_deref())
                    .unwrap_or("?")
            ),
            "add_landing" => format!(
                "新增落地「{}」{}:{}{}",
                op.name.as_deref().unwrap_or("?"),
                op.server.as_deref().unwrap_or("?"),
                op.port.unwrap_or(1080),
                op.dialer_proxy
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|d| format!(" · 前置 {d}"))
                    .unwrap_or_default()
            ),
            "update_landing" => format!(
                "更新落地代理 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .unwrap_or("?")
            ),
            "delete_landing" => format!(
                "删除落地代理 {}",
                op.id
                    .as_deref()
                    .or(op.name.as_deref())
                    .unwrap_or("?")
            ),
            other => format!("未知操作 {other}"),
        })
        .collect()
}

/// 折叠重复配置：同名策略组/规则集/落地、同 src 局域网只保留首次出现。
fn collapse_duplicate_config(data: &mut AppStateData) -> Vec<String> {
    let mut notes = Vec::new();
    let before_g = data.groups.len();
    let mut seen_g = HashSet::new();
    data.groups.retain(|g| {
        let k = g.name.trim().to_string();
        k.is_empty() || seen_g.insert(k)
    });
    let n = before_g.saturating_sub(data.groups.len());
    if n > 0 {
        notes.push(format!("已清理 {n} 个重复策略组"));
    }

    let before_r = data.rulesets.len();
    let mut seen_r = HashSet::new();
    data.rulesets.retain(|r| {
        let k = r.name.trim().to_string();
        k.is_empty() || seen_r.insert(k)
    });
    let n = before_r.saturating_sub(data.rulesets.len());
    if n > 0 {
        notes.push(format!("已清理 {n} 个重复规则集"));
    }

    let before_l = data.lan_routes.len();
    let mut seen_lan = HashSet::new();
    data.lan_routes.retain(|r| {
        let k = r.src.trim().to_string();
        k.is_empty() || seen_lan.insert(k)
    });
    let n = before_l.saturating_sub(data.lan_routes.len());
    if n > 0 {
        notes.push(format!("已清理 {n} 条重复局域网分流"));
    }

    let before_ld = data.landings.len();
    let mut seen_ld = HashSet::new();
    data.landings.retain(|l| {
        let k = l.name.trim().to_string();
        k.is_empty() || seen_ld.insert(k)
    });
    let n = before_ld.saturating_sub(data.landings.len());
    if n > 0 {
        notes.push(format!("已清理 {n} 个重复落地代理"));
    }
    notes
}

pub fn apply_ops(data: &mut AppStateData, ops: &[AiOp]) -> Result<Vec<String>> {
    let ops = prepare_ops(data, ops)?;
    let mut applied = collapse_duplicate_config(data);
    for op in &ops {
        applied.push(apply_one(data, op)?);
    }
    applied.extend(collapse_duplicate_config(data));
    Ok(applied)
}

fn apply_one(data: &mut AppStateData, op: &AiOp) -> Result<String> {
    match op.op.as_str() {
        "add_group" => {
            let name = op
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_group 缺少 name"))?
                .to_string();
            let proxies = op.proxies.clone().unwrap_or_else(|| vec!["DIRECT".into()]);
            if let Some(g) = data.groups.iter_mut().find(|g| g.name.trim() == name) {
                if matches!(data.groups_mode, GroupsMode::Managed) && !g.filter.trim().is_empty() {
                    bail!("策略组「{name}」为国家托管组，不能覆盖");
                }
                g.proxies = proxies;
                Ok(format!("已更新同名策略组「{name}」（未重复新增）"))
            } else {
                let id = format!("g-{}", &Uuid::new_v4().to_string()[..8]);
                data.groups.push(ProxyGroup {
                    id: id.clone(),
                    name: name.clone(),
                    group_type: GroupType::Select,
                    filter: String::new(),
                    proxies,
                    url: "https://www.gstatic.com/generate_204".into(),
                    interval: 300,
                    tolerance: 50,
                    lazy: true,
                });
                Ok(format!("已新增策略组「{name}」({id})"))
            }
        }
        "update_group" => {
            let id = resolve_group(data, op)?.id.clone();
            if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if data.groups.iter().any(|x| x.id != id && x.name.trim() == name) {
                    bail!("已存在同名策略组「{name}」，无法重命名");
                }
            }
            if let Some(g) = data.groups.iter_mut().find(|g| g.id == id) {
                if matches!(data.groups_mode, GroupsMode::Managed) && !g.filter.trim().is_empty() {
                    bail!("托管模式下禁止修改国家组「{}」", g.name);
                }
                if matches!(g.id.as_str(), "g-default" | "g-chain") && op.name.is_some() {
                    // 允许改 proxies，但下面按字段更新
                }
                if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    g.name = name.into();
                }
                if let Some(proxies) = &op.proxies {
                    g.proxies = proxies.clone();
                }
                Ok(format!("已更新策略组「{}」", g.name))
            } else {
                bail!("未找到策略组")
            }
        }
        "delete_group" => {
            let g = resolve_group(data, op)?;
            let id = g.id.clone();
            let name = g.name.clone();
            if matches!(id.as_str(), "g-default" | "g-chain") {
                bail!("禁止删除 {id}");
            }
            if matches!(data.groups_mode, GroupsMode::Managed) && !g.filter.trim().is_empty() {
                bail!("托管模式下禁止删除国家组「{name}」");
            }
            data.groups.retain(|x| x.id != id);
            Ok(format!("已删除策略组「{name}」"))
        }
        "add_ruleset" => {
            let name = op
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_ruleset 缺少 name"))?
                .to_string();
            let group = op
                .group
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_ruleset 缺少 group"))?
                .to_string();
            let rules = op
                .rules
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_ruleset 缺少 rules"))?
                .to_string();
            // group 必须已存在，或为本批已写入的用户组
            if group != "DIRECT"
                && group != "REJECT"
                && !data.groups.iter().any(|g| g.name == group)
            {
                bail!("规则集指向的策略组「{group}」不存在");
            }
            if let Some(r) = data.rulesets.iter_mut().find(|r| r.name.trim() == name) {
                r.group = group;
                r.rules = rules;
                if let Some(en) = op.enabled {
                    r.enabled = en;
                }
                Ok(format!("已更新同名规则集「{name}」（未重复新增）"))
            } else {
                let id = format!("r-{}", &Uuid::new_v4().to_string()[..8]);
                data.rulesets.push(RuleSet {
                    id: id.clone(),
                    name: name.clone(),
                    group,
                    rules,
                    enabled: op.enabled.unwrap_or(true),
                });
                Ok(format!("已新增规则集「{name}」({id})"))
            }
        }
        "update_ruleset" => {
            let id = resolve_ruleset(data, op)?.id.clone();
            if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if data.rulesets.iter().any(|x| x.id != id && x.name.trim() == name) {
                    bail!("已存在同名规则集「{name}」，无法重命名");
                }
            }
            if let Some(group) = op.group.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if group != "DIRECT"
                    && group != "REJECT"
                    && !data.groups.iter().any(|g| g.name == group)
                {
                    bail!("规则集指向的策略组「{group}」不存在");
                }
            }
            if let Some(r) = data.rulesets.iter_mut().find(|r| r.id == id) {
                if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    r.name = name.into();
                }
                if let Some(group) = op.group.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    r.group = group.into();
                }
                if let Some(rules) = &op.rules {
                    r.rules = rules.clone();
                }
                if let Some(en) = op.enabled {
                    r.enabled = en;
                }
                Ok(format!("已更新规则集「{}」", r.name))
            } else {
                bail!("未找到规则集")
            }
        }
        "delete_ruleset" => {
            let id = resolve_ruleset(data, op)?.id.clone();
            let name = resolve_ruleset(data, op)?.name.clone();
            data.rulesets.retain(|r| r.id != id);
            Ok(format!("已删除规则集「{name}」"))
        }
        "add_lan_route" => {
            let src = op
                .src
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_lan_route 缺少 src"))?
                .to_string();
            if !looks_like_src(&src) {
                bail!("src 格式无效: {src}");
            }
            let target = op
                .target
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_lan_route 缺少 target"))?
                .to_string();
            if target != "DIRECT"
                && target != "REJECT"
                && !data.groups.iter().any(|g| g.name == target)
            {
                bail!("局域网分流目标策略组「{target}」不存在");
            }
            let name = op
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("")
                .to_string();
            if let Some(r) = data.lan_routes.iter_mut().find(|r| r.src.trim() == src) {
                if !name.is_empty() {
                    r.name = name;
                }
                r.target = target;
                if let Some(en) = op.enabled {
                    r.enabled = en;
                }
                Ok(format!("已更新同 src 局域网分流 {src}（未重复新增）"))
            } else {
                let id = format!("lan-{}", &Uuid::new_v4().to_string()[..8]);
                data.lan_routes.push(LanRoute {
                    id: id.clone(),
                    name,
                    src: src.clone(),
                    target,
                    enabled: op.enabled.unwrap_or(true),
                });
                Ok(format!("已新增局域网分流 {src} ({id})"))
            }
        }
        "update_lan_route" => {
            let id = resolve_lan(data, op)?.id.clone();
            if let Some(src) = op.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if !looks_like_src(src) {
                    bail!("src 格式无效: {src}");
                }
                if data.lan_routes.iter().any(|x| x.id != id && x.src.trim() == src) {
                    bail!("已存在相同源地址的局域网分流 {src}");
                }
            }
            if let Some(target) = op.target.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if target != "DIRECT"
                    && target != "REJECT"
                    && !data.groups.iter().any(|g| g.name == target)
                {
                    bail!("局域网分流目标策略组「{target}」不存在");
                }
            }
            if let Some(r) = data.lan_routes.iter_mut().find(|r| r.id == id) {
                if let Some(name) = op.name.as_deref() {
                    r.name = name.trim().into();
                }
                if let Some(src) = op.src.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    r.src = src.into();
                }
                if let Some(target) = op.target.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    r.target = target.into();
                }
                if let Some(en) = op.enabled {
                    r.enabled = en;
                }
                Ok(format!("已更新局域网分流 {}", r.src))
            } else {
                bail!("未找到局域网分流")
            }
        }
        "delete_lan_route" => {
            let id = resolve_lan(data, op)?.id.clone();
            let src = resolve_lan(data, op)?.src.clone();
            data.lan_routes.retain(|r| r.id != id);
            Ok(format!("已删除局域网分流 {src}"))
        }
        "add_landing" => {
            let name = op
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_landing 缺少 name"))?
                .to_string();
            let server = op
                .server
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("add_landing 缺少 server"))?
                .to_string();
            let landing_type = op
                .landing_type
                .as_deref()
                .map(parse_landing_type)
                .transpose()?
                .unwrap_or(LandingType::Socks5);
            let port = op.port.unwrap_or(1080);
            if port == 0 {
                bail!("port 无效");
            }
            if let Some(d) = op.dialer_proxy.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if d != "DIRECT"
                    && d != "REJECT"
                    && !data.groups.iter().any(|g| g.name == d)
                {
                    bail!("落地前置策略组「{d}」不存在");
                }
            }
            if let Some(l) = data.landings.iter_mut().find(|l| l.name.trim() == name) {
                l.landing_type = landing_type;
                l.server = server.clone();
                l.port = port;
                if let Some(username) = &op.username {
                    l.username = username.clone();
                }
                if let Some(password) = &op.password {
                    l.password = password.clone();
                }
                if let Some(dialer) = &op.dialer_proxy {
                    l.dialer_proxy = dialer.trim().into();
                }
                if let Some(en) = op.enabled {
                    l.enabled = en;
                }
                Ok(format!("已更新同名落地代理「{name}」（未重复新增）"))
            } else {
                let id = format!("l-{}", &Uuid::new_v4().to_string()[..8]);
                data.landings.push(LandingProxy {
                    id: id.clone(),
                    name: name.clone(),
                    landing_type,
                    server: server.clone(),
                    port,
                    username: op.username.clone().unwrap_or_default(),
                    password: op.password.clone().unwrap_or_default(),
                    dialer_proxy: op
                        .dialer_proxy
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or("")
                        .to_string(),
                    enabled: op.enabled.unwrap_or(true),
                });
                Ok(format!("已新增落地代理「{name}」({id}) {server}:{port}"))
            }
        }
        "update_landing" => {
            let id = resolve_landing(data, op)?.id.clone();
            if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if data.landings.iter().any(|x| x.id != id && x.name.trim() == name) {
                    bail!("已存在同名落地代理「{name}」，无法重命名");
                }
            }
            if let Some(d) = op.dialer_proxy.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if d != "DIRECT"
                    && d != "REJECT"
                    && !data.groups.iter().any(|g| g.name == d)
                {
                    bail!("落地前置策略组「{d}」不存在");
                }
            }
            if let Some(l) = data.landings.iter_mut().find(|l| l.id == id) {
                if let Some(name) = op.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    l.name = name.into();
                }
                if let Some(t) = op.landing_type.as_deref() {
                    l.landing_type = parse_landing_type(t)?;
                }
                if let Some(server) = op.server.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    l.server = server.into();
                }
                if let Some(port) = op.port {
                    if port == 0 {
                        bail!("port 无效");
                    }
                    l.port = port;
                }
                if let Some(username) = &op.username {
                    l.username = username.clone();
                }
                if let Some(password) = &op.password {
                    l.password = password.clone();
                }
                if let Some(dialer) = &op.dialer_proxy {
                    l.dialer_proxy = dialer.trim().into();
                }
                if let Some(en) = op.enabled {
                    l.enabled = en;
                }
                Ok(format!("已更新落地代理「{}」", l.name))
            } else {
                bail!("未找到落地代理")
            }
        }
        "delete_landing" => {
            let id = resolve_landing(data, op)?.id.clone();
            let name = resolve_landing(data, op)?.name.clone();
            data.landings.retain(|l| l.id != id);
            Ok(format!("已删除落地代理「{name}」"))
        }
        other => bail!("未知 op: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_normalizes_region_aliases_and_orders_ops() {
        let data = AppStateData::default_skeleton();
        let ops = vec![
            AiOp {
                op: "add_ruleset".into(),
                name: Some("TradingView".into()),
                group: Some("TradingView".into()),
                rules: Some("DOMAIN-SUFFIX,tradingview.com".into()),
                id: None,
                proxies: None,
                enabled: None,
                src: None,
                target: None,
                landing_type: None,
                server: None,
                port: None,
                username: None,
                password: None,
                dialer_proxy: None,
            },
            AiOp {
                op: "add_group".into(),
                name: Some("TradingView".into()),
                proxies: Some(vec![
                    "香港".into(),
                    "台湾".into(),
                    "日本".into(),
                    "韩国".into(),
                ]),
                id: None,
                group: None,
                rules: None,
                enabled: None,
                src: None,
                target: None,
                landing_type: None,
                server: None,
                port: None,
                username: None,
                password: None,
                dialer_proxy: None,
            },
        ];
        let prepared = prepare_ops(&data, &ops).unwrap();
        assert_eq!(prepared[0].op, "add_group");
        assert_eq!(
            prepared[0].proxies.as_ref().unwrap(),
            &vec![
                "🇭🇰 香港".to_string(),
                "🇹🇼 台湾".to_string(),
                "🇯🇵 日本".to_string(),
                "🇰🇷 韩国".to_string()
            ]
        );
        assert_eq!(prepared[1].op, "add_ruleset");
    }

    #[test]
    fn prepare_rejects_fake_group_reference() {
        let data = AppStateData::default_skeleton();
        let ops = vec![AiOp {
            op: "add_ruleset".into(),
            name: Some("X".into()),
            group: Some("完全不存在的组".into()),
            rules: Some("DOMAIN-SUFFIX,x.com".into()),
            id: None,
            proxies: None,
            enabled: None,
            src: None,
            target: None,
            landing_type: None,
            server: None,
            port: None,
            username: None,
            password: None,
            dialer_proxy: None,
        }];
        assert!(prepare_ops(&data, &ops).is_err());
    }

    #[test]
    fn add_group_and_ruleset_are_idempotent() {
        let mut data = AppStateData::default_skeleton();
        let text = r#"{"summary":"ok","ops":[
          {"op":"add_group","name":"TradingView","proxies":["🇭🇰 香港","🇹🇼 台湾"]},
          {"op":"add_ruleset","name":"TradingView","group":"TradingView","rules":"DOMAIN-SUFFIX,tradingview.com"}
        ]}"#;
        let plan = parse_plan_json(text).unwrap();
        apply_ops(&mut data, &plan.ops).unwrap();
        apply_ops(&mut data, &plan.ops).unwrap();
        let groups: Vec<_> = data
            .groups
            .iter()
            .filter(|g| g.name == "TradingView")
            .collect();
        let rules: Vec<_> = data
            .rulesets
            .iter()
            .filter(|r| r.name == "TradingView")
            .collect();
        assert_eq!(groups.len(), 1);
        assert_eq!(rules.len(), 1);
        assert_eq!(groups[0].proxies, vec!["🇭🇰 香港", "🇹🇼 台湾"]);
    }

    #[test]
    fn collapse_removes_existing_duplicates() {
        let mut data = AppStateData::default_skeleton();
        data.groups.push(ProxyGroup {
            id: "g-a".into(),
            name: "TradingView".into(),
            group_type: GroupType::Select,
            filter: String::new(),
            proxies: vec!["🇭🇰 香港".into()],
            url: String::new(),
            interval: 300,
            tolerance: 50,
            lazy: true,
        });
        data.groups.push(ProxyGroup {
            id: "g-b".into(),
            name: "TradingView".into(),
            group_type: GroupType::Select,
            filter: String::new(),
            proxies: vec!["🇯🇵 日本".into()],
            url: String::new(),
            interval: 300,
            tolerance: 50,
            lazy: true,
        });
        data.rulesets.push(RuleSet {
            id: "r-a".into(),
            name: "TradingView".into(),
            group: "TradingView".into(),
            rules: "DOMAIN-SUFFIX,tradingview.com".into(),
            enabled: true,
        });
        data.rulesets.push(RuleSet {
            id: "r-b".into(),
            name: "TradingView".into(),
            group: "TradingView".into(),
            rules: "DOMAIN-SUFFIX,tradingview.com".into(),
            enabled: true,
        });
        let notes = collapse_duplicate_config(&mut data);
        assert!(notes.iter().any(|n| n.contains("策略组")));
        assert!(notes.iter().any(|n| n.contains("规则集")));
        assert_eq!(
            data.groups.iter().filter(|g| g.name == "TradingView").count(),
            1
        );
        assert_eq!(
            data.rulesets
                .iter()
                .filter(|r| r.name == "TradingView")
                .count(),
            1
        );
        assert_eq!(
            data.groups
                .iter()
                .find(|g| g.name == "TradingView")
                .unwrap()
                .id,
            "g-a"
        );
    }

    #[test]
    fn parses_plan_and_validates_add_lan() {
        let mut data = AppStateData::default_skeleton();
        let text = r#"{"summary":"ok","ops":[{"op":"add_lan_route","src":"172.16.1.50","target":"📺 奈飞","name":"电视"}]}"#;
        let plan = parse_plan_json(text).unwrap();
        validate_ops(&data, &plan.ops).unwrap();
        let applied = apply_ops(&mut data, &plan.ops).unwrap();
        assert_eq!(data.lan_routes.len(), 1);
        assert!(applied[0].contains("172.16.1.50"));
    }

    #[test]
    fn rejects_delete_default() {
        let data = AppStateData::default_skeleton();
        let ops = vec![AiOp {
            op: "delete_group".into(),
            id: Some("g-default".into()),
            name: None,
            proxies: None,
            group: None,
            rules: None,
            enabled: None,
            src: None,
            target: None,
            landing_type: None,
            server: None,
            port: None,
            username: None,
            password: None,
            dialer_proxy: None,
        }];
        assert!(validate_ops(&data, &ops).is_err());
    }

    #[test]
    fn applies_add_landing() {
        let mut data = AppStateData::default_skeleton();
        let text = r#"{"summary":"ok","ops":[{"op":"add_landing","name":"家宽SOCKS","landing_type":"socks5","server":"127.0.0.1","port":1080,"dialer_proxy":"🇭🇰 香港"}]}"#;
        let plan = parse_plan_json(text).unwrap();
        validate_ops(&data, &plan.ops).unwrap();
        let applied = apply_ops(&mut data, &plan.ops).unwrap();
        assert_eq!(data.landings.len(), 1);
        assert_eq!(data.landings[0].dialer_proxy, "🇭🇰 香港");
        assert!(applied[0].contains("家宽SOCKS"));
    }

    #[test]
    fn classifies_free_and_paid_tiers() {
        let catalog = builtin_tier_map();
        assert_eq!(lookup_free_tier("gemini-2.0-flash", &catalog), Some(true));
        assert_eq!(
            lookup_free_tier("gemini-2.0-flash-001", &catalog),
            Some(true)
        );
        assert_eq!(
            lookup_free_tier("gemini-3.1-pro-preview", &catalog),
            Some(false)
        );
        assert_eq!(
            lookup_free_tier("gemini-3-pro-image-preview", &catalog),
            Some(false)
        );
    }

    #[test]
    fn parses_pricing_markdown_snippet() {
        let body = r#"
## Gemini 2.0 Flash
`gemini-2.0-flash`
### Standard
| | Free Tier | Paid Tier |
| Input price | Free of charge | $0.10 |

## Gemini 3.1 Pro Preview
`gemini-3.1-pro-preview`
### Standard
| | Free Tier | Paid Tier |
| Input price | Not available | $0.75 |
"#;
        let m = parse_pricing_tiers(body);
        assert_eq!(m.get("gemini-2.0-flash"), Some(&true));
        assert_eq!(m.get("gemini-3.1-pro-preview"), Some(&false));
    }
}

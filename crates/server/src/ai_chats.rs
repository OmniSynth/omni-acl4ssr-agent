//! AI 助手本地对话历史：新建 / 列表 / 归档 / 删除 / 分支。
//! Gemini Developer API 的 Interactions 删除目前不稳定（常见 501），故以本机持久化为准。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::ai::AiOp;
use crate::ai_agent::AiAgentOption;

const CHATS_FILE: &str = "ai-chats.json";
const MAX_CHATS: usize = 80;
const MAX_MESSAGES: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String, // user | assistant
    pub content: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// plan | ask | advice | error；旧消息缺省为 plan
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<AiOp>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<AiAgentOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<String>>,
    /// 模型思考过程（开启 thinking 时）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// 本轮输入 tokens（≈ 当时上下文占用，对齐 Gemini promptTokenCount）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// 已成功应用到配置的时间（RFC3339）；有值则不可再次应用
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatThread {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// 本对话最近一轮的上下文占用（Gemini prompt tokens）
    #[serde(default)]
    pub last_prompt_tokens: u64,
    #[serde(default)]
    pub last_output_tokens: u64,
    #[serde(default)]
    pub last_total_tokens: u64,
    /// 该轮生效的上下文窗口上限
    #[serde(default)]
    pub context_limit: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
    pub model: String,
    pub message_count: usize,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ChatFile {
    #[serde(default)]
    chats: Vec<ChatThread>,
}

#[derive(Clone)]
pub struct ChatStore {
    path: PathBuf,
    inner: Arc<RwLock<Vec<ChatThread>>>,
}

impl ChatStore {
    pub async fn open(data_dir: impl AsRef<Path>) -> Self {
        let path = data_dir.as_ref().join(CHATS_FILE);
        let chats = load_file(&path).unwrap_or_default().chats;
        Self {
            path,
            inner: Arc::new(RwLock::new(chats)),
        }
    }

    /// 历史侧栏列表。
    /// - `roots_only`：只返回主对话（无 parent_id）；分支不进入历史列表
    /// - 空对话（无消息）不展示，避免「新对话」占位污染
    pub async fn list(&self, include_archived: bool, roots_only: bool) -> Vec<ChatSummary> {
        // 顺带清掉无消息的空壳对话（例如点了「新对话」却从未发送）
        {
            let mut guard = self.inner.write().await;
            let before = guard.len();
            guard.retain(|c| !c.messages.is_empty());
            if guard.len() != before {
                let _ = self.persist(&guard).await;
            }
        }

        let mut chats = self.inner.read().await.clone();
        chats.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        chats
            .into_iter()
            .filter(|c| include_archived || !c.archived)
            .filter(|c| !c.messages.is_empty())
            .filter(|c| !roots_only || c.parent_id.is_none())
            .map(|c| ChatSummary {
                id: c.id,
                title: c.title,
                archived: c.archived,
                created_at: c.created_at,
                updated_at: c.updated_at,
                model: c.model,
                message_count: c.messages.len(),
                parent_id: c.parent_id,
            })
            .collect()
    }

    pub async fn get(&self, id: &str) -> Result<ChatThread> {
        let mut chat = self
            .inner
            .read()
            .await
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("对话不存在"))?;
        // 旧数据无线程级用量时，从最近一条助手消息回填
        if chat.last_prompt_tokens == 0
            && chat.last_output_tokens == 0
            && chat.last_total_tokens == 0
        {
            let (lp, lo, lt) = last_tokens_from_messages(&chat.messages);
            chat.last_prompt_tokens = lp;
            chat.last_output_tokens = lo;
            chat.last_total_tokens = lt;
        }
        Ok(chat)
    }

    pub async fn create(&self, title: Option<String>, model: &str) -> Result<ChatThread> {
        let now = now_rfc3339();
        let chat = ChatThread {
            id: new_id("chat"),
            title: title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "新对话".into()),
            archived: false,
            created_at: now.clone(),
            updated_at: now,
            model: model.to_string(),
            messages: Vec::new(),
            parent_id: None,
            last_prompt_tokens: 0,
            last_output_tokens: 0,
            last_total_tokens: 0,
            context_limit: 0,
        };
        {
            let mut guard = self.inner.write().await;
            guard.insert(0, chat.clone());
            trim_chats(&mut guard);
            self.persist(&guard).await?;
        }
        Ok(chat)
    }

    pub async fn patch(
        &self,
        id: &str,
        title: Option<String>,
        archived: Option<bool>,
    ) -> Result<ChatThread> {
        let mut guard = self.inner.write().await;
        let chat = guard
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow::anyhow!("对话不存在"))?;
        if let Some(t) = title {
            let t = t.trim();
            if !t.is_empty() {
                chat.title = t.to_string();
            }
        }
        if let Some(a) = archived {
            chat.archived = a;
        }
        chat.updated_at = now_rfc3339();
        let out = chat.clone();
        self.persist(&guard).await?;
        Ok(out)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut guard = self.inner.write().await;
        let before = guard.len();
        guard.retain(|c| c.id != id);
        if guard.len() == before {
            bail!("对话不存在");
        }
        self.persist(&guard).await?;
        Ok(())
    }

    /// 标记某条助手消息的 ops 已应用到配置。
    pub async fn mark_applied(&self, chat_id: &str, message_id: &str) -> Result<ChatThread> {
        let mut guard = self.inner.write().await;
        let chat = guard
            .iter_mut()
            .find(|c| c.id == chat_id)
            .ok_or_else(|| anyhow::anyhow!("对话不存在"))?;
        let msg = chat
            .messages
            .iter_mut()
            .find(|m| m.id == message_id)
            .ok_or_else(|| anyhow::anyhow!("消息不存在"))?;
        if msg.role != "assistant" {
            bail!("只能标记助手消息为已应用");
        }
        if msg.ops.as_ref().map(|o| o.is_empty()).unwrap_or(true) {
            bail!("该消息没有可应用的变更");
        }
        if msg.applied_at.is_some() {
            bail!("该方案已应用过");
        }
        msg.applied_at = Some(now_rfc3339());
        chat.updated_at = now_rfc3339();
        let out = chat.clone();
        self.persist(&guard).await?;
        Ok(out)
    }

    /// 从某条消息处分叉：复制到该消息为止的历史，开新对话。
    pub async fn branch(&self, id: &str, message_id: &str) -> Result<ChatThread> {
        let src = self.get(id).await?;
        let idx = src
            .messages
            .iter()
            .position(|m| m.id == message_id)
            .ok_or_else(|| anyhow::anyhow!("消息不存在"))?;
        let now = now_rfc3339();
        let mut messages: Vec<ChatMessage> = src.messages[..=idx]
            .iter()
            .cloned()
            .map(|mut m| {
                m.id = new_id("msg");
                m
            })
            .collect();
        if messages.len() > MAX_MESSAGES {
            messages = messages.split_off(messages.len() - MAX_MESSAGES);
        }
        let title = format!("{} · 分支", truncate(&src.title, 24));
        let (lp, lo, lt) = last_tokens_from_messages(&messages);
        let chat = ChatThread {
            id: new_id("chat"),
            title,
            archived: false,
            created_at: now.clone(),
            updated_at: now,
            model: src.model,
            messages,
            parent_id: Some(src.id),
            last_prompt_tokens: lp,
            last_output_tokens: lo,
            last_total_tokens: lt,
            context_limit: src.context_limit,
        };
        {
            let mut guard = self.inner.write().await;
            guard.insert(0, chat.clone());
            trim_chats(&mut guard);
            self.persist(&guard).await?;
        }
        Ok(chat)
    }

    pub async fn append_turn(
        &self,
        id: &str,
        user_content: &str,
        assistant_summary: &str,
        assistant_content: &str,
        kind: &str,
        ops: Vec<AiOp>,
        preview: Vec<String>,
        options: Vec<AiAgentOption>,
        steps: Vec<String>,
        thinking: &str,
        prompt_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        context_limit: u64,
        model: &str,
    ) -> Result<ChatThread> {
        let mut guard = self.inner.write().await;
        let chat = guard
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow::anyhow!("对话不存在"))?;
        let now = now_rfc3339();
        if chat.title == "新对话" {
            chat.title = truncate(user_content, 28);
        }
        if !model.is_empty() {
            chat.model = model.to_string();
        }
        let kind = {
            let k = kind.trim().to_ascii_lowercase();
            match k.as_str() {
                "ask" | "advice" | "error" | "plan" => k,
                _ => "plan".into(),
            }
        };
        chat.messages.push(ChatMessage {
            id: new_id("msg"),
            role: "user".into(),
            content: user_content.to_string(),
            created_at: now.clone(),
            summary: None,
            kind: None,
            ops: None,
            preview: None,
            options: None,
            steps: None,
            thinking: None,
            prompt_tokens: None,
            output_tokens: None,
            total_tokens: None,
            applied_at: None,
        });
        let thinking = thinking.trim();
        chat.messages.push(ChatMessage {
            id: new_id("msg"),
            role: "assistant".into(),
            content: assistant_content.to_string(),
            created_at: now.clone(),
            summary: Some(assistant_summary.to_string()),
            kind: Some(kind),
            ops: Some(ops),
            preview: Some(preview),
            options: if options.is_empty() {
                None
            } else {
                Some(options)
            },
            steps: if steps.is_empty() { None } else { Some(steps) },
            thinking: if thinking.is_empty() {
                None
            } else {
                Some(thinking.to_string())
            },
            prompt_tokens,
            output_tokens,
            total_tokens,
            applied_at: None,
        });
        if let Some(n) = prompt_tokens {
            chat.last_prompt_tokens = n;
        }
        if let Some(n) = output_tokens {
            chat.last_output_tokens = n;
        }
        if let Some(n) = total_tokens {
            chat.last_total_tokens = n;
        } else if prompt_tokens.is_some() || output_tokens.is_some() {
            chat.last_total_tokens = prompt_tokens.unwrap_or(0).saturating_add(output_tokens.unwrap_or(0));
        }
        if context_limit > 0 {
            chat.context_limit = context_limit;
        }
        if chat.messages.len() > MAX_MESSAGES {
            let drop_n = chat.messages.len() - MAX_MESSAGES;
            chat.messages.drain(0..drop_n);
        }
        chat.updated_at = now;
        let out = chat.clone();
        self.persist(&guard).await?;
        Ok(out)
    }

    /// 供多轮 Agent：user/model 交替文本。
    pub async fn history_for_gemini(&self, id: &str) -> Result<Vec<(String, String)>> {
        let chat = self.get(id).await?;
        let mut out = Vec::new();
        for m in &chat.messages {
            let role = match m.role.as_str() {
                "user" => "user",
                "assistant" => "model",
                _ => continue,
            };
            let text = if role == "model" {
                assistant_history_text(m)
            } else {
                m.content.clone()
            };
            if text.trim().is_empty() {
                continue;
            }
            out.push((role.to_string(), text));
        }
        Ok(out)
    }

    /// 从最近一条 ask 消息解析选项文案（供 choice_id 续聊）。
    pub async fn resolve_choice_label(&self, id: &str, choice_id: &str) -> Result<Option<String>> {
        let choice_id = choice_id.trim();
        if choice_id.is_empty() {
            return Ok(None);
        }
        let chat = self.get(id).await?;
        for m in chat.messages.iter().rev() {
            if m.role != "assistant" {
                continue;
            }
            let kind = m.kind.as_deref().unwrap_or("");
            if kind != "ask" {
                continue;
            }
            if let Some(opts) = &m.options {
                if let Some(o) = opts.iter().find(|o| o.id == choice_id) {
                    return Ok(Some(o.label.clone()));
                }
            }
            break;
        }
        Ok(None)
    }

    async fn persist(&self, chats: &[ChatThread]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let file = ChatFile {
            chats: chats.to_vec(),
        };
        let pretty = serde_json::to_string_pretty(&file).context("序列化 ai-chats.json 失败")?;
        tokio::fs::write(&self.path, pretty)
            .await
            .with_context(|| format!("写入 {} 失败", self.path.display()))?;
        Ok(())
    }
}

fn assistant_history_text(m: &ChatMessage) -> String {
    let kind = m
        .kind
        .as_deref()
        .unwrap_or_else(|| {
            if m.ops.as_ref().map(|o| !o.is_empty()).unwrap_or(false) {
                "plan"
            } else {
                "plan"
            }
        })
        .to_string();
    let summary = m.summary.clone().unwrap_or_default();
    let ops = m.ops.clone().unwrap_or_default();
    let options = m.options.clone().unwrap_or_default();
    // 优先用结构化摘要，避免把整段 API raw 塞回上下文
    if !summary.is_empty() || !ops.is_empty() || !options.is_empty() || m.kind.is_some() {
        return serde_json::json!({
            "kind": kind,
            "summary": summary,
            "ops": ops,
            "options": options,
        })
        .to_string();
    }
    if !m.content.trim().is_empty() {
        m.content.clone()
    } else {
        summary
    }
}

fn last_tokens_from_messages(messages: &[ChatMessage]) -> (u64, u64, u64) {
    for m in messages.iter().rev() {
        if m.role != "assistant" {
            continue;
        }
        let lp = m.prompt_tokens.unwrap_or(0);
        let lo = m.output_tokens.unwrap_or(0);
        let lt = m
            .total_tokens
            .unwrap_or_else(|| lp.saturating_add(lo));
        if lp == 0 && lo == 0 && lt == 0 {
            continue;
        }
        // 旧消息可能只有 total：用其近似上下文占用
        let prompt = if lp > 0 { lp } else { lt };
        return (prompt, lo, lt);
    }
    (0, 0, 0)
}

fn load_file(path: &Path) -> Option<ChatFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn trim_chats(chats: &mut Vec<ChatThread>) {
    if chats.len() > MAX_CHATS {
        // 优先丢掉最旧的已归档，再丢最旧的
        chats.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        chats.truncate(MAX_CHATS);
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().as_simple())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn truncate(s: &str, max_chars: usize) -> String {
    let t = s.trim().replace('\n', " ");
    let mut it = t.chars();
    let head: String = it.by_ref().take(max_chars).collect();
    if it.next().is_some() {
        format!("{head}…")
    } else if head.is_empty() {
        "新对话".into()
    } else {
        head
    }
}

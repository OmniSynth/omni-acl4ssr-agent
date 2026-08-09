//! omni-acl4ssr-agent 配置 Agent：基础上下文 + 本地只读工具循环 → 最终结果。

use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::ai::{
    self, prepare_ops, preview_ops, AiAttachment, AiOp, AiStore, AiUsagePublic,
};
use crate::countries::WorldCatalog;
use crate::engine;
use crate::model::AppStateData;
use crate::regions::is_user_strategy_group;

const MAX_TOOL_ROUNDS: usize = 8;
const AGENT_SYSTEM: &str = r#"你是 omni-acl4ssr-agent（OpenWrt 本地 Mihomo 订阅转换）的项目配置 Agent。

工作方式：
1. 每轮请求会附带当前配置摘要；先判断能否处理用户需求。
2. 信息不足时，必须通过工具向本地 Agent 取证（查策略组/规则集/局域网/落地、搜索、校验 ops、转换诊断），不要猜测虚构的 id/组名。
3. 取证足够后，必须调用工具 submit_result 提交最终结果，且只能是以下 kind 之一：
   - plan：可执行的配置变更（ops 必须真实可用；提交前应用 validate_ops）
   - ask：需要用户在选项中选择（options 非空）
   - advice：指出用户理解有误、能力边界外的做法或更稳妥方案（无 ops；能解释清楚时优先用 advice，不要用 error）
   - error：工具/系统故障或完全无法理解需求时才用
4. 禁止静默改配置；plan 仅提案，用户确认后才会应用。
5. 不要修改国家/地区托管组（带 filter）；不要删除 g-default、g-chain。
6. proxies/group/target/dialer_proxy 必须使用真实组名（含 emoji 前缀如 🇭🇰 香港）或 DIRECT/REJECT，或本批将新增的组名。
7. 除工具调用外不要输出 Markdown；最终必须以 submit_result 结束。
8. 同名项已存在用 update_*；不存在用 add_*。删除/更新优先用工具返回的真实 id。
9. ops.op 仅允许：
   - add_group: name, proxies[]
   - update_group / delete_group: id 或 name；update 可选 name/proxies
   - add_ruleset: name, group, rules, 可选 enabled
   - update_ruleset / delete_ruleset: id 或 name
   - add_lan_route: src, target, 可选 name/enabled
   - update_lan_route / delete_lan_route: id 或 name
   - add_landing: name, server, 可选 landing_type/port/username/password/dialer_proxy/enabled
   - update_landing / delete_landing: id 或 name
   注意：没有 remove_*，删除一律用 delete_*。
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAgentOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiAgentResponse {
    /// plan | ask | advice | error
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub ops: Vec<AiOp>,
    #[serde(default)]
    pub preview: Vec<String>,
    #[serde(default)]
    pub options: Vec<AiAgentOption>,
    #[serde(default)]
    pub steps: Vec<String>,
    /// 聚合后的思考过程（开启 thinking 时）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thinking: String,
    pub usage: AiUsagePublic,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
}

/// 流式事件（SSE）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Status { text: String },
    Thinking { delta: String },
    Step { text: String },
    Result { result: AiAgentResponse },
    Error { message: String },
}

pub type AgentEventTx = mpsc::UnboundedSender<AgentEvent>;

fn emit(tx: &Option<AgentEventTx>, ev: AgentEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(ev);
    }
}

fn emit_status(tx: &Option<AgentEventTx>, text: impl Into<String>) {
    emit(tx, AgentEvent::Status { text: text.into() });
}

fn emit_thinking(tx: &Option<AgentEventTx>, delta: &str) {
    if delta.is_empty() {
        return;
    }
    emit(
        tx,
        AgentEvent::Thinking {
            delta: delta.to_string(),
        },
    );
}

fn emit_step(tx: &Option<AgentEventTx>, text: impl Into<String>) {
    emit(tx, AgentEvent::Step { text: text.into() });
}

#[derive(Debug, Deserialize)]
struct SubmitResultArgs {
    kind: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    ops: Vec<AiOp>,
    #[serde(default)]
    options: Vec<AiAgentOption>,
}

struct ToolCall {
    name: String,
    args: Value,
}

fn openai_tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "get_overview",
                "description": "获取配置总览（模式、数量、用户策略组/规则集/局域网/落地摘要）",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_groups",
                "description": "列出策略组；user_only=true 时仅用户策略组",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "user_only": { "type": "boolean" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_group",
                "description": "按 id 或 name 获取单个策略组",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_rulesets",
                "description": "列出全部规则集摘要",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_ruleset",
                "description": "按 id 或 name 获取规则集详情（含 rules 全文）",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_lan_routes",
                "description": "列出局域网分流",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_landings",
                "description": "列出落地代理（不含密码明文）",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_config",
                "description": "按关键字搜索组名、规则行、局域网 src、落地名/服务器",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "validate_ops",
                "description": "校验候选配置变更 ops 是否可应用到当前配置，返回 preview 或错误",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "ops": {
                            "type": "array",
                            "items": { "type": "object" }
                        }
                    },
                    "required": ["ops"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "convert_diag",
                "description": "运行订阅转换诊断（不返回完整 YAML）：节点/组/规则数量、地区统计、未匹配样例",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "submit_result",
                "description": "提交最终结果并结束。kind=plan|ask|advice|error",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["plan", "ask", "advice", "error"]
                        },
                        "summary": { "type": "string" },
                        "ops": {
                            "type": "array",
                            "items": { "type": "object" }
                        },
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "label": { "type": "string" }
                                },
                                "required": ["id", "label"]
                            }
                        }
                    },
                    "required": ["kind", "summary"]
                }
            }
        }
    ])
}

fn gemini_tools_schema() -> Value {
    let mut decls = Vec::new();
    if let Some(arr) = openai_tools_schema().as_array() {
        for t in arr {
            let f = &t["function"];
            decls.push(json!({
                "name": f["name"],
                "description": f["description"],
                "parameters": f["parameters"]
            }));
        }
    }
    json!([{ "functionDeclarations": decls }])
}

fn step_label(name: &str, result: &Value) -> String {
    match name {
        "get_overview" => "已读取配置总览".into(),
        "list_groups" => format!(
            "已列出策略组 {} 个",
            result.get("count").and_then(|v| v.as_u64()).unwrap_or(0)
        ),
        "get_group" => {
            if result.get("error").is_some() {
                "查询策略组失败".into()
            } else {
                format!(
                    "已读取策略组「{}」",
                    result
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                )
            }
        }
        "list_rulesets" => format!(
            "已列出规则集 {} 个",
            result.get("count").and_then(|v| v.as_u64()).unwrap_or(0)
        ),
        "get_ruleset" => {
            if result.get("error").is_some() {
                "查询规则集失败".into()
            } else {
                format!(
                    "已读取规则集「{}」",
                    result
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                )
            }
        }
        "list_lan_routes" => format!(
            "已列出局域网分流 {} 条",
            result.get("count").and_then(|v| v.as_u64()).unwrap_or(0)
        ),
        "list_landings" => format!(
            "已列出落地代理 {} 个",
            result.get("count").and_then(|v| v.as_u64()).unwrap_or(0)
        ),
        "search_config" => format!(
            "已搜索配置，命中 {} 处",
            result.get("hits").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
        ),
        "validate_ops" => {
            if result.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                "候选变更已通过校验".into()
            } else {
                "候选变更校验未通过".into()
            }
        }
        "convert_diag" => {
            if result.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                format!(
                    "转换诊断：{} 节点 / {} 组 / {} 规则",
                    result.get("proxy_count").and_then(|v| v.as_u64()).unwrap_or(0),
                    result.get("group_count").and_then(|v| v.as_u64()).unwrap_or(0),
                    result.get("rule_count").and_then(|v| v.as_u64()).unwrap_or(0)
                )
            } else {
                "转换诊断失败".into()
            }
        }
        "submit_result" => "已提交最终结果".into(),
        other => format!("已执行 {other}"),
    }
}

async fn exec_tool(
    name: &str,
    args: &Value,
    data: &AppStateData,
    http: &reqwest::Client,
    world: &WorldCatalog,
) -> Result<(Value, Option<SubmitResultArgs>)> {
    match name {
        "get_overview" => Ok((tool_overview(data), None)),
        "list_groups" => {
            let user_only = args.get("user_only").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok((tool_list_groups(data, user_only), None))
        }
        "get_group" => Ok((tool_get_group(data, args), None)),
        "list_rulesets" => Ok((tool_list_rulesets(data), None)),
        "get_ruleset" => Ok((tool_get_ruleset(data, args), None)),
        "list_lan_routes" => Ok((tool_list_lan(data), None)),
        "list_landings" => Ok((tool_list_landings(data), None)),
        "search_config" => {
            let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
            Ok((tool_search(data, q), None))
        }
        "validate_ops" => {
            let ops: Vec<AiOp> = args
                .get("ops")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            Ok((tool_validate_ops(data, &ops), None))
        }
        "convert_diag" => Ok((tool_convert_diag(data, http, world).await, None)),
        "submit_result" => {
            let mut parsed: SubmitResultArgs =
                serde_json::from_value(args.clone()).context("submit_result 参数无效")?;
            let kind = parsed.kind.trim().to_ascii_lowercase();
            match kind.as_str() {
                "plan" => match prepare_ops(data, &parsed.ops) {
                    Ok(ops) => {
                        parsed.kind = "plan".into();
                        parsed.ops = ops;
                        Ok((
                            json!({
                                "ok": true,
                                "kind": "plan",
                                "preview": preview_ops(&parsed.ops),
                            }),
                            Some(parsed),
                        ))
                    }
                    Err(e) => Ok((
                        json!({
                            "ok": false,
                            "error": e.to_string(),
                            "hint": "请修正 ops 后再次 submit_result，或改为 ask / advice / error"
                        }),
                        None,
                    )),
                },
                "ask" => {
                    if parsed.options.is_empty() {
                        Ok((
                            json!({
                                "ok": false,
                                "error": "ask 需要非空 options",
                                "hint": "补全 options 后再次 submit_result"
                            }),
                            None,
                        ))
                    } else {
                        parsed.kind = "ask".into();
                        Ok((json!({ "ok": true, "kind": "ask" }), Some(parsed)))
                    }
                }
                "advice" | "error" => {
                    parsed.kind = kind;
                    Ok((
                        json!({ "ok": true, "kind": parsed.kind }),
                        Some(parsed),
                    ))
                }
                other => Ok((
                    json!({
                        "ok": false,
                        "error": format!("未知 kind「{other}」"),
                        "hint": "kind 必须是 plan|ask|advice|error"
                    }),
                    None,
                )),
            }
        }
        other => Ok((json!({ "error": format!("未知工具 {other}") }), None)),
    }
}

fn tool_overview(data: &AppStateData) -> Value {
    let user_groups: Vec<_> = data
        .groups
        .iter()
        .filter(|g| is_user_strategy_group(g))
        .map(|g| json!({ "id": g.id, "name": g.name, "proxies": g.proxies }))
        .collect();
    json!({
        "groups_mode": data.groups_mode.as_str(),
        "profile": {
            "name": data.profile.name,
            "enabled": data.profile.enabled,
            "upstream_count": data.profile.urls().len(),
            "default_group": data.profile.default_group,
        },
        "counts": {
            "groups": data.groups.len(),
            "user_strategy_groups": user_groups.len(),
            "rulesets": data.rulesets.len(),
            "lan_routes": data.lan_routes.len(),
            "landings": data.landings.len(),
        },
        "user_strategy_groups": user_groups,
        "context_json": ai::build_context(data),
    })
}

fn tool_list_groups(data: &AppStateData, user_only: bool) -> Value {
    let groups: Vec<Value> = data
        .groups
        .iter()
        .filter(|g| !user_only || is_user_strategy_group(g))
        .map(|g| {
            json!({
                "id": g.id,
                "name": g.name,
                "group_type": g.group_type,
                "filter": g.filter,
                "proxies": g.proxies,
                "user_strategy": is_user_strategy_group(g),
            })
        })
        .collect();
    json!({ "count": groups.len(), "groups": groups })
}

fn tool_get_group(data: &AppStateData, args: &Value) -> Value {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    let g = data.groups.iter().find(|g| {
        (!id.is_empty() && g.id == id) || (!name.is_empty() && g.name == name)
    });
    match g {
        Some(g) => json!({
            "id": g.id,
            "name": g.name,
            "group_type": g.group_type,
            "filter": g.filter,
            "proxies": g.proxies,
            "url": g.url,
            "interval": g.interval,
            "user_strategy": is_user_strategy_group(g),
        }),
        None => json!({ "error": "未找到策略组" }),
    }
}

fn tool_list_rulesets(data: &AppStateData) -> Value {
    let items: Vec<Value> = data
        .rulesets
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "group": r.group,
                "enabled": r.enabled,
                "rules_lines": r.rules.lines().filter(|l| !l.trim().is_empty()).count(),
            })
        })
        .collect();
    json!({ "count": items.len(), "rulesets": items })
}

fn tool_get_ruleset(data: &AppStateData, args: &Value) -> Value {
    let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    let r = data.rulesets.iter().find(|r| {
        (!id.is_empty() && r.id == id) || (!name.is_empty() && r.name == name)
    });
    match r {
        Some(r) => json!({
            "id": r.id,
            "name": r.name,
            "group": r.group,
            "enabled": r.enabled,
            "rules": r.rules,
        }),
        None => json!({ "error": "未找到规则集" }),
    }
}

fn tool_list_lan(data: &AppStateData) -> Value {
    let items: Vec<Value> = data
        .lan_routes
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "src": r.src,
                "target": r.target,
                "enabled": r.enabled,
            })
        })
        .collect();
    json!({ "count": items.len(), "lan_routes": items })
}

fn tool_list_landings(data: &AppStateData) -> Value {
    let items: Vec<Value> = data
        .landings
        .iter()
        .map(|l| {
            json!({
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
    json!({ "count": items.len(), "landings": items })
}

fn tool_search(data: &AppStateData, query: &str) -> Value {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return json!({ "hits": [], "error": "query 为空" });
    }
    let mut hits = Vec::new();
    for g in &data.groups {
        if g.name.to_ascii_lowercase().contains(&q)
            || g.id.to_ascii_lowercase().contains(&q)
            || g.proxies.iter().any(|p| p.to_ascii_lowercase().contains(&q))
        {
            hits.push(json!({ "type": "group", "id": g.id, "name": g.name }));
        }
    }
    for r in &data.rulesets {
        if r.name.to_ascii_lowercase().contains(&q)
            || r.group.to_ascii_lowercase().contains(&q)
            || r.rules.to_ascii_lowercase().contains(&q)
        {
            hits.push(json!({
                "type": "ruleset",
                "id": r.id,
                "name": r.name,
                "group": r.group,
            }));
        }
    }
    for r in &data.lan_routes {
        if r.src.to_ascii_lowercase().contains(&q)
            || r.target.to_ascii_lowercase().contains(&q)
            || r.name.to_ascii_lowercase().contains(&q)
        {
            hits.push(json!({
                "type": "lan_route",
                "id": r.id,
                "src": r.src,
                "target": r.target,
            }));
        }
    }
    for l in &data.landings {
        if l.name.to_ascii_lowercase().contains(&q)
            || l.server.to_ascii_lowercase().contains(&q)
            || l.dialer_proxy.to_ascii_lowercase().contains(&q)
        {
            hits.push(json!({
                "type": "landing",
                "id": l.id,
                "name": l.name,
                "server": l.server,
            }));
        }
    }
    if hits.len() > 40 {
        hits.truncate(40);
    }
    json!({ "query": query, "hits": hits })
}

fn tool_validate_ops(data: &AppStateData, ops: &[AiOp]) -> Value {
    match prepare_ops(data, ops) {
        Ok(prepared) => json!({
            "ok": true,
            "ops": prepared,
            "preview": preview_ops(&prepared),
        }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

async fn tool_convert_diag(
    data: &AppStateData,
    http: &reqwest::Client,
    world: &WorldCatalog,
) -> Value {
    let extras = world.extras().await;
    match engine::convert(data, http, &extras).await {
        Ok(r) => json!({
            "ok": true,
            "proxy_count": r.proxy_count,
            "group_count": r.group_count,
            "rule_count": r.rule_count,
            "groups_mode": r.groups_mode.as_str(),
            "regions": r.regions,
            "unmatched_count": r.unmatched.len(),
            "unmatched_samples": r.unmatched.into_iter().take(12).collect::<Vec<_>>(),
            "message": "转换成功",
        }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn finalize_submit(
    data: &AppStateData,
    submit: SubmitResultArgs,
    steps: Vec<String>,
    thinking: String,
    usage: AiUsagePublic,
    raw: String,
) -> Result<AiAgentResponse> {
    let kind = match submit.kind.trim().to_ascii_lowercase().as_str() {
        "plan" => "plan",
        "ask" => "ask",
        "advice" => "advice",
        "error" => "error",
        other => {
            return Ok(AiAgentResponse {
                kind: "error".into(),
                summary: format!("Agent 返回了未知 kind「{other}」"),
                ops: vec![],
                preview: vec![],
                options: vec![],
                steps,
                thinking,
                usage,
                raw,
                chat_id: None,
            });
        }
    };

    if kind == "plan" {
        match prepare_ops(data, &submit.ops) {
            Ok(ops) => {
                let preview = preview_ops(&ops);
                let summary = if submit.summary.trim().is_empty() {
                    if ops.is_empty() {
                        "无需变更".into()
                    } else {
                        "已生成配置变更方案".into()
                    }
                } else {
                    submit.summary
                };
                Ok(AiAgentResponse {
                    kind: "plan".into(),
                    summary,
                    ops,
                    preview,
                    options: vec![],
                    steps,
                    thinking,
                    usage,
                    raw,
                    chat_id: None,
                })
            }
            Err(e) => Ok(AiAgentResponse {
                kind: "error".into(),
                summary: format!("方案未通过配置校验，已拒绝交给你应用：{e}"),
                ops: vec![],
                preview: vec![],
                options: vec![],
                steps,
                thinking,
                usage,
                raw,
                chat_id: None,
            }),
        }
    } else if kind == "ask" {
        if submit.options.is_empty() {
            Ok(AiAgentResponse {
                kind: "error".into(),
                summary: "Agent 想提问但未提供选项".into(),
                ops: vec![],
                preview: vec![],
                options: vec![],
                steps,
                thinking,
                usage,
                raw,
                chat_id: None,
            })
        } else {
            Ok(AiAgentResponse {
                kind: "ask".into(),
                summary: submit.summary,
                ops: vec![],
                preview: vec![],
                options: submit.options,
                steps,
                thinking,
                usage,
                raw,
                chat_id: None,
            })
        }
    } else {
        Ok(AiAgentResponse {
            kind: kind.into(),
            summary: if submit.summary.trim().is_empty() {
                if kind == "error" {
                    "处理失败".into()
                } else {
                    "建议如下".into()
                }
            } else {
                submit.summary
            },
            ops: vec![],
            preview: vec![],
            options: vec![],
            steps,
            thinking,
            usage,
            raw,
            chat_id: None,
        })
    }
}

fn gemini_supports_thinking(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("2.5")
        || m.contains("gemini-3")
        || m.contains("thinking")
        || m.starts_with("gemini-3")
}

fn gemini_thinking_config(enabled: bool, model: &str) -> Option<Value> {
    if !gemini_supports_thinking(model) {
        return None;
    }
    if enabled {
        Some(json!({
            "includeThoughts": true,
            "thinkingBudget": -1
        }))
    } else {
        Some(json!({ "thinkingBudget": 0 }))
    }
}

async fn consume_sse_data_frames(
    resp: reqwest::Response,
    events: &Option<AgentEventTx>,
    mut on_data: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    use std::time::Instant;
    use tokio::time::{interval, MissedTickBehavior};

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut heartbeat = interval(Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let mut last_progress = Instant::now();
    let mut saw_data = false;

    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break; };
                let chunk = chunk.context("读取上游 SSE 流失败")?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buf.find("\n\n") {
                    let frame = buf[..pos].to_string();
                    buf.drain(..pos + 2);
                    for line in frame.lines() {
                        let line = line.trim_end();
                        let Some(rest) = line.strip_prefix("data:") else {
                            continue;
                        };
                        let data = rest.trim_start();
                        if data.is_empty() {
                            continue;
                        }
                        if data == "[DONE]" {
                            return Ok(());
                        }
                        saw_data = true;
                        last_progress = Instant::now();
                        on_data(data)?;
                    }
                }
            }
            _ = heartbeat.tick() => {
                if !saw_data || last_progress.elapsed() >= Duration::from_secs(2) {
                    emit_status(events, "仍在生成…");
                    last_progress = Instant::now();
                }
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct OpenAiToolAcc {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct OpenAiStreamAcc {
    content: String,
    reasoning: String,
    tool_calls: Vec<OpenAiToolAcc>,
    finish_reason: Option<String>,
    usage: Option<Value>,
}

fn merge_openai_stream_chunk(acc: &mut OpenAiStreamAcc, v: &Value, tx: &Option<AgentEventTx>) {
    if let Some(u) = v.get("usage") {
        acc.usage = Some(u.clone());
    }
    let Some(choice) = v.pointer("/choices/0") else {
        return;
    };
    if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str()) {
        if !fr.is_empty() && fr != "null" {
            acc.finish_reason = Some(fr.to_string());
        }
    }
    let Some(delta) = choice.get("delta") else {
        return;
    };
    if let Some(rc) = delta.get("reasoning_content").and_then(|x| x.as_str()) {
        if !rc.is_empty() {
            acc.reasoning.push_str(rc);
            emit_thinking(tx, rc);
        }
    }
    if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
        if !c.is_empty() {
            acc.content.push_str(c);
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
        for tc in tcs {
            let idx = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            while acc.tool_calls.len() <= idx {
                acc.tool_calls.push(OpenAiToolAcc::default());
            }
            let slot = &mut acc.tool_calls[idx];
            if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                if !id.is_empty() {
                    slot.id = id.to_string();
                }
            }
            if let Some(name) = tc.pointer("/function/name").and_then(|x| x.as_str()) {
                if !name.is_empty() {
                    slot.name.push_str(name);
                }
            }
            if let Some(args) = tc.pointer("/function/arguments").and_then(|x| x.as_str()) {
                slot.arguments.push_str(args);
            }
        }
    }
}

fn urlencoding_key(key: &str) -> String {
    key.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{:02X}", other),
        })
        .collect()
}

/// 运行配置 Agent（Gemini 或 DeepSeek）。
pub async fn run_agent(
    provider: &str,
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    data: &AppStateData,
    store: &AiStore,
    world: &WorldCatalog,
    user_prompt: &str,
    context_limit: u64,
    history: &[(String, String)],
    attachments: &[AiAttachment],
    thinking_enabled: bool,
    events: Option<AgentEventTx>,
) -> Result<AiAgentResponse> {
    let provider = ai::normalize_provider(provider);
    if provider == "deepseek" {
        if !attachments.is_empty() {
            bail!("DeepSeek Agent 暂不支持图片附件，请去掉附件或改用 Gemini。");
        }
        run_agent_deepseek(
            http,
            api_key,
            model,
            data,
            store,
            world,
            user_prompt,
            context_limit,
            history,
            thinking_enabled,
            events,
        )
        .await
    } else {
        run_agent_gemini(
            http,
            api_key,
            model,
            data,
            store,
            world,
            user_prompt,
            context_limit,
            history,
            attachments,
            thinking_enabled,
            events,
        )
        .await
    }
}

async fn run_agent_deepseek(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    data: &AppStateData,
    store: &AiStore,
    world: &WorldCatalog,
    user_prompt: &str,
    context_limit: u64,
    history: &[(String, String)],
    thinking_enabled: bool,
    events: Option<AgentEventTx>,
) -> Result<AiAgentResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 DeepSeek API Key");
    }
    let prompt = user_prompt.trim();
    if prompt.is_empty() {
        bail!("请输入需求描述");
    }
    let model = if model.trim().is_empty() {
        "deepseek-v4-flash"
    } else {
        model.trim()
    };
    emit_status(
        &events,
        if thinking_enabled {
            "正在思考…"
        } else {
            "正在查阅配置…"
        },
    );
    let context = ai::build_context(data);
    let mut messages: Vec<Value> = vec![json!({
        "role": "system",
        "content": AGENT_SYSTEM
    })];
    for (role, text) in history {
        let role = match role.as_str() {
            "model" | "assistant" => "assistant",
            _ => "user",
        };
        if text.trim().is_empty() {
            continue;
        }
        messages.push(json!({ "role": role, "content": text }));
    }
    messages.push(json!({
        "role": "user",
        "content": format!(
            "当前配置摘要（JSON）：\n{context}\n\n用户需求：\n{prompt}\n\n请先判断；不足则调用工具；完成后必须 submit_result。"
        )
    }));

    let mut steps = Vec::new();
    let mut thinking_all = String::new();
    let mut last_usage = store.usage_public().await;
    let mut raw_last = String::new();
    let mut last_tool_hint = String::new();
    let think_type = if thinking_enabled { "enabled" } else { "disabled" };

    for round in 0..MAX_TOOL_ROUNDS {
        emit_status(&events, format!("模型推理中（第 {} 轮）…", round + 1));
        let body = json!({
            "model": model,
            "messages": messages,
            "tools": openai_tools_schema(),
            "tool_choice": "auto",
            "temperature": 0.2,
            "thinking": { "type": think_type },
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        let resp = http
            .post("https://api.deepseek.com/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key.trim()))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(180))
            .json(&body)
            .send()
            .await
            .context("请求 DeepSeek Agent 失败")?;
        let status = resp.status();
        if !status.is_success() {
            let err = resp.text().await.unwrap_or_default();
            bail!(
                "DeepSeek Agent HTTP {status}: {}",
                err.chars().take(400).collect::<String>()
            );
        }

        let mut acc = OpenAiStreamAcc::default();
        let mut frames = Vec::new();
        consume_sse_data_frames(resp, &events, |data| {
            frames.push(data.to_string());
            let v: Value = serde_json::from_str(data).context("解析 DeepSeek SSE JSON 失败")?;
            merge_openai_stream_chunk(&mut acc, &v, &events);
            Ok(())
        })
        .await?;
        raw_last = frames.last().cloned().unwrap_or_default();
        if !acc.reasoning.is_empty() {
            if !thinking_all.is_empty() {
                thinking_all.push_str("\n\n");
            }
            thinking_all.push_str(&acc.reasoning);
        }

        if let Some(u) = acc.usage.as_ref() {
            let pt = u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            let ot = u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            let tt = u.get("total_tokens").and_then(|x| x.as_u64()).unwrap_or(pt + ot);
            let hit = u
                .get("prompt_cache_hit_tokens")
                .and_then(|x| x.as_u64());
            let miss = u
                .get("prompt_cache_miss_tokens")
                .and_then(|x| x.as_u64());
            let limit = if context_limit > 0 {
                context_limit
            } else {
                1_048_576
            };
            last_usage = store
                .record_usage_ex(model, pt, ot, tt, limit, hit, miss)
                .await;
        }

        let tool_calls: Vec<Value> = acc
            .tool_calls
            .iter()
            .filter(|t| !t.name.is_empty())
            .enumerate()
            .map(|(i, t)| {
                json!({
                    "id": if t.id.is_empty() { format!("call_{i}") } else { t.id.clone() },
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "arguments": t.arguments
                    }
                })
            })
            .collect();

        if tool_calls.is_empty() {
            if let Ok(submit) = serde_json::from_str::<SubmitResultArgs>(&acc.content) {
                return finalize_submit(
                    data,
                    submit,
                    steps,
                    thinking_all,
                    last_usage,
                    acc.content,
                );
            }
            if round + 1 >= MAX_TOOL_ROUNDS {
                break;
            }
            let mut msg = json!({
                "role": "assistant",
                "content": if acc.content.is_empty() { Value::Null } else { Value::String(acc.content.clone()) }
            });
            if thinking_enabled && !acc.reasoning.is_empty() {
                msg["reasoning_content"] = Value::String(acc.reasoning.clone());
            }
            messages.push(msg);
            messages.push(json!({
                "role": "user",
                "content": "请调用工具取证，或调用 submit_result 提交最终结果（kind=plan|ask|advice|error）。删除用 delete_*，不要用 remove_*。"
            }));
            continue;
        }

        let mut msg = json!({
            "role": "assistant",
            "content": if acc.content.is_empty() { Value::Null } else { Value::String(acc.content.clone()) },
            "tool_calls": tool_calls
        });
        // DeepSeek：带 tools 时必须回传 reasoning_content
        if thinking_enabled && !acc.reasoning.is_empty() {
            msg["reasoning_content"] = Value::String(acc.reasoning.clone());
        }
        messages.push(msg);

        let mut submitted = None;
        for tc in &acc.tool_calls {
            if tc.name.is_empty() {
                continue;
            }
            let id = if tc.id.is_empty() {
                "tool".to_string()
            } else {
                tc.id.clone()
            };
            let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
            emit_status(&events, format!("执行工具 {name}…", name = tc.name));
            let (result, submit) = exec_tool(&tc.name, &args, data, http, world).await?;
            let label = step_label(&tc.name, &result);
            steps.push(label.clone());
            emit_step(&events, label);
            if result.get("ok") == Some(&Value::Bool(false)) {
                if let Some(err) = result.get("error").and_then(|e| e.as_str()) {
                    last_tool_hint = err.to_string();
                }
            }
            if let Some(s) = submit {
                submitted = Some(s);
            }
            messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result.to_string()
            }));
        }
        if let Some(s) = submitted {
            info!(rounds = round + 1, steps = steps.len(), "DeepSeek Agent 完成");
            return finalize_submit(data, s, steps, thinking_all, last_usage, raw_last);
        }
    }

    let summary = if last_tool_hint.is_empty() {
        "Agent 工具轮次用尽仍未提交结果，请简化需求后重试".into()
    } else {
        format!("Agent 工具轮次用尽仍未提交结果。最近工具错误：{last_tool_hint}")
    };
    Ok(AiAgentResponse {
        kind: "error".into(),
        summary,
        ops: vec![],
        preview: vec![],
        options: vec![],
        steps,
        thinking: thinking_all,
        usage: last_usage,
        raw: raw_last,
        chat_id: None,
    })
}

async fn run_agent_gemini(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    data: &AppStateData,
    store: &AiStore,
    world: &WorldCatalog,
    user_prompt: &str,
    context_limit: u64,
    history: &[(String, String)],
    attachments: &[AiAttachment],
    thinking_enabled: bool,
    events: Option<AgentEventTx>,
) -> Result<AiAgentResponse> {
    if api_key.trim().is_empty() {
        bail!("未配置 Gemini API Key");
    }
    let prompt = user_prompt.trim();
    let media = ai::validate_attachments(attachments)?;
    if prompt.is_empty() && media.is_empty() {
        bail!("请输入需求描述，或上传/录制附件");
    }
    let model = if model.trim().is_empty() {
        "gemini-2.0-flash"
    } else {
        model.trim()
    };
    emit_status(
        &events,
        if thinking_enabled && gemini_supports_thinking(model) {
            "正在思考…"
        } else {
            "正在查阅配置…"
        },
    );
    if thinking_enabled && !gemini_supports_thinking(model) {
        emit_status(
            &events,
            "当前 Gemini 模型不支持思考模式，已按普通模式继续",
        );
    }
    let context = ai::build_context(data);

    let mut contents: Vec<Value> = Vec::new();
    for (role, text) in history {
        let role = match role.as_str() {
            "model" | "assistant" => "model",
            _ => "user",
        };
        if text.trim().is_empty() {
            continue;
        }
        contents.push(json!({
            "role": role,
            "parts": [{ "text": text }]
        }));
    }
    let need = if prompt.is_empty() {
        "（用户未写文字，请根据附件中的截图理解需求）"
    } else {
        prompt
    };
    let mut parts: Vec<Value> = vec![json!({
        "text": format!(
            "当前配置摘要（JSON）：\n{context}\n\n用户需求：\n{need}\n\n请先判断；不足则调用工具；完成后必须 submit_result。"
        )
    })];
    for (mime, b64, _) in &media {
        parts.push(json!({
            "inline_data": {
                "mime_type": mime,
                "data": b64
            }
        }));
    }
    contents.push(json!({
        "role": "user",
        "parts": parts
    }));

    let mut steps = Vec::new();
    let mut thinking_all = String::new();
    let mut last_usage = store.usage_public().await;
    let mut raw_last = String::new();
    let mut last_tool_hint = String::new();
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse&key={}",
        urlencoding_key(api_key)
    );

    for round in 0..MAX_TOOL_ROUNDS {
        emit_status(&events, format!("模型推理中（第 {} 轮）…", round + 1));
        let mut generation = json!({
            "temperature": 0.2,
            "maxOutputTokens": 8192
        });
        if let Some(tc) = gemini_thinking_config(thinking_enabled, model) {
            generation["thinkingConfig"] = tc;
        }
        let body = json!({
            "systemInstruction": { "parts": [{ "text": AGENT_SYSTEM }] },
            "contents": contents,
            "tools": gemini_tools_schema(),
            "toolConfig": {
                "functionCallingConfig": { "mode": "AUTO" }
            },
            "generationConfig": generation
        });
        let resp = http
            .post(&url)
            .timeout(Duration::from_secs(180))
            .json(&body)
            .send()
            .await
            .context("请求 Gemini Agent 失败")?;
        let status = resp.status();
        if !status.is_success() {
            let err = resp.text().await.unwrap_or_default();
            bail!(
                "Gemini Agent HTTP {status}: {}",
                err.chars().take(400).collect::<String>()
            );
        }

        let mut model_parts: Vec<Value> = Vec::new();
        let mut function_calls: Vec<ToolCall> = Vec::new();
        let mut text_bits = Vec::new();
        let mut raw_frames = Vec::new();
        let mut usage_meta: Option<(u64, u64, u64, u64)> = None;
        consume_sse_data_frames(resp, &events, |data| {
            raw_frames.push(data.to_string());
            let v: Value = serde_json::from_str(data).context("解析 Gemini SSE JSON 失败")?;
            if let Some(meta) = v.get("usageMetadata") {
                let pt = meta
                    .get("promptTokenCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let ot = meta
                    .get("candidatesTokenCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let thought = meta
                    .get("thoughtsTokenCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let tt = meta
                    .get("totalTokenCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(pt + ot + thought);
                usage_meta = Some((pt, ot, thought, tt));
            }
            let Some(parts) = v
                .pointer("/candidates/0/content/parts")
                .and_then(|p| p.as_array())
            else {
                return Ok(());
            };
            for p in parts {
                model_parts.push(p.clone());
                let is_thought = p.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    if is_thought {
                        thinking_all.push_str(t);
                        emit_thinking(&events, t);
                    } else {
                        text_bits.push(t.to_string());
                    }
                }
                if let Some(fc) = p.get("functionCall") {
                    let name = fc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = fc.get("args").cloned().unwrap_or(json!({}));
                    if !name.is_empty() {
                        // 流式可能分片；同名多次时合并 args 较难，通常整段一次给出
                        if function_calls.last().map(|c| c.name.as_str()) != Some(name.as_str()) {
                            function_calls.push(ToolCall { name, args });
                        } else if let Some(last) = function_calls.last_mut() {
                            if last.args.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                                last.args = args;
                            }
                        }
                    }
                }
            }
            Ok(())
        })
        .await?;
        raw_last = raw_frames.last().cloned().unwrap_or_default();

        if let Some((pt, ot, thought, tt)) = usage_meta {
            let limit = if context_limit > 0 {
                context_limit
            } else {
                1_048_576
            };
            last_usage = store
                .record_usage(model, pt, ot.saturating_add(thought), tt, limit)
                .await;
        }

        if model_parts.is_empty() {
            warn!(round, "Gemini Agent 无 parts");
            break;
        }

        contents.push(json!({
            "role": "model",
            "parts": model_parts
        }));

        if function_calls.is_empty() {
            let text = text_bits.join("\n");
            if let Ok(submit) = serde_json::from_str::<SubmitResultArgs>(&text) {
                return finalize_submit(data, submit, steps, thinking_all, last_usage, text);
            }
            if round + 1 >= MAX_TOOL_ROUNDS {
                break;
            }
            contents.push(json!({
                "role": "user",
                "parts": [{
                    "text": "请调用工具取证，或调用 submit_result 提交最终结果（kind=plan|ask|advice|error）。"
                }]
            }));
            continue;
        }

        let mut response_parts = Vec::new();
        let mut submitted = None;
        for fc in function_calls {
            emit_status(&events, format!("执行工具 {}…", fc.name));
            let (result, submit) = exec_tool(&fc.name, &fc.args, data, http, world).await?;
            let label = step_label(&fc.name, &result);
            steps.push(label.clone());
            emit_step(&events, label);
            if result.get("ok") == Some(&Value::Bool(false)) {
                if let Some(err) = result.get("error").and_then(|e| e.as_str()) {
                    last_tool_hint = err.to_string();
                }
            }
            if let Some(s) = submit {
                submitted = Some(s);
            }
            response_parts.push(json!({
                "functionResponse": {
                    "name": fc.name,
                    "response": result
                }
            }));
        }
        contents.push(json!({
            "role": "user",
            "parts": response_parts
        }));

        if let Some(s) = submitted {
            info!(rounds = round + 1, steps = steps.len(), "Gemini Agent 完成");
            return finalize_submit(data, s, steps, thinking_all, last_usage, raw_last);
        }
    }

    let summary = if last_tool_hint.is_empty() {
        "Agent 工具轮次用尽仍未提交结果，请简化需求后重试".into()
    } else {
        format!("Agent 工具轮次用尽仍未提交结果。最近工具错误：{last_tool_hint}")
    };
    Ok(AiAgentResponse {
        kind: "error".into(),
        summary,
        ops: vec![],
        preview: vec![],
        options: vec![],
        steps,
        thinking: thinking_all,
        usage: last_usage,
        raw: raw_last,
        chat_id: None,
    })
}

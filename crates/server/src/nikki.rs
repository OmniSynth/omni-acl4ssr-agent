//! 调用本机 Nikki（luci-app-nikki）更新订阅并重载。

use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NikkiSubscription {
    pub section_id: String,
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NikkiUpdateResult {
    pub ok: bool,
    pub message: String,
    pub section_ids: Vec<String>,
    pub reloaded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NikkiPanelInfo {
    pub ok: bool,
    /// 相对路径，前端拼上当前主机名后打开
    pub path: String,
    pub port: u16,
    pub ui_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct UbusUpdateResp {
    #[serde(default)]
    success: bool,
}

/// 读取 Nikki 外部控制面板（zashboard / metacubexd）地址信息。
pub async fn panel_info() -> NikkiPanelInfo {
    let listen = uci_get("nikki.mixin.api_listen")
        .await
        .or(uci_get("nikki.@mixin[0].api_listen").await)
        .unwrap_or_else(|| "0.0.0.0:9090".into());
    let ui_path = uci_get("nikki.mixin.ui_path")
        .await
        .unwrap_or_else(|| "ui".into());
    let secret = uci_get("nikki.mixin.api_secret").await.filter(|s| !s.is_empty());

    let port = listen
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(9090);
    let ui = ui_path.trim().trim_matches('/').to_string();
    let ui = if ui.is_empty() { "ui".into() } else { ui };

    NikkiPanelInfo {
        ok: true,
        path: format!("/{ui}/"),
        port,
        ui_path: ui,
        secret,
        message: "ok".into(),
    }
}

async fn uci_get(key: &str) -> Option<String> {
    let output = Command::new("uci")
        .args(["-q", "get", key])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 列出 uci `nikki` 中的 subscription 段。
pub async fn list_subscriptions() -> Vec<NikkiSubscription> {
    let Ok(output) = Command::new("uci")
        .args(["-q", "show", "nikki"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_uci_subscriptions(&String::from_utf8_lossy(&output.stdout))
}

fn parse_uci_subscriptions(text: &str) -> Vec<NikkiSubscription> {
    use std::collections::BTreeMap;
    let mut ids: Vec<String> = Vec::new();
    let mut fields: BTreeMap<String, (String, String, Option<String>, Option<bool>)> =
        BTreeMap::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("nikki.") {
            if let Some((id, rhs)) = rest.split_once('=') {
                if rhs == "subscription" || rhs.trim_matches('\'') == "subscription" {
                    if !ids.contains(&id.to_string()) {
                        ids.push(id.to_string());
                    }
                    fields.entry(id.to_string()).or_default();
                }
            }
            if let Some((id_prop, raw)) = rest.split_once('=') {
                let Some((id, prop)) = id_prop.split_once('.') else {
                    continue;
                };
                let entry = fields.entry(id.to_string()).or_default();
                let val = unquote(raw);
                match prop {
                    "name" => entry.0 = val,
                    "url" => entry.1 = val,
                    "update" => entry.2 = Some(val),
                    "success" => entry.3 = Some(val == "1" || val.eq_ignore_ascii_case("true")),
                    _ => {}
                }
            }
        }
    }

    ids.into_iter()
        .filter_map(|id| {
            let (name, url, last_update, success) = fields.remove(&id)?;
            Some(NikkiSubscription {
                section_id: id,
                name: if name.is_empty() {
                    "subscription".into()
                } else {
                    name
                },
                url,
                last_update,
                success,
            })
        })
        .collect()
}

fn unquote(raw: &str) -> String {
    let s = raw.trim();
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        s[1..s.len().saturating_sub(1)].to_string()
    } else {
        s.to_string()
    }
}

/// 更新指定（或全部）Nikki 订阅，并默认 reload 使运行配置生效。
pub async fn update_subscriptions(
    section_id: Option<&str>,
    reload: bool,
) -> NikkiUpdateResult {
    let subs = list_subscriptions().await;
    if subs.is_empty() {
        return NikkiUpdateResult {
            ok: false,
            message: "未检测到 Nikki 订阅（uci nikki 无 subscription 段）".into(),
            section_ids: vec![],
            reloaded: false,
        };
    }

    let targets: Vec<NikkiSubscription> = if let Some(id) = section_id.map(str::trim).filter(|s| !s.is_empty())
    {
        let found: Vec<_> = subs.into_iter().filter(|s| s.section_id == id).collect();
        if found.is_empty() {
            return NikkiUpdateResult {
                ok: false,
                message: format!("找不到 Nikki 订阅段：{id}"),
                section_ids: vec![],
                reloaded: false,
            };
        }
        found
    } else {
        // 优先更新指向本机 /sub 的订阅；否则全部更新
        let local: Vec<_> = subs
            .iter()
            .filter(|s| s.url.contains(":8787/sub") || s.url.contains("127.0.0.1:8787"))
            .cloned()
            .collect();
        if local.is_empty() {
            subs
        } else {
            local
        }
    };

    let mut ok_ids = Vec::new();
    let mut errors = Vec::new();
    for s in &targets {
        match update_one(&s.section_id).await {
            Ok(()) => {
                info!(section = %s.section_id, "Nikki 订阅已更新");
                ok_ids.push(s.section_id.clone());
            }
            Err(e) => {
                warn!(section = %s.section_id, error = %e, "Nikki 订阅更新失败");
                errors.push(format!("{}: {e}", s.section_id));
            }
        }
    }

    if ok_ids.is_empty() {
        return NikkiUpdateResult {
            ok: false,
            message: format!("更新失败：{}", errors.join("; ")),
            section_ids: vec![],
            reloaded: false,
        };
    }

    let mut reloaded = false;
    if reload {
        match reload_nikki().await {
            Ok(()) => {
                reloaded = true;
                info!("Nikki 已 reload");
            }
            Err(e) => {
                warn!(error = %e, "Nikki reload 失败");
                errors.push(format!("reload: {e}"));
            }
        }
    }

    let mut message = format!("已更新 Nikki 订阅 {}", ok_ids.join(", "));
    if reloaded {
        message.push_str("，并已重载生效");
    } else if reload {
        message.push_str("（下载成功，但重载失败）");
    }
    if !errors.is_empty() {
        message.push_str("；部分问题：");
        message.push_str(&errors.join("; "));
    }

    NikkiUpdateResult {
        ok: !ok_ids.is_empty() && (!reload || reloaded),
        message,
        section_ids: ok_ids,
        reloaded,
    }
}

async fn update_one(section_id: &str) -> Result<(), String> {
    // ubus call luci.nikki update_subscription '{"section_id":"..."}'
    let arg = serde_json::json!({ "section_id": section_id }).to_string();
    let fut = Command::new("ubus")
        .args(["call", "luci.nikki", "update_subscription", &arg])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let output = timeout(Duration::from_secs(150), fut)
        .await
        .map_err(|_| "ubus 更新订阅超时".to_string())?
        .map_err(|e| format!("执行 ubus 失败：{e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(if err.trim().is_empty() {
            format!("ubus 退出码 {}", output.status.code().unwrap_or(-1))
        } else {
            err.trim().to_string()
        });
    }
    let resp: UbusUpdateResp = serde_json::from_slice(&output.stdout).unwrap_or(UbusUpdateResp {
        success: false,
    });
    if !resp.success {
        // 回退：直接调 init.d（与 luci.nikki 内部一致）
        let ok = run_cmd(
            "service",
            &["nikki", "update_subscription", section_id],
            Duration::from_secs(150),
        )
        .await
        .is_ok();
        if !ok {
            return Err("Nikki 返回 success=false".into());
        }
    }
    Ok(())
}

async fn reload_nikki() -> Result<(), String> {
    run_cmd("/etc/init.d/nikki", &["reload"], Duration::from_secs(60)).await
}

async fn run_cmd(prog: &str, args: &[&str], limit: Duration) -> Result<(), String> {
    let fut = Command::new(prog)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let output = timeout(limit, fut)
        .await
        .map_err(|_| format!("{prog} 超时"))?
        .map_err(|e| format!("执行 {prog} 失败：{e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(if err.trim().is_empty() {
            format!("{prog} 失败")
        } else {
            err.trim().to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_subs() {
        let text = r#"
nikki.subscription=subscription
nikki.subscription.name='st'
nikki.subscription.url='http://172.16.1.1:8787/sub'
nikki.subscription.update='2026-08-09 14:34:21'
nikki.subscription.success='1'
nikki.other=subscription
nikki.other.name='backup'
nikki.other.url='https://example.com/sub'
"#;
        let list = parse_uci_subscriptions(text);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].section_id, "subscription");
        assert_eq!(list[0].url, "http://172.16.1.1:8787/sub");
        assert_eq!(list[0].success, Some(true));
        assert_eq!(list[1].section_id, "other");
    }
}

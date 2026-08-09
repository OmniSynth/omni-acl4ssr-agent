use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::model::{AppStateData, GroupType, GroupsMode, LandingType};
use crate::regions::{self, ManagedPlan, RegionEntry, RegionStat, NAME_DEFAULT};

#[derive(Clone, Default)]
pub struct YamlCache {
    inner: Arc<RwLock<Option<CacheEntry>>>,
}

struct CacheEntry {
    key: String,
    yaml: String,
    at: Instant,
}

impl YamlCache {
    pub async fn get(&self, key: &str, ttl: Duration) -> Option<String> {
        let guard = self.inner.read().await;
        let entry = guard.as_ref()?;
        if entry.key == key && entry.at.elapsed() < ttl {
            Some(entry.yaml.clone())
        } else {
            None
        }
    }

    pub async fn set(&self, key: String, yaml: String) {
        *self.inner.write().await = Some(CacheEntry {
            key,
            yaml,
            at: Instant::now(),
        });
    }

    pub async fn clear(&self) {
        *self.inner.write().await = None;
    }
}

pub struct ConvertResult {
    pub yaml: String,
    pub proxy_count: usize,
    pub group_count: usize,
    pub rule_count: usize,
    pub groups_mode: GroupsMode,
    pub regions: Vec<RegionStat>,
    pub unmatched: Vec<String>,
}

pub async fn convert(
    data: &AppStateData,
    http: &reqwest::Client,
    region_extras: &[RegionEntry],
) -> Result<ConvertResult> {
    let urls = data.profile.urls();
    if urls.is_empty() {
        bail!("未配置上游订阅 URL");
    }

    let mut root_meta = Value::Mapping(serde_yaml::Mapping::new());
    let mut all_proxies = Vec::new();
    let mut seen_names = HashSet::new();
    let mut fetch_errors = Vec::new();

    for (idx, url) in urls.iter().enumerate() {
        match fetch_upstream(http, url, &data.profile.user_agent).await {
            Ok(upstream) => {
                let mut root: Value = serde_yaml::from_str(&upstream)
                    .with_context(|| format!("订阅 #{} 不是合法 YAML", idx + 1))?;
                if idx == 0 {
                    root_meta = root.clone();
                }
                let proxies = extract_proxies(&mut root)
                    .with_context(|| format!("订阅 #{} 缺少 proxies", idx + 1))?;
                let mut added = 0usize;
                for p in proxies {
                    let name = p
                        .as_mapping()
                        .and_then(|m| m.get(Value::String("name".into())))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    if seen_names.insert(name) {
                        all_proxies.push(p);
                        added += 1;
                    }
                }
                info!(url = %url, added, total = all_proxies.len(), "已合并上游节点");
            }
            Err(err) => {
                warn!(url = %url, error = %err, "拉取上游失败");
                fetch_errors.push(format!("#{} {}: {}", idx + 1, url, err));
            }
        }
    }

    if all_proxies.is_empty() {
        if fetch_errors.is_empty() {
            bail!("所有上游均无可用节点");
        }
        bail!("拉取上游失败: {}", fetch_errors.join(" | "));
    }

    let root = root_meta;
    let proxy_names = proxy_names(&all_proxies);
    info!(count = proxy_names.len(), sources = urls.len(), "聚合完成");
    let mut landing_names = Vec::new();
    for landing in data.landings.iter().filter(|l| l.enabled) {
        if landing.server.trim().is_empty() || landing.port == 0 {
            warn!(name = %landing.name, "跳过无效落地代理");
            continue;
        }
        all_proxies.push(landing_to_value(landing));
        landing_names.push(landing.name.clone());
    }

    let (effective_groups, plan_meta) = resolve_groups(data, &proxy_names, region_extras)?;
    let default_group = match data.groups_mode {
        GroupsMode::Managed => NAME_DEFAULT.to_string(),
        GroupsMode::Custom => data.profile.default_group.clone(),
    };

    let mut groups_yaml = Vec::new();
    for g in &effective_groups {
        let mut members = Vec::new();
        if !g.filter.trim().is_empty() {
            let re = Regex::new(&g.filter)
                .with_context(|| format!("策略组「{}」正则无效: {}", g.name, g.filter))?;
            for name in &proxy_names {
                if re.is_match(name) {
                    members.push(Value::String(name.clone()));
                }
            }
        }
        for p in &g.proxies {
            if !members.iter().any(|m| m.as_str() == Some(p.as_str())) {
                members.push(Value::String(p.clone()));
            }
        }
        // 落地 SOCKS5/HTTP：挂到「链路」与「默认」，可独立选用（无 dialer 时直连该落地）
        let inject_landings = g.name.contains('链')
            || g.id == "g-chain"
            || g.name.contains("默认")
            || g.id == "g-default";
        if inject_landings {
            for n in landing_names.iter().rev() {
                if !members.iter().any(|m| m.as_str() == Some(n.as_str())) {
                    members.insert(0, Value::String(n.clone()));
                }
            }
        }
        if members.is_empty() {
            members.push(Value::String("DIRECT".into()));
        }

        let mut map = serde_yaml::Mapping::new();
        map.insert(
            Value::String("name".into()),
            Value::String(g.name.clone()),
        );
        map.insert(
            Value::String("type".into()),
            Value::String(g.group_type.as_clash().into()),
        );
        map.insert(Value::String("proxies".into()), Value::Sequence(members));
        if matches!(g.group_type, GroupType::UrlTest) {
            map.insert(
                Value::String("url".into()),
                Value::String(g.url.clone()),
            );
            map.insert(
                Value::String("interval".into()),
                Value::Number(g.interval.into()),
            );
            map.insert(
                Value::String("tolerance".into()),
                Value::Number(g.tolerance.into()),
            );
            map.insert(Value::String("lazy".into()), Value::Bool(g.lazy));
        }
        groups_yaml.push(Value::Mapping(map));
    }

    let rules = build_rules(data, &default_group);
    let rule_count = rules.len();

    let mut out = serde_yaml::Mapping::new();
    // 保留上游部分基础字段（若有）
    copy_if_present(&root, &mut out, "mixed-port");
    copy_if_present(&root, &mut out, "port");
    copy_if_present(&root, &mut out, "socks-port");
    copy_if_present(&root, &mut out, "allow-lan");
    copy_if_present(&root, &mut out, "mode");
    copy_if_present(&root, &mut out, "log-level");
    copy_if_present(&root, &mut out, "ipv6");
    copy_if_present(&root, &mut out, "dns");
    if !out.contains_key(Value::String("mode".into())) {
        out.insert(
            Value::String("mode".into()),
            Value::String("rule".into()),
        );
    }

    out.insert(
        Value::String("proxies".into()),
        Value::Sequence(all_proxies),
    );
    out.insert(
        Value::String("proxy-groups".into()),
        Value::Sequence(groups_yaml.clone()),
    );
    out.insert(
        Value::String("rules".into()),
        Value::Sequence(rules.into_iter().map(Value::String).collect()),
    );

    let yaml = serde_yaml::to_string(&Value::Mapping(out)).context("序列化 YAML 失败")?;
    let (regions, unmatched) = plan_meta;
    Ok(ConvertResult {
        proxy_count: proxy_names.len() + landing_names.len(),
        group_count: groups_yaml.len(),
        rule_count,
        yaml,
        groups_mode: data.groups_mode,
        regions,
        unmatched,
    })
}

/// 解析本次转换实际使用的策略组；managed 模式按节点自适应生成。
fn resolve_groups(
    data: &AppStateData,
    proxy_names: &[String],
    region_extras: &[RegionEntry],
) -> Result<(Vec<crate::model::ProxyGroup>, (Vec<RegionStat>, Vec<String>))> {
    match data.groups_mode {
        GroupsMode::Managed => {
            let plan: ManagedPlan =
                regions::build_managed_groups(proxy_names, region_extras, &data.groups)?;
            info!(
                regions = plan.regions.len(),
                unmatched = plan.unmatched.len(),
                groups = plan.groups.len(),
                extras = region_extras.len(),
                "托管策略组已按订阅节点生成（国家自动，用户策略组自定义）"
            );
            Ok((plan.groups, (plan.regions, plan.unmatched)))
        }
        GroupsMode::Custom => {
            if data.groups.is_empty() {
                anyhow::bail!("自定义模式未配置任何策略组");
            }
            Ok((data.groups.clone(), (Vec::new(), Vec::new())))
        }
    }
}

pub fn config_cache_key(data: &AppStateData, region_extras: &[RegionEntry]) -> String {
    let raw = serde_json::to_string(data).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    // 国家缓存刷新后使 /sub 缓存失效（转换本身不读盘、不联网）
    hasher.update(region_extras.len().to_le_bytes());
    for e in region_extras {
        hasher.update(e.id.as_bytes());
        hasher.update(e.filter.as_bytes());
    }
    hex::encode(hasher.finalize())
}

async fn fetch_upstream(http: &reqwest::Client, url: &str, ua: &str) -> Result<String> {
    let resp = http
        .get(url)
        .header("User-Agent", ua)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .context("拉取上游订阅失败")?;
    if !resp.status().is_success() {
        bail!("上游订阅 HTTP {}", resp.status());
    }
    let bytes = resp.bytes().await.context("读取上游订阅正文失败")?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    if looks_like_yaml(&text) {
        return Ok(text);
    }
    // 部分订阅整包 base64
    use base64::Engine as _;
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(text.trim()) {
        let decoded_text = String::from_utf8_lossy(&decoded).to_string();
        if looks_like_yaml(&decoded_text) {
            return Ok(decoded_text);
        }
    }
    Ok(text)
}

fn looks_like_yaml(s: &str) -> bool {
    let t = s.trim_start();
    t.contains("proxies:") || t.contains("proxy-groups:") || t.starts_with("port:")
}

fn extract_proxies(root: &mut Value) -> Result<Vec<Value>> {
    let Some(map) = root.as_mapping_mut() else {
        bail!("上游根节点不是 mapping");
    };
    let key = Value::String("proxies".into());
    let Some(proxies) = map.remove(&key) else {
        bail!("上游缺少 proxies");
    };
    let Value::Sequence(seq) = proxies else {
        bail!("proxies 不是列表");
    };
    Ok(seq)
}

fn proxy_names(proxies: &[Value]) -> Vec<String> {
    proxies
        .iter()
        .filter_map(|p| {
            p.as_mapping()
                .and_then(|m| m.get(Value::String("name".into())))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

fn landing_to_value(landing: &crate::model::LandingProxy) -> Value {
    let mut map = serde_yaml::Mapping::new();
    map.insert(
        Value::String("name".into()),
        Value::String(landing.name.clone()),
    );
    map.insert(
        Value::String("type".into()),
        Value::String(landing.landing_type.as_clash().into()),
    );
    map.insert(
        Value::String("server".into()),
        Value::String(landing.server.clone()),
    );
    map.insert(
        Value::String("port".into()),
        Value::Number(landing.port.into()),
    );
    if !landing.username.is_empty() {
        map.insert(
            Value::String("username".into()),
            Value::String(landing.username.clone()),
        );
    }
    if !landing.password.is_empty() {
        map.insert(
            Value::String("password".into()),
            Value::String(landing.password.clone()),
        );
    }
    if !landing.dialer_proxy.is_empty() {
        map.insert(
            Value::String("dialer-proxy".into()),
            Value::String(landing.dialer_proxy.clone()),
        );
    }
    if matches!(landing.landing_type, LandingType::Socks5) {
        map.insert(Value::String("udp".into()), Value::Bool(true));
    }
    Value::Mapping(map)
}

fn build_rules(data: &AppStateData, default_group: &str) -> Vec<String> {
    let mut rules = Vec::new();
    // 局域网源 IP 规则置顶，整机覆盖后续域名规则
    for route in data.lan_routes.iter().filter(|r| r.enabled) {
        let target = route.target.trim();
        if target.is_empty() {
            continue;
        }
        if let Some(cidr) = normalize_src_cidr(&route.src) {
            rules.push(format!("SRC-IP-CIDR,{cidr},{target}"));
        } else {
            warn!(src = %route.src, "跳过无效局域网源地址");
        }
    }
    for rs in data.rulesets.iter().filter(|r| r.enabled) {
        for line in rs.rules.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // 已带策略名则保留，否则追加组名
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 3
                && !parts[0].eq_ignore_ascii_case("AND")
                && !parts[0].eq_ignore_ascii_case("OR")
                && !parts[0].eq_ignore_ascii_case("NOT")
            {
                // DOMAIN-SUFFIX,x,GROUP or GEOIP,CN,DIRECT
                rules.push(line.to_string());
            } else {
                rules.push(format!("{},{}", line, rs.group));
            }
        }
    }
    rules.push("GEOIP,CN,DIRECT".into());
    rules.push("GEOSITE,CN,DIRECT".into());
    rules.push(format!("MATCH,{default_group}"));
    rules
}

/// 将 IP 或 CIDR 规范为 Clash SRC-IP-CIDR 可用形式
fn normalize_src_cidr(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains('/') {
        return Some(s.to_string());
    }
    if s.contains(':') {
        // IPv6 单主机
        return Some(format!("{s}/128"));
    }
    // 粗判 IPv4：四段数字
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) {
        return Some(format!("{s}/32"));
    }
    None
}

fn copy_if_present(root: &Value, out: &mut serde_yaml::Mapping, key: &str) {
    if let Some(map) = root.as_mapping() {
        let k = Value::String(key.into());
        if let Some(v) = map.get(&k) {
            out.insert(k, v.clone());
        }
    }
}

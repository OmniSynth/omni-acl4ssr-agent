//! 公开国家列表拉取与磁盘缓存。
//!
//! - 常见地区仍由 `regions::catalog()` 硬编码优先匹配
//! - 其余国家从 mledoze/countries（REST Countries 同源公开 JSON）补全
//! - 转换路径只读内存快照，绝不在 convert 里发网络请求
//! - 启动时异步刷新；TTL 默认 30 天（国家变动极少）

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::regions::{builtin_ids, RegionEntry};

const CACHE_VERSION: u32 = 1;
const DEFAULT_TTL_SECS: u64 = 30 * 24 * 3600; // 30 天
const CACHE_FILE: &str = "countries-cache.json";

/// jsDelivr 国内镜像（优先）
const SOURCE_MIRROR: &str =
    "https://cdn.jsdmirror.com/gh/mledoze/countries@master/countries.json";
const SOURCE_PRIMARY: &str =
    "https://cdn.jsdelivr.net/gh/mledoze/countries@master/countries.json";
const SOURCE_FALLBACK: &str =
    "https://raw.githubusercontent.com/mledoze/countries/master/countries.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    fetched_at_unix: u64,
    source: String,
    regions: Vec<RegionEntry>,
}

#[derive(Debug, Deserialize)]
struct ApiCountry {
    cca2: String,
    #[serde(default)]
    cca3: String,
    #[serde(default)]
    flag: String,
    name: ApiName,
    #[serde(default, rename = "altSpellings")]
    alt_spellings: Vec<String>,
    #[serde(default)]
    translations: std::collections::HashMap<String, ApiTranslation>,
    #[serde(default)]
    capital: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ApiName {
    common: String,
    #[serde(default)]
    official: String,
}

#[derive(Debug, Deserialize)]
struct ApiTranslation {
    #[serde(default)]
    common: String,
    #[serde(default)]
    official: String,
}

#[derive(Clone)]
pub struct WorldCatalog {
    path: PathBuf,
    http: reqwest::Client,
    ttl: Duration,
    inner: Arc<RwLock<Vec<RegionEntry>>>,
}

impl WorldCatalog {
    pub async fn open(data_dir: impl AsRef<Path>, http: reqwest::Client) -> Self {
        let path = data_dir.as_ref().join(CACHE_FILE);
        let ttl_secs = std::env::var("OMNI_COUNTRIES_TTL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TTL_SECS);
        let regions = load_cache(&path).unwrap_or_default();
        if !regions.is_empty() {
            info!(count = regions.len(), path = %path.display(), "已加载国家缓存");
        } else {
            info!(path = %path.display(), "国家缓存为空，将在后台拉取");
        }
        Self {
            path,
            http,
            ttl: Duration::from_secs(ttl_secs),
            inner: Arc::new(RwLock::new(regions)),
        }
    }

    /// 转换用：内存快照，无 IO / 无网络。
    pub async fn extras(&self) -> Vec<RegionEntry> {
        self.inner.read().await.clone()
    }

    /// 启动后调用：缓存缺失或过期时后台刷新，不阻塞服务。
    pub fn spawn_background_refresh(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = this.refresh_if_needed().await {
                warn!(error = %err, "国家列表刷新失败（将仅使用内置常见地区）");
            }
        });
    }

    pub async fn refresh_if_needed(&self) -> Result<()> {
        if !self.needs_refresh().await {
            info!("国家缓存仍在有效期内，跳过刷新");
            return Ok(());
        }
        self.refresh_now().await
    }

    async fn needs_refresh(&self) -> bool {
        let meta = read_cache_meta(&self.path);
        match meta {
            None => true,
            Some((fetched_at, count)) => {
                count == 0 || now_unix().saturating_sub(fetched_at) >= self.ttl.as_secs()
            }
        }
    }

    pub async fn refresh_now(&self) -> Result<()> {
        let (source, regions) = fetch_regions(&self.http).await?;
        let cache = CacheFile {
            version: CACHE_VERSION,
            fetched_at_unix: now_unix(),
            source: source.clone(),
            regions: regions.clone(),
        };
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let pretty = serde_json::to_string_pretty(&cache).context("序列化国家缓存失败")?;
        tokio::fs::write(&self.path, pretty)
            .await
            .with_context(|| format!("写入国家缓存失败: {}", self.path.display()))?;
        *self.inner.write().await = regions.clone();
        info!(
            count = regions.len(),
            source = %source,
            path = %self.path.display(),
            "国家列表已刷新并写入缓存"
        );
        Ok(())
    }
}

fn load_cache(path: &Path) -> Option<Vec<RegionEntry>> {
    let raw = std::fs::read_to_string(path).ok()?;
    let cache: CacheFile = serde_json::from_str(&raw).ok()?;
    if cache.version != CACHE_VERSION {
        return None;
    }
    Some(cache.regions)
}

fn read_cache_meta(path: &Path) -> Option<(u64, usize)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let cache: CacheFile = serde_json::from_str(&raw).ok()?;
    if cache.version != CACHE_VERSION {
        return None;
    }
    Some((cache.fetched_at_unix, cache.regions.len()))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn fetch_regions(http: &reqwest::Client) -> Result<(String, Vec<RegionEntry>)> {
    let custom = std::env::var("OMNI_COUNTRIES_URL").ok().filter(|s| !s.trim().is_empty());
    let urls: Vec<String> = if let Some(u) = custom {
        vec![u]
    } else {
        vec![
            SOURCE_MIRROR.into(),
            SOURCE_PRIMARY.into(),
            SOURCE_FALLBACK.into(),
        ]
    };

    let mut last_err = None;
    for url in &urls {
        match fetch_one(http, url).await {
            Ok(regions) => return Ok((url.clone(), regions)),
            Err(err) => {
                warn!(url = %url, error = %err, "拉取国家列表失败，尝试下一源");
                last_err = Some(err);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("无可用国家数据源")))
}

async fn fetch_one(http: &reqwest::Client, url: &str) -> Result<Vec<RegionEntry>> {
    let resp = http
        .get(url)
        .header("User-Agent", "omni-acl4ssr-agent/countries-cache")
        .timeout(Duration::from_secs(45))
        .send()
        .await
        .context("请求国家列表失败")?;
    if !resp.status().is_success() {
        bail!("国家列表 HTTP {}", resp.status());
    }
    let body = resp.bytes().await.context("读取国家列表失败")?;
    let list: Vec<ApiCountry> =
        serde_json::from_slice(&body).context("解析国家列表 JSON 失败")?;
    Ok(countries_to_regions(&list))
}

fn countries_to_regions(list: &[ApiCountry]) -> Vec<RegionEntry> {
    let skip = builtin_ids();
    // 大陆节点极少作为机场「出国」组；且易与规则文案误匹配
    let mut skip = skip;
    skip.insert("cn".into());

    let mut out = Vec::new();
    for c in list {
        let id = c.cca2.to_ascii_lowercase();
        if id.len() != 2 || skip.contains(&id) {
            continue;
        }
        let Some(entry) = api_to_entry(c) else {
            continue;
        };
        out.push(entry);
    }
    // 更长的中文名优先，降低「南」类短词误伤（匹配时仍按列表顺序）
    out.sort_by(|a, b| {
        let la = a.name.chars().count();
        let lb = b.name.chars().count();
        lb.cmp(&la).then_with(|| a.id.cmp(&b.id))
    });
    out
}

fn api_to_entry(c: &ApiCountry) -> Option<RegionEntry> {
    let id = c.cca2.to_ascii_lowercase();
    let zho = c.translations.get("zho");
    let zho_common = zho.map(|t| t.common.trim()).filter(|s| !s.is_empty());
    let zho_official = zho.map(|t| t.official.trim()).filter(|s| !s.is_empty());

    let label = zho_common
        .or(zho_official)
        .unwrap_or(c.name.common.trim());
    if label.is_empty() {
        return None;
    }

    let flag = c.flag.trim();
    let name = if flag.is_empty() {
        label.to_string()
    } else {
        format!("{flag} {label}")
    };

    let mut alts: Vec<String> = Vec::new();
    push_alt(&mut alts, zho_common);
    push_alt(&mut alts, zho_official);
    push_alt(&mut alts, Some(c.name.common.trim()).filter(|s| s.len() >= 4));
    push_alt(
        &mut alts,
        Some(c.name.official.trim()).filter(|s| s.len() >= 6),
    );
    for a in &c.alt_spellings {
        let t = a.trim();
        // 跳过纯 ISO 短码，改由下方 cca2 规则处理
        if t.len() >= 4 {
            push_alt(&mut alts, Some(t));
        }
    }
    for cap in &c.capital {
        let t = cap.trim();
        if t.len() >= 4 {
            push_alt(&mut alts, Some(t));
        }
    }
    if !flag.is_empty() {
        push_alt(&mut alts, Some(flag));
    }

    // ISO 码：要求后面跟数字或作为独立词，降低误匹配
    let cca2 = c.cca2.to_ascii_uppercase();
    if cca2.len() == 2 && cca2.chars().all(|ch| ch.is_ascii_alphabetic()) {
        alts.push(format!(r"\b{cca2}\d*\b"));
    }
    let cca3 = c.cca3.to_ascii_uppercase();
    if cca3.len() == 3 && cca3.chars().all(|ch| ch.is_ascii_alphabetic()) {
        alts.push(format!(r"\b{cca3}\b"));
    }

    if alts.is_empty() {
        return None;
    }

    let filter = format!("(?i){}", alts.join("|"));
    // 验证正则可编译
    if regex::Regex::new(&filter).is_err() {
        return None;
    }

    Some(RegionEntry { id, name, filter })
}

fn push_alt(out: &mut Vec<String>, raw: Option<&str>) {
    let Some(s) = raw else { return };
    let s = s.trim();
    if s.is_empty() {
        return;
    }
    let esc = regex_escape(s);
    if out.iter().any(|x| x == &esc) {
        return;
    }
    out.push(esc);
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' => {
                out.push('\\');
                out.push(ch);
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_poland_from_api_shape() {
        let raw = r#"[{
            "cca2":"PL","cca3":"POL","flag":"🇵🇱",
            "name":{"common":"Poland","official":"Republic of Poland"},
            "altSpellings":["PL","Republic of Poland"],
            "translations":{"zho":{"official":"波兰共和国","common":"波兰"}},
            "capital":["Warsaw"]
        }]"#;
        let list: Vec<ApiCountry> = serde_json::from_str(raw).unwrap();
        let regions = countries_to_regions(&list);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].id, "pl");
        assert!(regions[0].name.contains("波兰"));
        let re = regex::Regex::new(&regions[0].filter).unwrap();
        assert!(re.is_match("🇵🇱15波兰-优化"));
        assert!(re.is_match("Poland-01"));
    }

    #[test]
    fn skips_builtin_and_china() {
        let raw = r#"[
          {"cca2":"US","cca3":"USA","flag":"🇺🇸","name":{"common":"United States","official":"United States of America"},"altSpellings":[],"translations":{"zho":{"common":"美国","official":"美利坚合众国"}},"capital":["Washington, D.C."]},
          {"cca2":"CN","cca3":"CHN","flag":"🇨🇳","name":{"common":"China","official":"People's Republic of China"},"altSpellings":[],"translations":{"zho":{"common":"中国","official":"中华人民共和国"}},"capital":["Beijing"]},
          {"cca2":"NO","cca3":"NOR","flag":"🇳🇴","name":{"common":"Norway","official":"Kingdom of Norway"},"altSpellings":["NO"],"translations":{"zho":{"common":"挪威","official":"挪威王国"}},"capital":["Oslo"]}
        ]"#;
        let list: Vec<ApiCountry> = serde_json::from_str(raw).unwrap();
        let regions = countries_to_regions(&list);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].id, "no");
    }
}

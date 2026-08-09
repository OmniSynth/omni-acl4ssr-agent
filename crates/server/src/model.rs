use serde::{Deserialize, Serialize};

use crate::regions::{RegionStat, NAME_DEFAULT};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupsMode {
    /// 按订阅节点自动识别地区并编排策略组（默认，适合不想折腾的用户）
    #[default]
    Managed,
    /// 完全使用下方手动配置的策略组
    Custom,
}

impl GroupsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateData {
    pub profile: Profile,
    /// 策略组模式：managed=自动编排，custom=手写 groups
    #[serde(default)]
    pub groups_mode: GroupsMode,
    /// 自定义策略组（custom 模式使用；managed 下仍保留，便于切回）
    pub groups: Vec<ProxyGroup>,
    pub rulesets: Vec<RuleSet>,
    pub landings: Vec<LandingProxy>,
    /// 局域网设备按源 IP 走指定策略组/节点
    #[serde(default)]
    pub lan_routes: Vec<LanRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// 兼容旧字段：单订阅 URL（读取后会并入 upstream_urls）
    #[serde(default)]
    pub upstream_url: String,
    /// 多上游订阅，转换时聚合节点（同名保留先出现的）
    #[serde(default)]
    pub upstream_urls: Vec<String>,
    /// MATCH 默认出口策略组名
    pub default_group: String,
    pub enabled: bool,
    /// 拉取订阅时的 User-Agent
    #[serde(default = "default_ua")]
    pub user_agent: String,
}

impl Profile {
    /// 归一化：把旧 upstream_url 并入列表，去空去重
    pub fn normalize(&mut self) {
        let mut urls = Vec::new();
        for u in self
            .upstream_urls
            .iter()
            .chain(std::iter::once(&self.upstream_url))
        {
            let t = u.trim();
            if t.is_empty() {
                continue;
            }
            if !urls.iter().any(|x: &String| x == t) {
                urls.push(t.to_string());
            }
        }
        self.upstream_urls = urls;
        self.upstream_url = self
            .upstream_urls
            .first()
            .cloned()
            .unwrap_or_default();
    }

    pub fn urls(&self) -> Vec<String> {
        let mut p = self.clone();
        p.normalize();
        p.upstream_urls
    }
}

fn default_ua() -> String {
    "clash.meta/omni-acl4ssr-agent".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupType {
    Select,
    UrlTest,
}

impl GroupType {
    pub fn as_clash(&self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::UrlTest => "url-test",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyGroup {
    pub id: String,
    pub name: String,
    pub group_type: GroupType,
    /// 匹配节点名的正则；空表示手动组（仅含其它组/DIRECT 等 proxies 字段）
    #[serde(default)]
    pub filter: String,
    /// select 组可额外引用的组名或特殊值（DIRECT / REJECT）
    #[serde(default)]
    pub proxies: Vec<String>,
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_tolerance")]
    pub tolerance: u64,
    #[serde(default = "default_true")]
    pub lazy: bool,
}

fn default_url() -> String {
    "https://www.gstatic.com/generate_204".to_string()
}
fn default_interval() -> u64 {
    300
}
fn default_tolerance() -> u64 {
    50
}
fn default_true() -> bool {
    true
}

/// 国家/地区 url-test 策略组骨架
fn url_test_group(id: &str, name: &str, filter: &str) -> ProxyGroup {
    ProxyGroup {
        id: id.into(),
        name: name.into(),
        group_type: GroupType::UrlTest,
        filter: filter.into(),
        proxies: vec![],
        url: default_url(),
        interval: 300,
        tolerance: 50,
        lazy: true,
    }
}

fn select_group(id: &str, name: &str, proxies: Vec<String>) -> ProxyGroup {
    ProxyGroup {
        id: id.into(),
        name: name.into(),
        group_type: GroupType::Select,
        filter: String::new(),
        proxies,
        url: default_url(),
        interval: 300,
        tolerance: 50,
        lazy: true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSet {
    pub id: String,
    pub name: String,
    /// 绑定的策略组名
    pub group: String,
    /// Clash 规则载荷（不含末尾策略名），每行一条，如 DOMAIN-SUFFIX,openai.com
    pub rules: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// 局域网源 IP → 策略组/节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanRoute {
    pub id: String,
    /// 备注（如「客厅电视」）
    #[serde(default)]
    pub name: String,
    /// 源 IP 或 CIDR，如 172.16.1.50 / 172.16.1.0/24
    pub src: String,
    /// 目标策略组名或节点名（含 DIRECT / REJECT）
    pub target: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandingType {
    Socks5,
    Http,
}

impl LandingType {
    pub fn as_clash(&self) -> &'static str {
        match self {
            Self::Socks5 => "socks5",
            Self::Http => "http",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandingProxy {
    pub id: String,
    pub name: String,
    pub landing_type: LandingType,
    pub server: String,
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// 前置策略组或节点名（dialer-proxy）
    #[serde(default)]
    pub dialer_proxy: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl AppStateData {
    pub fn default_skeleton() -> Self {
        Self {
            profile: Profile {
                name: "default".to_string(),
                upstream_url: String::new(),
                upstream_urls: vec![],
                default_group: NAME_DEFAULT.to_string(),
                enabled: true,
                user_agent: default_ua(),
            },
            groups_mode: GroupsMode::Managed,
            groups: {
                let countries = vec![
                    url_test_group("g-hk", "🇭🇰 香港", r"(?i)香港|Hong Kong"),
                    url_test_group("g-tw", "🇹🇼 台湾", r"(?i)台湾|Taiwan"),
                    url_test_group("g-jp", "🇯🇵 日本", r"(?i)日本|Japan"),
                    url_test_group("g-sg", "🇸🇬 新加坡", r"(?i)新加坡|Singapore|狮城"),
                    url_test_group("g-kr", "🇰🇷 韩国", r"(?i)韩国|Korea|首尔"),
                    url_test_group(
                        "g-us",
                        "🇺🇸 美国",
                        r"(?i)美国|Gemini|United States",
                    ),
                    url_test_group("g-ca", "🇨🇦 加拿大", r"(?i)加拿大|Canada"),
                    url_test_group("g-in", "🇮🇳 印度", r"(?i)印度|India"),
                    url_test_group(
                        "g-au",
                        "🇦🇺 澳大利亚",
                        r"(?i)澳大利亚|澳洲|Australia",
                    ),
                ];
                let country_names: Vec<String> =
                    countries.iter().map(|g| g.name.clone()).collect();
                let mut groups = countries;
                groups.extend([
                    select_group(
                        "g-ai",
                        "🤖 AI",
                        vec![
                            "🇺🇸 美国".into(),
                            "🇯🇵 日本".into(),
                            "🇸🇬 新加坡".into(),
                            "🇭🇰 香港".into(),
                        ],
                    ),
                    select_group(
                        "g-binance",
                        "💰 币安",
                        vec![
                            "🇭🇰 香港".into(),
                            "🇸🇬 新加坡".into(),
                            "🇯🇵 日本".into(),
                            "🇺🇸 美国".into(),
                        ],
                    ),
                    select_group(
                        "g-netflix",
                        "📺 奈飞",
                        vec![
                            "🇸🇬 新加坡".into(),
                            "🇯🇵 日本".into(),
                            "🇹🇼 台湾".into(),
                            "🇺🇸 美国".into(),
                            "🇭🇰 香港".into(),
                        ],
                    ),
                    select_group("g-chain", "⛓ 链路", vec!["🇭🇰 香港".into()]),
                    {
                        let mut def = country_names;
                        def.push("DIRECT".into());
                        select_group("g-default", NAME_DEFAULT, def)
                    },
                ]);
                groups
            },
            rulesets: vec![
                RuleSet {
                    id: "r-ai".into(),
                    name: "AI".into(),
                    group: "🤖 AI".into(),
                    enabled: true,
                    rules: [
                        "DOMAIN-SUFFIX,openai.com",
                        "DOMAIN-SUFFIX,chatgpt.com",
                        "DOMAIN-SUFFIX,anthropic.com",
                        "DOMAIN-SUFFIX,claude.ai",
                        "DOMAIN-SUFFIX,cursor.sh",
                        "DOMAIN-SUFFIX,cursor.com",
                        "DOMAIN-KEYWORD,gemini",
                        "DOMAIN-SUFFIX,googleapis.com",
                        "DOMAIN-SUFFIX,deepseek.com",
                        "DOMAIN-SUFFIX,api.deepseek.com",
                        "GEOSITE,openai",
                    ]
                    .join("\n"),
                },
                RuleSet {
                    id: "r-binance".into(),
                    name: "币安".into(),
                    group: "💰 币安".into(),
                    enabled: true,
                    rules: [
                        "DOMAIN-SUFFIX,binance.com",
                        "DOMAIN-SUFFIX,binance.me",
                        "DOMAIN-SUFFIX,bnapp.net",
                        "DOMAIN-SUFFIX,bnbstatic.com",
                        "DOMAIN-KEYWORD,binance",
                    ]
                    .join("\n"),
                },
                RuleSet {
                    id: "r-netflix".into(),
                    name: "奈飞".into(),
                    group: "📺 奈飞".into(),
                    enabled: true,
                    rules: [
                        "DOMAIN-SUFFIX,netflix.com",
                        "DOMAIN-SUFFIX,netflix.net",
                        "DOMAIN-SUFFIX,nflxvideo.net",
                        "DOMAIN-SUFFIX,nflxso.net",
                        "DOMAIN-SUFFIX,nflximg.net",
                        "GEOSITE,netflix",
                    ]
                    .join("\n"),
                },
            ],
            landings: vec![],
            lan_routes: vec![],
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ConvertResponse {
    pub ok: bool,
    pub proxy_count: usize,
    pub group_count: usize,
    pub rule_count: usize,
    pub yaml: Option<String>,
    pub message: String,
    pub groups_mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<RegionStat>,
    #[serde(default)]
    pub unmatched_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmatched_samples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupsModeBody {
    pub groups_mode: GroupsMode,
}

#[derive(Debug, Deserialize)]
pub struct ConvertRequest {
    #[serde(default)]
    pub include_yaml: bool,
}

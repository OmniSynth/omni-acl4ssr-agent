//! 订阅节点地区识别与托管策略组生成。
//!
//! `groups_mode = managed` 时：只生成有节点的地区组，AI/币安/奈飞/默认按可用地区自动编排。
//! 常见地区见 `catalog()`；其余国家由 `countries` 模块缓存补全。

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::model::{GroupType, ProxyGroup};

#[derive(Debug, Clone, Copy)]
pub struct RegionDef {
    pub id: &'static str,
    pub name: &'static str,
    pub filter: &'static str,
}

/// 运行时地区条目（内置或公开 API 缓存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionEntry {
    pub id: String,
    pub name: String,
    pub filter: String,
}

/// 地区目录：靠前的条目优先匹配（更具体的别名放前面）。
pub fn catalog() -> &'static [RegionDef] {
    &[
        RegionDef {
            id: "hk",
            name: "🇭🇰 香港",
            filter: r"(?i)香港|香港节点|Hong\s*Kong|\bHKG\b|\bHK\d*\b|🇭🇰",
        },
        RegionDef {
            id: "tw",
            name: "🇹🇼 台湾",
            filter: r"(?i)台湾|台灣|Taiwan|\bTPE\b|\bTW\d*\b|台北|臺北|🇹🇼",
        },
        RegionDef {
            id: "jp",
            name: "🇯🇵 日本",
            filter: r"(?i)日本|Japan|\bJP\d*\b|东京|東京|大阪|名古屋|🇯🇵",
        },
        RegionDef {
            id: "sg",
            name: "🇸🇬 新加坡",
            filter: r"(?i)新加坡|Singapore|狮城|\bSGP\b|\bSG\d*\b|🇸🇬",
        },
        RegionDef {
            id: "kr",
            name: "🇰🇷 韩国",
            filter: r"(?i)韩国|韓國|Korea|首尔|首爾|\bKR\d*\b|\bICN\b|🇰🇷",
        },
        RegionDef {
            id: "us",
            name: "🇺🇸 美国",
            filter: r"(?i)美国|美國|United\s*States|\bUSA\b|\bUS\d*\b|洛杉矶|紐約|纽约|硅谷|西雅图|芝加哥|Gemini|🇺🇸",
        },
        RegionDef {
            id: "gb",
            name: "🇬🇧 英国",
            filter: r"(?i)英国|英國|\bUK\b|United\s*Kingdom|伦敦|倫敦|🇬🇧",
        },
        RegionDef {
            id: "de",
            name: "🇩🇪 德国",
            filter: r"(?i)德国|德國|Germany|法兰克福|法蘭克福|\bDE\d*\b|🇩🇪",
        },
        RegionDef {
            id: "fr",
            name: "🇫🇷 法国",
            filter: r"(?i)法国|法國|France|巴黎|\bFR\d*\b|🇫🇷",
        },
        RegionDef {
            id: "nl",
            name: "🇳🇱 荷兰",
            filter: r"(?i)荷兰|荷蘭|Netherlands|阿姆斯特丹|\bNL\d*\b|🇳🇱",
        },
        RegionDef {
            id: "ca",
            name: "🇨🇦 加拿大",
            filter: r"(?i)加拿大|Canada|多伦多|溫哥华|温哥华|🇨🇦",
        },
        RegionDef {
            id: "au",
            name: "🇦🇺 澳大利亚",
            filter: r"(?i)澳大利亚|澳洲|Australia|悉尼|墨尔本|\bAU\d*\b|🇦🇺",
        },
        RegionDef {
            id: "in",
            name: "🇮🇳 印度",
            filter: r"(?i)印度|India|孟买|mumbai|\bIN\d+\b|🇮🇳",
        },
        RegionDef {
            id: "tr",
            name: "🇹🇷 土耳其",
            filter: r"(?i)土耳其|Turkey|Istanbul|伊斯坦|\bTR\d*\b|🇹🇷",
        },
        RegionDef {
            id: "br",
            name: "🇧🇷 巴西",
            filter: r"(?i)巴西|Brazil|圣保罗|聖保羅|\bBR\d*\b|🇧🇷",
        },
        RegionDef {
            id: "ar",
            name: "🇦🇷 阿根廷",
            filter: r"(?i)阿根廷|Argentina|Buenos|\bAR\d*\b|🇦🇷",
        },
        RegionDef {
            id: "ru",
            name: "🇷🇺 俄罗斯",
            filter: r"(?i)俄罗斯|俄羅斯|Russia|莫斯科|\bRU\d*\b|🇷🇺",
        },
        RegionDef {
            id: "ph",
            name: "🇵🇭 菲律宾",
            filter: r"(?i)菲律宾|菲律賓|Philippines|马尼拉|\bPH\d*\b|🇵🇭",
        },
        RegionDef {
            id: "th",
            name: "🇹🇭 泰国",
            filter: r"(?i)泰国|泰國|Thailand|曼谷|\bTH\d*\b|🇹🇭",
        },
        RegionDef {
            id: "my",
            name: "🇲🇾 马来西亚",
            filter: r"(?i)马来|馬來|Malaysia|吉隆坡|\bMY\d*\b|🇲🇾",
        },
        RegionDef {
            id: "vn",
            name: "🇻🇳 越南",
            filter: r"(?i)越南|Vietnam|胡志明|\bVN\d*\b|🇻🇳",
        },
        RegionDef {
            id: "id",
            name: "🇮🇩 印尼",
            filter: r"(?i)印尼|印度尼西亚|Indonesia|雅加达|\bID\d+\b|🇮🇩",
        },
        RegionDef {
            id: "ae",
            name: "🇦🇪 阿联酋",
            filter: r"(?i)阿联酋|迪拜|Dubai|UAE|Emirates|🇦🇪",
        },
        RegionDef {
            id: "mx",
            name: "🇲🇽 墨西哥",
            filter: r"(?i)墨西哥|Mexico|🇲🇽",
        },
        RegionDef {
            id: "es",
            name: "🇪🇸 西班牙",
            filter: r"(?i)西班牙|Spain|Madrid|🇪🇸",
        },
        RegionDef {
            id: "ch",
            name: "🇨🇭 瑞士",
            filter: r"(?i)瑞士|Switzerland|Zurich|🇨🇭",
        },
        RegionDef {
            id: "za",
            name: "🇿🇦 南非",
            filter: r"(?i)南非|South\s*Africa|约翰内斯堡|Johannesburg|🇿🇦",
        },
        RegionDef {
            id: "it",
            name: "🇮🇹 意大利",
            filter: r"(?i)意大利|Italy|米兰|罗马|🇮🇹",
        },
        RegionDef {
            id: "se",
            name: "🇸🇪 瑞典",
            filter: r"(?i)瑞典|Sweden|Stockholm|🇸🇪",
        },
    ]
}

pub fn builtin_ids() -> HashSet<String> {
    catalog().iter().map(|r| r.id.to_string()).collect()
}

fn builtin_entries() -> Vec<RegionEntry> {
    catalog()
        .iter()
        .map(|r| RegionEntry {
            id: r.id.into(),
            name: r.name.into(),
            filter: r.filter.into(),
        })
        .collect()
}

const PREF_AI: &[&str] = &["us", "jp", "sg", "hk", "tw", "kr", "gb", "de"];
const PREF_BINANCE: &[&str] = &["hk", "sg", "jp", "us", "tw"];
const PREF_NETFLIX: &[&str] = &["sg", "jp", "tw", "us", "hk", "kr"];
const PREF_CHAIN: &[&str] = &["hk", "sg", "jp", "tw", "us"];

pub const NAME_DEFAULT: &str = "🚀 默认";
pub const NAME_OTHER: &str = "🌐 其他";
pub const NAME_AI: &str = "🤖 AI";
pub const NAME_BINANCE: &str = "💰 币安";
pub const NAME_NETFLIX: &str = "📺 奈飞";
pub const NAME_CHAIN: &str = "⛓ 链路";

#[derive(Debug, Clone, Serialize)]
pub struct RegionStat {
    pub id: String,
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagedPlan {
    pub groups: Vec<ProxyGroup>,
    pub regions: Vec<RegionStat>,
    pub unmatched: Vec<String>,
}

fn url_test(id: &str, name: &str, filter: &str) -> ProxyGroup {
    ProxyGroup {
        id: id.into(),
        name: name.into(),
        group_type: GroupType::UrlTest,
        filter: filter.into(),
        proxies: vec![],
        url: "https://www.gstatic.com/generate_204".into(),
        interval: 300,
        tolerance: 50,
        lazy: true,
    }
}

fn select(id: &str, name: &str, proxies: Vec<String>) -> ProxyGroup {
    ProxyGroup {
        id: id.into(),
        name: name.into(),
        group_type: GroupType::Select,
        filter: String::new(),
        proxies,
        url: "https://www.gstatic.com/generate_204".into(),
        interval: 300,
        tolerance: 50,
        lazy: true,
    }
}

fn pick(prefs: &[&str], available: &HashMap<&str, &RegionEntry>, fallback: &[String]) -> Vec<String> {
    let mut out: Vec<String> = prefs
        .iter()
        .filter_map(|id| available.get(id).map(|r| r.name.clone()))
        .collect();
    if out.is_empty() {
        out.extend(fallback.iter().cloned());
    }
    out
}

/// 托管模式下的「用户策略组」：无 filter，且不是默认/链路/其他。
pub fn is_user_strategy_group(g: &ProxyGroup) -> bool {
    if matches!(g.id.as_str(), "g-default" | "g-chain" | "g-other") {
        return false;
    }
    g.filter.trim().is_empty()
}

fn default_prefs_for(id: &str, name: &str) -> &'static [&'static str] {
    if id == "g-ai" || name == NAME_AI {
        PREF_AI
    } else if id == "g-binance" || name == NAME_BINANCE {
        PREF_BINANCE
    } else if id == "g-netflix" || name == NAME_NETFLIX {
        PREF_NETFLIX
    } else {
        &[]
    }
}

/// 解析用户策略组成员：仅保留当前存在的地区组 / DIRECT / REJECT；全空则按组类型回退。
fn resolve_user_proxies(
    g: &ProxyGroup,
    available: &HashMap<&str, &RegionEntry>,
    region_names: &[String],
) -> Vec<String> {
    let allowed: HashSet<&str> = region_names
        .iter()
        .map(|s| s.as_str())
        .chain(["DIRECT", "REJECT"])
        .collect();
    let mut out: Vec<String> = Vec::new();
    for p in &g.proxies {
        let t = p.trim();
        if t.is_empty() || out.iter().any(|x| x == t) {
            continue;
        }
        if allowed.contains(t) {
            out.push(t.to_string());
        }
    }
    if !out.is_empty() {
        return out;
    }
    let prefs = default_prefs_for(&g.id, &g.name);
    if !prefs.is_empty() {
        return pick(prefs, available, region_names);
    }
    if region_names.is_empty() {
        vec!["DIRECT".into()]
    } else {
        region_names.to_vec()
    }
}

/// 按节点名生成托管策略组。
/// - 国家/地区组：按订阅自动识别
/// - 用户策略组（无 filter）：按配置输出，成员引用地区组名
/// `extras` 为公开 API 缓存的补充国家（已排除内置 id）；转换时只读内存传入。
pub fn build_managed_groups(
    proxy_names: &[String],
    extras: &[RegionEntry],
    custom_groups: &[ProxyGroup],
) -> Result<ManagedPlan> {
    let mut entries = builtin_entries();
    let builtin = builtin_ids();
    for e in extras {
        if !builtin.contains(&e.id) {
            entries.push(e.clone());
        }
    }

    let compiled: Vec<(RegionEntry, Regex)> = entries
        .into_iter()
        .map(|r| {
            Regex::new(&r.filter)
                .with_context(|| format!("地区正则无效 {}: {}", r.id, r.filter))
                .map(|re| (r, re))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut buckets: HashMap<String, Vec<String>> = HashMap::new();
    let mut unmatched = Vec::new();

    for name in proxy_names {
        let mut hit: Option<&str> = None;
        for (region, re) in &compiled {
            if re.is_match(name) {
                hit = Some(region.id.as_str());
                break;
            }
        }
        match hit {
            Some(id) => buckets.entry(id.to_string()).or_default().push(name.clone()),
            None => unmatched.push(name.clone()),
        }
    }

    let by_id: HashMap<&str, &RegionEntry> = compiled
        .iter()
        .map(|(r, _)| (r.id.as_str(), r))
        .collect();

    let mut available: HashMap<&str, &RegionEntry> = HashMap::new();
    let mut regions = Vec::new();
    let mut groups = Vec::new();
    let mut region_names = Vec::new();

    // 先按内置目录顺序输出，再输出有命中的补充国家
    let mut emit_order: Vec<&RegionEntry> = Vec::new();
    for r in catalog() {
        if buckets.contains_key(r.id) {
            if let Some(entry) = by_id.get(r.id) {
                emit_order.push(entry);
            }
        }
    }
    for (r, _) in &compiled {
        if builtin.contains(&r.id) {
            continue;
        }
        if buckets.contains_key(&r.id) {
            emit_order.push(r);
        }
    }

    for region in emit_order {
        let count = buckets.get(&region.id).map(|v| v.len()).unwrap_or(0);
        if count == 0 {
            continue;
        }
        available.insert(region.id.as_str(), region);
        regions.push(RegionStat {
            id: region.id.clone(),
            name: region.name.clone(),
            count,
        });
        region_names.push(region.name.clone());
        groups.push(url_test(
            &format!("g-{}", region.id),
            &region.name,
            &region.filter,
        ));
    }

    if !unmatched.is_empty() {
        regions.push(RegionStat {
            id: "other".into(),
            name: NAME_OTHER.into(),
            count: unmatched.len(),
        });
        let mut other = url_test("g-other", NAME_OTHER, "");
        other.proxies = unmatched.clone();
        region_names.push(NAME_OTHER.into());
        groups.push(other);
    }

    // 用户策略组（可增删）；保留配置中的名称与顺序
    for g in custom_groups.iter().filter(|g| is_user_strategy_group(g)) {
        let proxies = resolve_user_proxies(g, &available, &region_names);
        let mut out = g.clone();
        out.proxies = proxies;
        out.filter = String::new();
        groups.push(out);
    }

    let mut chain = pick(PREF_CHAIN, &available, &region_names);
    if chain.len() > 3 {
        chain.truncate(3);
    }
    groups.push(select("g-chain", NAME_CHAIN, chain));

    let mut default_proxies = region_names;
    default_proxies.push("DIRECT".into());
    groups.push(select("g-default", NAME_DEFAULT, default_proxies));

    Ok(ManagedPlan {
        groups,
        regions,
        unmatched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_names() {
        let names = vec![
            "香港 IEPL 01".into(),
            "HK-02".into(),
            "🇺🇸 美国 Gemini".into(),
            "Japan-Tokyo".into(),
            "专线加速".into(),
        ];
        let custom = vec![select(
            "g-ai",
            NAME_AI,
            vec!["🇺🇸 美国".into(), "🇯🇵 日本".into(), "🇭🇰 香港".into()],
        )];
        let plan = build_managed_groups(&names, &[], &custom).unwrap();
        let map: HashMap<_, _> = plan.regions.iter().map(|r| (r.id.as_str(), r.count)).collect();
        assert_eq!(map.get("hk"), Some(&2));
        assert_eq!(map.get("us"), Some(&1));
        assert_eq!(map.get("jp"), Some(&1));
        assert_eq!(map.get("other"), Some(&1));
        assert!(plan.groups.iter().any(|g| g.name == NAME_DEFAULT));
        assert!(plan.groups.iter().any(|g| g.name == "🇭🇰 香港"));
        assert!(!plan.groups.iter().any(|g| g.name == "🇹🇼 台湾"));
        let ai = plan.groups.iter().find(|g| g.id == "g-ai").unwrap();
        assert!(ai.proxies.iter().any(|p| p.contains("美国")));
        assert!(ai.proxies.iter().any(|p| p.contains("日本")));
        assert!(ai.proxies.iter().any(|p| p.contains("香港")));
    }

    #[test]
    fn extras_match_uncommon_country() {
        let extras = vec![RegionEntry {
            id: "no".into(),
            name: "🇳🇴 挪威".into(),
            filter: r"(?i)挪威|Norway|\bNO\d*\b|🇳🇴".into(),
        }];
        let names = vec!["挪威-01".into(), "专线".into()];
        let plan = build_managed_groups(&names, &extras, &[]).unwrap();
        let map: HashMap<_, _> = plan.regions.iter().map(|r| (r.id.as_str(), r.count)).collect();
        assert_eq!(map.get("no"), Some(&1));
        assert_eq!(map.get("other"), Some(&1));
    }

    #[test]
    fn custom_ai_overrides_auto() {
        let names = vec!["香港-1".into(), "美国-1".into(), "日本-1".into()];
        let custom = vec![select(
            "g-ai",
            NAME_AI,
            vec!["🇯🇵 日本".into(), "🇭🇰 香港".into()],
        )];
        let plan = build_managed_groups(&names, &[], &custom).unwrap();
        let ai = plan.groups.iter().find(|g| g.id == "g-ai").unwrap();
        assert_eq!(ai.proxies, vec!["🇯🇵 日本".to_string(), "🇭🇰 香港".to_string()]);
    }

    #[test]
    fn user_strategy_groups_add_and_omit_missing() {
        let names = vec!["香港-1".into()];
        let custom = vec![
            select("g-ai", NAME_AI, vec!["🇭🇰 香港".into(), "🇺🇸 美国".into()]),
            select("g-game", "🎮 游戏", vec!["🇭🇰 香港".into()]),
        ];
        let plan = build_managed_groups(&names, &[], &custom).unwrap();
        assert!(plan.groups.iter().any(|g| g.id == "g-game"));
        let ai = plan.groups.iter().find(|g| g.id == "g-ai").unwrap();
        // 美国本次无节点，被过滤
        assert_eq!(ai.proxies, vec!["🇭🇰 香港".to_string()]);
        assert!(!plan.groups.iter().any(|g| g.id == "g-binance"));
    }

    #[test]
    fn empty_regions_fallback_to_other() {
        let names = vec!["专线A".into(), "专线B".into()];
        let custom = vec![select(
            "g-ai",
            NAME_AI,
            vec!["🇺🇸 美国".into()],
        )];
        let plan = build_managed_groups(&names, &[], &custom).unwrap();
        assert_eq!(plan.regions.len(), 1);
        assert_eq!(plan.regions[0].id, "other");
        let ai = plan.groups.iter().find(|g| g.id == "g-ai").unwrap();
        assert_eq!(ai.proxies, vec![NAME_OTHER.to_string()]);
    }
}

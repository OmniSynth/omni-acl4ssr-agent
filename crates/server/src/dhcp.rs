//! OpenWrt DHCP 客户端列表（动态租约 + 静态绑定）。

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpClient {
    pub hostname: String,
    pub ip: String,
    pub mac: String,
    /// 来自 `/etc/config/dhcp` 的静态 host 绑定
    pub static_lease: bool,
}

#[derive(Debug, Deserialize)]
struct LuciDhcpLeases {
    #[serde(default)]
    dhcp_leases: Vec<LuciLease>,
}

#[derive(Debug, Deserialize)]
struct LuciLease {
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default)]
    macaddr: Option<String>,
    #[serde(default)]
    ipaddr: Option<String>,
}

/// 汇总当前局域网设备，供前端下拉选择。
pub async fn list_dhcp_clients() -> Vec<DhcpClient> {
    let mut by_ip: BTreeMap<String, DhcpClient> = BTreeMap::new();

    for c in load_static_hosts().await {
        by_ip.insert(c.ip.clone(), c);
    }

    for c in load_dynamic_leases().await {
        by_ip
            .entry(c.ip.clone())
            .and_modify(|old| {
                if old.hostname.is_empty() && !c.hostname.is_empty() {
                    old.hostname = c.hostname.clone();
                }
                if old.mac.is_empty() && !c.mac.is_empty() {
                    old.mac = c.mac.clone();
                }
            })
            .or_insert(c);
    }

    let mut out: Vec<DhcpClient> = by_ip.into_values().collect();
    out.sort_by(|a, b| {
        ip_sort_key(&a.ip)
            .cmp(&ip_sort_key(&b.ip))
            .then_with(|| a.hostname.to_ascii_lowercase().cmp(&b.hostname.to_ascii_lowercase()))
    });
    out
}

async fn load_dynamic_leases() -> Vec<DhcpClient> {
    if let Some(list) = load_via_ubus().await {
        return list;
    }
    load_dhcp_leases_file(Path::new("/tmp/dhcp.leases")).await
}

async fn load_via_ubus() -> Option<Vec<DhcpClient>> {
    let output = Command::new("ubus")
        .args(["call", "luci-rpc", "getDHCPLeases"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        debug!(status = ?output.status, "ubus luci-rpc getDHCPLeases 失败");
        return None;
    }
    let parsed: LuciDhcpLeases = serde_json::from_slice(&output.stdout).ok()?;
    let mut out = Vec::new();
    for lease in parsed.dhcp_leases {
        let ip = lease.ipaddr.unwrap_or_default().trim().to_string();
        if ip.is_empty() || !looks_like_ipv4(&ip) {
            continue;
        }
        let mac = normalize_mac(&lease.macaddr.unwrap_or_default());
        let hostname = normalize_hostname(&lease.hostname.unwrap_or_default());
        out.push(DhcpClient {
            hostname,
            ip,
            mac,
            static_lease: false,
        });
    }
    Some(out)
}

async fn load_dhcp_leases_file(path: &Path) -> Vec<DhcpClient> {
    let Ok(raw) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // expiry mac ip hostname clientid
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let mac = normalize_mac(parts[1]);
        let ip = parts[2].trim().to_string();
        if !looks_like_ipv4(&ip) {
            continue;
        }
        let hostname = normalize_hostname(parts[3]);
        out.push(DhcpClient {
            hostname,
            ip,
            mac,
            static_lease: false,
        });
    }
    out
}

async fn load_static_hosts() -> Vec<DhcpClient> {
    if let Some(list) = load_static_via_uci().await {
        return list;
    }
    load_static_from_config(Path::new("/etc/config/dhcp")).await
}

async fn load_static_via_uci() -> Option<Vec<DhcpClient>> {
    let output = Command::new("uci")
        .args(["-q", "show", "dhcp"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_uci_dhcp_hosts(&text)
}

fn parse_uci_dhcp_hosts(text: &str) -> Option<Vec<DhcpClient>> {
    // dhcp.@host[0]=host
    // dhcp.@host[0].name='cloudflare'
    // dhcp.@host[0].mac='BC:24:11:FF:D2:EB'
    // dhcp.@host[0].ip='172.16.1.2'
    let mut hosts: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
    let mut found = false;
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("dhcp.@host[") else {
            continue;
        };
        found = true;
        let Some((idx_s, prop)) = rest.split_once(']') else {
            continue;
        };
        let Ok(idx) = idx_s.parse::<u32>() else {
            continue;
        };
        let entry = hosts.entry(idx).or_default();
        if prop == "=host" {
            continue;
        }
        let Some(prop) = prop.strip_prefix('.') else {
            continue;
        };
        let Some((key, raw)) = prop.split_once('=') else {
            continue;
        };
        let val = unquote_uci(raw);
        match key {
            "name" => entry.0 = val,
            "mac" => entry.1 = normalize_mac(&val),
            "ip" => entry.2 = val,
            _ => {}
        }
    }
    if !found {
        return None;
    }
    let mut out = Vec::new();
    for (_, (name, mac, ip)) in hosts {
        if !looks_like_ipv4(&ip) {
            continue;
        }
        out.push(DhcpClient {
            hostname: normalize_hostname(&name),
            ip,
            mac,
            static_lease: true,
        });
    }
    Some(out)
}

async fn load_static_from_config(path: &Path) -> Vec<DhcpClient> {
    let Ok(raw) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_host = false;
    let mut name = String::new();
    let mut mac = String::new();
    let mut ip = String::new();

    let flush = |out: &mut Vec<DhcpClient>, name: &mut String, mac: &mut String, ip: &mut String| {
        if looks_like_ipv4(ip) {
            out.push(DhcpClient {
                hostname: normalize_hostname(name),
                ip: ip.clone(),
                mac: normalize_mac(mac),
                static_lease: true,
            });
        }
        name.clear();
        mac.clear();
        ip.clear();
    };

    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("config ") {
            if in_host {
                flush(&mut out, &mut name, &mut mac, &mut ip);
            }
            in_host = t == "config host" || t.starts_with("config host ");
            continue;
        }
        if !in_host {
            continue;
        }
        if let Some(v) = option_value(t, "name") {
            name = v;
        } else if let Some(v) = option_value(t, "mac") {
            mac = v;
        } else if let Some(v) = option_value(t, "ip") {
            ip = v;
        }
    }
    if in_host {
        flush(&mut out, &mut name, &mut mac, &mut ip);
    }
    out
}

fn option_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("option {key} ");
    let rest = line.strip_prefix(&prefix)?;
    Some(unquote_uci(rest))
}

fn unquote_uci(raw: &str) -> String {
    let s = raw.trim();
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        s[1..s.len().saturating_sub(1)].to_string()
    } else {
        s.to_string()
    }
}

fn normalize_hostname(raw: &str) -> String {
    let h = raw.trim();
    if h.is_empty() || h == "*" || h == "?" {
        String::new()
    } else {
        h.to_string()
    }
}

fn normalize_mac(raw: &str) -> String {
    let t = raw.trim().to_ascii_uppercase().replace('-', ":");
    if t.chars().filter(|c| *c == ':').count() == 5 {
        t
    } else if t.len() == 12 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        format!(
            "{}:{}:{}:{}:{}:{}",
            &t[0..2],
            &t[2..4],
            &t[4..6],
            &t[6..8],
            &t[8..10],
            &t[10..12]
        )
    } else {
        t
    }
}

fn looks_like_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|p| p.parse::<u8>().is_ok())
}

fn ip_sort_key(ip: &str) -> u32 {
    let mut parts = [0u8; 4];
    for (i, p) in ip.split('.').take(4).enumerate() {
        parts[i] = p.parse().unwrap_or(0);
    }
    u32::from_be_bytes(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uci_hosts() {
        let text = r#"
dhcp.@host[0]=host
dhcp.@host[0].name='cloudflare'
dhcp.@host[0].mac='BC:24:11:FF:D2:EB'
dhcp.@host[0].ip='172.16.1.2'
dhcp.@host[1]=host
dhcp.@host[1].mac='aa-bb-cc-dd-ee-ff'
dhcp.@host[1].ip='172.16.1.4'
"#;
        let list = parse_uci_dhcp_hosts(text).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].hostname, "cloudflare");
        assert_eq!(list[0].mac, "BC:24:11:FF:D2:EB");
        assert_eq!(list[1].mac, "AA:BB:CC:DD:EE:FF");
        assert!(list[1].hostname.is_empty());
    }
}

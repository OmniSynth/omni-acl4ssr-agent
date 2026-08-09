//! 局域网自签 HTTPS：浏览器要求麦克风等能力必须在 Secure Context 下。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use tracing::info;

pub fn ensure_dev_cert(data_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let dir = data_dir.join("tls");
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    if cert_path.is_file() && key_path.is_file() {
        return Ok((cert_path, key_path));
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建 {} 失败", dir.display()))?;

    // 覆盖常见本机/局域网访问名；IP 不匹配时浏览器仍会警告，用户信任后即为安全上下文
    let subject_alt_names = vec![
        "localhost".into(),
        "127.0.0.1".into(),
        "::1".into(),
        "omni-acl4ssr-agent.local".into(),
    ];
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(subject_alt_names).context("生成自签证书失败")?;
    std::fs::write(&cert_path, cert.pem()).context("写入 cert.pem 失败")?;
    std::fs::write(&key_path, signing_key.serialize_pem()).context("写入 key.pem 失败")?;
    info!(
        cert = %cert_path.display(),
        "已生成局域网 HTTPS 自签证书（麦克风等需 HTTPS）"
    );
    Ok((cert_path, key_path))
}

pub async fn serve_https(app: Router, addr: SocketAddr, cert: &Path, key: &Path) -> Result<()> {
    let config = RustlsConfig::from_pem_file(cert, key)
        .await
        .with_context(|| {
            format!(
                "加载 TLS 证书失败：{} / {}",
                cert.display(),
                key.display()
            )
        })?;
    info!(%addr, "HTTPS 监听（语音/麦克风请用此地址）");
    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
        .context("HTTPS 服务异常")?;
    Ok(())
}

/// 默认 TLS 口：HTTP 端口 +1（8787 → 8788）；可用 OMNI_TLS_LISTEN 覆盖，设为空字符串关闭。
pub fn resolve_tls_listen(http_listen: &str) -> Option<String> {
    match std::env::var("OMNI_TLS_LISTEN") {
        Ok(v) => {
            let t = v.trim();
            if t.is_empty() || t.eq_ignore_ascii_case("off") || t == "0" {
                None
            } else {
                Some(t.to_string())
            }
        }
        Err(_) => {
            // 未设置时：由 HTTP 地址推导
            if let Ok(addr) = http_listen.parse::<SocketAddr>() {
                let port = addr.port().saturating_add(1);
                Some(SocketAddr::new(addr.ip(), port).to_string())
            } else if let Some((host, port_s)) = http_listen.rsplit_once(':') {
                if let Ok(port) = port_s.parse::<u16>() {
                    Some(format!("{host}:{}", port.saturating_add(1)))
                } else {
                    Some("0.0.0.0:8788".into())
                }
            } else {
                Some("0.0.0.0:8788".into())
            }
        }
    }
}

mod ai;
mod ai_agent;
mod ai_chats;
mod api;
mod countries;
mod dhcp;
mod engine;
mod model;
mod nikki;
mod regions;
mod store;
mod tls;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{header, HeaderValue, Request};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use crate::ai::AiStore;
use crate::ai_chats::ChatStore;
use crate::api::AppState;
use crate::countries::WorldCatalog;
use crate::engine::YamlCache;
use crate::store::Store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // axum-server / rustls 0.23：需显式安装加密后端，否则 HTTPS 线程会 panic
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "omni_acl4ssr_agent=info,tower_http=info".into()),
        )
        .init();

    let data_dir = std::env::var("OMNI_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"));
    let listen = std::env::var("OMNI_LISTEN").unwrap_or_else(|_| "0.0.0.0:8787".into());
    let web_dir = std::env::var("OMNI_WEB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("web/dist"));

    let store = Store::open(&data_dir).await?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;
    let world = Arc::new(WorldCatalog::open(&data_dir, http.clone()).await);
    world.spawn_background_refresh();
    let ai = AiStore::open(&data_dir).await;
    let chats = ChatStore::open(&data_dir).await;

    let state = Arc::new(AppState {
        store,
        http,
        cache: YamlCache::default(),
        world,
        ai,
        chats,
    });

    let api = Router::new()
        .route("/health", get(api::health))
        .route("/config", get(api::get_config).put(api::put_config))
        .route("/profile", get(api::get_profile).put(api::put_profile))
        .route("/groups", get(api::get_groups).put(api::put_groups))
        .route(
            "/groups-mode",
            get(api::get_groups_mode).put(api::put_groups_mode),
        )
        .route("/rulesets", get(api::get_rulesets).put(api::put_rulesets))
        .route("/landings", get(api::get_landings).put(api::put_landings))
        .route("/lan-routes", get(api::get_lan_routes).put(api::put_lan_routes))
        .route("/dhcp-clients", get(api::get_dhcp_clients))
        .route(
            "/nikki/subscriptions",
            get(api::get_nikki_subscriptions),
        )
        .route("/nikki/panel", get(api::get_nikki_panel))
        .route(
            "/nikki/update-subscription",
            post(api::post_nikki_update_subscription),
        )
        .route("/convert", post(api::convert))
        .route(
            "/ai/settings",
            get(api::get_ai_settings).put(api::put_ai_settings),
        )
        .route("/ai/models", get(api::get_ai_models))
        .route("/ai/usage", get(api::get_ai_usage))
        .route("/ai/plan", post(api::ai_plan))
        .route("/ai/transcribe", post(api::ai_transcribe))
        .route("/ai/apply", post(api::ai_apply))
        .route("/ai/chats", get(api::list_ai_chats).post(api::create_ai_chat))
        .route(
            "/ai/chats/{id}",
            get(api::get_ai_chat)
                .patch(api::patch_ai_chat)
                .delete(api::delete_ai_chat),
        )
        .route("/ai/chats/{id}/branch", post(api::branch_ai_chat));

    let app = Router::new()
        .nest("/api", api)
        .route("/sub", get(api::subscription))
        .route("/sub/{id}", get(api::subscription))
        .with_state(state);

    let app = if web_dir.join("index.html").exists() {
        info!(path = %web_dir.display(), "托管前端静态资源");
        let index = web_dir.join("index.html");
        let static_files =
            ServeDir::new(web_dir).not_found_service(ServeFile::new(index));
        app.fallback_service(static_files)
            .layer(middleware::from_fn(static_cache_headers))
    } else {
        info!("未找到 web/dist，仅提供 API（开发时可另起 Vite）");
        app
    };

    let app = app
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = listen.parse()?;
    info!(%addr, data = %data_dir.display(), "omni-acl4ssr-agent 启动");

    // HTTPS：局域网 HTTP 非安全上下文，浏览器禁用麦克风；默认同机 HTTP 端口+1
    if let Some(tls_listen) = tls::resolve_tls_listen(&listen) {
        match tls_listen.parse::<SocketAddr>() {
            Ok(tls_addr) => match tls::ensure_dev_cert(&data_dir) {
                Ok((cert, key)) => {
                    let https_app = app.clone();
                    tokio::spawn(async move {
                        if let Err(e) = tls::serve_https(https_app, tls_addr, &cert, &key).await {
                            warn!("HTTPS 服务退出: {e:#}");
                        }
                    });
                    info!(
                        https = %tls_addr,
                        "语音/麦克风请用 HTTPS（首次需在浏览器信任自签证书）"
                    );
                }
                Err(e) => warn!("无法准备 TLS 证书，跳过 HTTPS: {e:#}"),
            },
            Err(e) => warn!(tls_listen = %tls_listen, "OMNI_TLS_LISTEN 无效，跳过 HTTPS: {e}"),
        }
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// index.html 禁止长期缓存（LuCI iframe 否则会一直显示旧前端）；带 hash 的 /assets/* 可长期缓存。
async fn static_cache_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let mut res = next.run(req).await;
    if path == "/" || path.ends_with(".html") {
        res.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
    } else if path.starts_with("/assets/") {
        res.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    res
}

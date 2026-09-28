//! Transport selection and startup for the MCP server.
//!
//! `MODE` picks the transport (`stdio`, `http`, or `both`); `ENABLE_HTTP_SERVER=true`
//! keeps stdio and adds the HTTP listener. The HTTP listener speaks the Streamable
//! HTTP transport statelessly (SEP-2567): no session is created, every request
//! carries its own protocol metadata, and the MCP endpoint lives at `/mcp`.

use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use rmcp::{
    ServiceExt,
    transport::{
        io::stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
        },
    },
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use url::{Host, Url};

use crate::tools::WebSearchHandler;

/// Loopback-only bind address for the HTTP transport.
const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:8000";
/// Path the MCP endpoint is mounted at.
const MCP_HTTP_PATH: &str = "/mcp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Stdio,
    Http,
    Both,
}

impl Mode {
    pub fn from_env() -> Result<Self> {
        let mode = std::env::var("MODE")
            .unwrap_or_else(|_| "stdio".into())
            .to_lowercase();

        let http_enabled =
            std::env::var("ENABLE_HTTP_SERVER").is_ok_and(|v| v.eq_ignore_ascii_case("true"));

        match mode.as_str() {
            "stdio" if http_enabled => Ok(Mode::Both),
            "stdio" => Ok(Mode::Stdio),
            "http" => Ok(Mode::Http),
            "both" => Ok(Mode::Both),
            other => anyhow::bail!("Unknown MODE `{other}`: expected `stdio`, `http`, or `both`"),
        }
    }
}

pub async fn serve(mode: Mode) -> Result<()> {
    match mode {
        Mode::Stdio => serve_stdio().await,
        Mode::Http => serve_http().await,
        Mode::Both => tokio::try_join!(serve_stdio(), serve_http()).map(|_| ()),
    }
}

async fn serve_stdio() -> Result<()> {
    tracing::info!("Starting STDIO transport...");
    let service = WebSearchHandler::new()
        .serve(stdio())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start stdio transport: {}", e))?;
    service.waiting().await?;
    Ok(())
}

async fn serve_http() -> Result<()> {
    let addr = std::env::var("HTTP_ADDR").unwrap_or_else(|_| DEFAULT_HTTP_ADDR.into());
    let allowed_origins = env_list("HTTP_ALLOWED_ORIGINS");

    // Stateless (SEP-2567): the transport keeps no per-client session state, so
    // each request is served by a fresh handler and must carry its own protocol
    // metadata.
    let mut config = StreamableHttpServerConfig::default().with_legacy_session_mode(false);
    if let Some(hosts) = env_list("HTTP_ALLOWED_HOSTS") {
        config = config.with_allowed_hosts(hosts);
    }
    if let Some(origins) = &allowed_origins {
        config = config.with_allowed_origins(origins.clone());
    }

    let service = StreamableHttpService::new(
        || Ok(WebSearchHandler::new()),
        Arc::new(NeverSessionManager::default()),
        config,
    );

    let router = Router::new()
        .nest_service(MCP_HTTP_PATH, service)
        .layer(cors_layer(allowed_origins.as_deref()));

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind HTTP transport to {addr}: {e}"))?;
    tracing::info!("Starting Streamable HTTP transport on http://{addr}{MCP_HTTP_PATH}");

    axum::serve(listener, router).await?;
    Ok(())
}

/// CORS policy for browser-hosted MCP clients.
///
/// With `HTTP_ALLOWED_ORIGINS` unset only loopback origins are allowed, so an
/// arbitrary web page cannot drive the local server. Setting the variable
/// replaces that with an explicit allow-list; the same list also drives the
/// transport's own `Origin` validation.
fn cors_layer(allowed_origins: Option<&[String]>) -> CorsLayer {
    let layer = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any);

    match allowed_origins {
        Some(origins) => layer.allow_origin(AllowOrigin::list(origins.iter().filter_map(|o| {
            o.parse().ok().or_else(|| {
                tracing::warn!("Ignoring unparsable origin in HTTP_ALLOWED_ORIGINS: {o}");
                None
            })
        }))),
        None => layer.allow_origin(AllowOrigin::predicate(|origin, _| {
            origin.to_str().is_ok_and(is_loopback_origin)
        })),
    }
}

fn is_loopback_origin(origin: &str) -> bool {
    Url::parse(origin)
        .ok()
        .and_then(|url| {
            url.host().map(|host| match host {
                Host::Domain(domain) => domain == "localhost",
                Host::Ipv4(ip) => ip.is_loopback(),
                Host::Ipv6(ip) => ip.is_loopback(),
            })
        })
        .is_some_and(|is_loopback| is_loopback)
}

/// Parse a comma-separated env var into a list; `None` when unset or empty.
fn env_list(name: &str) -> Option<Vec<String>> {
    let raw = std::env::var(name).ok()?;
    let items: Vec<String> = raw
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    (!items.is_empty()).then_some(items)
}

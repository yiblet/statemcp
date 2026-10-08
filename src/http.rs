//! Streamable HTTP via the official MCP SDK, with optional bearer authentication.
use crate::{Controller, Server, protocol::MAX_FRAME_BYTES};
use anyhow::{Result, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::State as AxumState,
    http::{HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::Serialize;
use state_http::BlockingExecutor;
use std::{net::SocketAddr, str::FromStr};
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct HttpOptions {
    authorization: Option<HeaderValue>,
    origins: Vec<String>,
    hosts: Vec<String>,
    prefix: HttpPrefix,
}

/// A normalized path prefix, excluding route syntax and ambiguous path segments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpPrefix(String);

impl FromStr for HttpPrefix {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        if value.is_empty() || value == "/" {
            return Ok(Self::default());
        }
        let prefix = value.strip_suffix('/').unwrap_or(value);
        if !prefix.starts_with('/')
            || prefix[1..].split('/').any(|segment| {
                segment.is_empty()
                    || matches!(segment, "." | "..")
                    || !segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
            })
        {
            bail!("HTTP prefix must be an absolute path with nonempty literal segments");
        }
        Ok(Self(prefix.to_owned()))
    }
}

impl HttpPrefix {
    pub fn mcp_path(&self) -> String {
        format!("{}/mcp", self.0)
    }
}

/// Tokens use the RFC6750 bearer alphabet. Never print a rejected credential.
pub fn validate_bearer(token: Option<&str>) -> Result<()> {
    if let Some(token) = token {
        let value = token.trim_end_matches('=');
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/".contains(&byte))
        {
            bail!("--auth-bearer requires a nonempty bearer token without whitespace");
        }
    }
    Ok(())
}
impl HttpOptions {
    /// Use the actual listening address, including its assigned port when binding port 0.
    pub fn new(address: SocketAddr, bearer: Option<String>) -> Result<Self> {
        Self::for_addresses(&[address], bearer)
    }

    pub fn for_addresses(addresses: &[SocketAddr], bearer: Option<String>) -> Result<Self> {
        if addresses.is_empty() {
            bail!("HTTP requires at least one listening address");
        }
        validate_bearer(bearer.as_deref())?;
        let authorization = bearer
            .map(|token| HeaderValue::from_str(&format!("Bearer {token}")))
            .transpose()
            .map_err(|_| anyhow::anyhow!("invalid bearer credential"))?;
        let mut hosts = Vec::new();
        for address in addresses {
            hosts.push(address.to_string());
            if address.ip().is_loopback() {
                hosts.push(format!("localhost:{}", address.port()));
            }
        }
        hosts.sort_unstable();
        hosts.dedup();
        let origins = hosts.iter().map(|host| format!("http://{host}")).collect();
        Ok(Self {
            authorization,
            origins,
            hosts,
            prefix: HttpPrefix::default(),
        })
    }

    pub fn with_prefix(mut self, prefix: HttpPrefix) -> Self {
        self.prefix = prefix;
        self
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

pub fn router(state: Controller, options: HttpOptions) -> Router {
    let executor = BlockingExecutor::new(state);
    let hosting = state_http::Hosting::new(&options.hosts, &options.prefix.0);
    let server = Server::new(executor.clone()).with_hosting(hosting);
    let namespace = state_http::namespace_router(executor, &options.prefix.0)
        .layer(middleware::from_fn_with_state(options.clone(), authorize));
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_max_request_body_bytes(MAX_FRAME_BYTES)
            .with_allowed_hosts(options.hosts.clone())
            .with_allowed_origins(options.origins.clone()),
    );
    let prefix = options.prefix.clone();
    let mcp = Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(options.clone(), authorize));
    let routes = Router::new()
        .route(
            "/health",
            get(|| async { Json(HealthResponse { status: "ok" }) }),
        )
        .merge(mcp)
        .fallback_service(namespace)
        .layer(middleware::from_fn_with_state(
            options,
            check_host_and_origin,
        ));
    let routes = if prefix.0.is_empty() {
        routes
    } else {
        Router::new().nest(&prefix.0, routes)
    };
    // Preserve legacy reserved route spelling without changing namespace URL semantics.
    Router::new()
        .fallback_service(routes)
        .layer(middleware::from_fn_with_state(prefix, normalize_reserved))
}

async fn normalize_reserved(
    AxumState(prefix): AxumState<HttpPrefix>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let trimmed = path.trim_end_matches('/');
    if path != trimmed
        && (trimmed == format!("{}/mcp", prefix.0) || trimmed == format!("{}/health", prefix.0))
    {
        let text = if let Some(query) = request.uri().query() {
            format!("{trimmed}?{query}")
        } else {
            trimmed.into()
        };
        match text.parse() {
            Ok(uri) => *request.uri_mut() = uri,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
    }
    next.run(request).await
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    state: Controller,
    options: HttpOptions,
) -> std::io::Result<()> {
    serve_listeners(vec![listener], state, options).await
}

/// Share a single router, session manager, and state across all listeners.
pub async fn serve_listeners(
    listeners: Vec<tokio::net::TcpListener>,
    state: Controller,
    options: HttpOptions,
) -> std::io::Result<()> {
    if listeners.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "HTTP requires at least one listener",
        ));
    }
    let app = router(state, options);
    let mut servers = tokio::task::JoinSet::new();
    for listener in listeners {
        let app = app.clone();
        servers.spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
        });
    }
    while let Some(result) = servers.join_next().await {
        result.map_err(std::io::Error::other)??;
    }
    Ok(())
}

async fn authorize(
    AxumState(options): AxumState<HttpOptions>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(expected) = &options.authorization {
        let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
        let matches = values.next().is_some_and(|value| {
            let bytes = value.as_bytes();
            // Authentication schemes are case insensitive; token bytes are exact.
            bytes
                .get(..7)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"Bearer "))
                && bool::from(bytes[7..].ct_eq(&expected.as_bytes()[7..]))
        }) && values.next().is_none();
        if !matches {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
            )
                .into_response();
        }
    }
    next.run(request).await
}

async fn check_host_and_origin(
    AxumState(options): AxumState<HttpOptions>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let host_allowed = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| {
            options
                .hosts
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(host))
        });
    let origin_allowed = request.headers().get(header::ORIGIN).is_none_or(|origin| {
        origin
            .to_str()
            .ok()
            .is_some_and(|origin| options.origins.iter().any(|allowed| allowed == origin))
    });
    if !host_allowed || !origin_allowed {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        let origin = state_http::RequestOrigin(format!("http://{host}"));
        request.extensions_mut().insert(origin);
    }
    next.run(request).await
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;

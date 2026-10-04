//! Streamable HTTP via the official MCP SDK, with optional bearer authentication.
use crate::{Server, State, protocol::MAX_FRAME_BYTES};
use anyhow::{Result, bail};
use axum::{
    Router,
    body::Body,
    extract::State as AxumState,
    http::{HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::net::SocketAddr;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct HttpOptions {
    authorization: Option<HeaderValue>,
    origins: Vec<String>,
    hosts: Vec<String>,
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
        validate_bearer(bearer.as_deref())?;
        let authorization = bearer
            .map(|token| HeaderValue::from_str(&format!("Bearer {token}")))
            .transpose()
            .map_err(|_| anyhow::anyhow!("invalid bearer credential"))?;
        let mut hosts = vec![address.to_string()];
        if address.ip().is_loopback() {
            hosts.push(format!("localhost:{}", address.port()));
        }
        let origins = hosts.iter().map(|host| format!("http://{host}")).collect();
        Ok(Self {
            authorization,
            origins,
            hosts,
        })
    }
}

pub fn router(state: State, options: HttpOptions) -> Router {
    let server = Server::new(state);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_max_request_body_bytes(MAX_FRAME_BYTES)
            .with_allowed_hosts(options.hosts.clone())
            .with_allowed_origins(options.origins.clone()),
    );
    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(options, authorize))
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    state: State,
    options: HttpOptions,
) -> std::io::Result<()> {
    axum::serve(listener, router(state, options))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
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
    next.run(request).await
}

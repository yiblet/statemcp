//! JSON-RPC over HTTP, sharing the stdio dispatcher and direct JSON results.
use crate::{
    Server, State,
    protocol::{self, MAX_FRAME_BYTES},
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State as AxumState,
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct HttpOptions {
    authorization: Option<HeaderValue>,
    origins: Vec<String>,
    hosts: Vec<String>,
}

/// Tokens use the RFC6750 bearer alphabet. Never print a rejected credential.
pub fn validate_bearer(token: Option<&str>) -> Result<(), String> {
    if let Some(token) = token {
        let value = token.trim_end_matches('=');
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/".contains(&byte))
        {
            return Err("--auth-bearer requires a nonempty bearer token without whitespace".into());
        }
    }
    Ok(())
}
impl HttpOptions {
    /// Use the actual listening address, including its assigned port when binding port 0.
    pub fn new(address: SocketAddr, bearer: Option<String>) -> Result<Self, String> {
        validate_bearer(bearer.as_deref())?;
        let authorization = bearer
            .map(|token| HeaderValue::from_str(&format!("Bearer {token}")))
            .transpose()
            .map_err(|_| "invalid bearer credential".to_owned())?;
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

type Session = Arc<Mutex<Server<State>>>;
#[derive(Clone)]
struct HttpService {
    state: State,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    slots: Arc<Semaphore>,
}

pub fn router(state: State, options: HttpOptions) -> Router {
    let service = HttpService {
        state,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        slots: Arc::new(Semaphore::new(16)),
    };
    Router::new()
        .route("/mcp", post(handle).delete(delete_session))
        .with_state(service)
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

async fn delete_session(
    AxumState(service): AxumState<HttpService>,
    headers: HeaderMap,
) -> StatusCode {
    let Some(id) = headers
        .get("mcp-session-id")
        .and_then(|id| id.to_str().ok())
    else {
        return StatusCode::BAD_REQUEST;
    };
    if service
        .sessions
        .lock()
        .expect("sessions")
        .remove(id)
        .is_some()
    {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn handle(AxumState(service): AxumState<HttpService>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    if !parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let bytes = match to_bytes(body, MAX_FRAME_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let message: Value = match serde_json::from_slice(&bytes) {
        Ok(message) => message,
        Err(_) => {
            return Json(protocol::rpc_error(Value::Null, -32700, "Parse error")).into_response();
        }
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let session_id = parts
        .headers
        .get("mcp-session-id")
        .and_then(|id| id.to_str().ok());
    let session = match session_id {
        Some(id) => match service.sessions.lock().expect("sessions").get(id).cloned() {
            Some(session) => session,
            None => return StatusCode::NOT_FOUND.into_response(),
        },
        None if message["method"] == "initialize" => {
            let mut server = Server::new(service.state.clone());
            let response = server.handle(message);
            let Some(response) = response else {
                return StatusCode::ACCEPTED.into_response();
            };
            if response.get("error").is_some() {
                return Json(response).into_response();
            }
            let mut sessions = service.sessions.lock().expect("sessions");
            if sessions.len() >= 256 {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let id = uuid::Uuid::new_v4().to_string();
            sessions.insert(id.clone(), Arc::new(Mutex::new(server)));
            return ([("mcp-session-id", id)], Json(response)).into_response();
        }
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    if message["method"] != "tools/call" || message.get("id").is_none() {
        return match session.lock().expect("session").handle(message) {
            Some(response) => Json(response).into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        };
    }
    // Clone only the lightweight session/dispatcher so a slow tool cannot block
    // discovery or other calls in the same HTTP session.
    let mut server = session.lock().expect("session").clone();
    let permit = match service.slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return Json(json!({"jsonrpc":"2.0","id":id,"result":protocol::tool_result(
            json!({"error":{"code":"BUSY","message":"Too many active invocations; retry later"}}), true)})).into_response(),
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        server.handle(message)
    })
    .await
    {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(_) => Json(protocol::rpc_error(id, -32603, "Invocation worker failed")).into_response(),
    }
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
    if request
        .headers()
        .get("mcp-protocol-version")
        .is_some_and(|version| version != crate::protocol::PROTOCOL_VERSION)
    {
        return StatusCode::BAD_REQUEST.into_response();
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

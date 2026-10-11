//! Hosting location metadata and the bounded namespace HTTP adapter.
use crate::{BlockingError, BlockingExecutor};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{OriginalUri, State},
    http::{HeaderName, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use state_contract::http::{ApiPublished, HttpRequest, HttpResponse, Method, decode_path};
use state_core::{Error, ErrorCode};
use std::collections::BTreeMap;

/// Validated request origin inserted by the server's host policy middleware.
#[derive(Clone, Debug)]
pub struct RequestOrigin(pub String);
/// Actual listener origins and prefix used to report hosted endpoint locations.
#[derive(Clone, Debug)]
pub struct Hosting {
    bases: Vec<String>,
    prefix: String,
}
impl Hosting {
    /// Configure already validated listener authorities and a literal server prefix.
    pub fn new(authorities: &[String], prefix: &str) -> Self {
        Self {
            bases: authorities.iter().map(|h| format!("http://{h}")).collect(),
            prefix: prefix.into(),
        }
    }
    /// Configure URL reporting from an explicit public origin and route prefix.
    pub fn from_config(config: state_contract::execution::PublicConfig) -> Self {
        Self {
            bases: vec![config.public_url.unwrap_or_default()],
            prefix: config.prefix,
        }
    }
    /// Populate route URL patterns using the configured public location.
    pub fn locate(&self, route: &mut ApiPublished, _origin: Option<&RequestOrigin>) {
        let bases = self.bases.iter().map(String::as_str);
        let path = if matches!(
            route.target,
            state_contract::http::TargetInput::Static { .. }
        ) {
            route
                .path
                .as_str()
                .split('{')
                .next()
                .unwrap_or(route.path.as_str())
        } else {
            route.path.as_str()
        };
        let namespace = if matches!(
            route.namespace.as_str(),
            "." | ".." | "mcp" | "health" | "upload" | "download"
        ) {
            route.namespace_id.as_str()
        } else {
            route.namespace.as_str()
        };
        let namespace = encode(namespace);
        let path = path
            .split('/')
            .map(|part| {
                if part.starts_with('{') && part.ends_with('}') {
                    part.into()
                } else {
                    encode(part)
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        route.urls = bases
            .into_iter()
            .map(|base| {
                format!(
                    "{base}{}/{namespace}{path}",
                    self.prefix,
                    namespace = namespace
                )
            })
            .collect();
    }
}
#[derive(Clone)]
struct Adapter {
    executor: BlockingExecutor,
    prefix: String,
}
/// Build a catch-all namespace router; caller applies authentication/authority policy.
pub fn namespace_router(executor: BlockingExecutor, prefix: &str) -> Router {
    Router::new().fallback(any(handle)).with_state(Adapter {
        executor,
        prefix: prefix.into(),
    })
}
async fn handle(
    State(adapter): State<Adapter>,
    OriginalUri(uri): OriginalUri,
    request: Request<Body>,
) -> Response {
    let original_path = uri.path();
    let full_path = match decode_path(original_path) {
        Ok(path) => path,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let Some(path) = full_path.strip_prefix(&adapter.prefix) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if uri.query().is_some_and(|query| query.len() > 16 * 1024) {
        return StatusCode::URI_TOO_LONG.into_response();
    }
    let Some((namespace, tail)) = path
        .trim_start_matches('/')
        .split_once('/')
        .map(|(n, p)| (n.to_owned(), format!("/{p}")))
        .or_else(|| {
            (!path.trim_matches('/').is_empty())
                .then(|| (path.trim_matches('/').into(), "/".into()))
        })
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let method = match Method::parse(request.method().as_str()) {
        Ok(m) => m,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let head = method.as_str() == "HEAD";
    let mut headers = Vec::new();
    let mut header_bytes = 0usize;
    for (k, v) in request.headers() {
        header_bytes = header_bytes.saturating_add(k.as_str().len() + v.as_bytes().len());
        if header_bytes > 64 * 1024 {
            return StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE.into_response();
        }
        let value = match v.to_str() {
            Ok(v) => v.to_owned(),
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        headers.push((k.as_str().into(), value));
    }
    let host = match request.headers().get("host").and_then(|h| h.to_str().ok()) {
        Some(host) => host.to_owned(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let body = match to_bytes(request.into_body(), 1024 * 1024).await {
        Ok(bytes) => bytes.to_vec(),
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let request = HttpRequest {
        method,
        path: full_path,
        query: uri.query().unwrap_or("").into(),
        scheme: "http".into(),
        host,
        headers,
        body,
        view_args: BTreeMap::new(),
    };
    match adapter
        .executor
        .run(move |controller| controller.handle_http(&namespace, &tail, request))
        .await
    {
        Ok(response) => write_response(response, head),
        Err(BlockingError::Application(error)) => failure(error),
        Err(BlockingError::Task(error)) => {
            tracing::error!(%error,"namespace HTTP task failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
fn failure(error: Error) -> Response {
    let status = match error.code {
        ErrorCode::HttpBadRequest | ErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
        ErrorCode::HttpUnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Busy | ErrorCode::Conflict => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::LimitExceeded => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    tracing::debug!(code=%error.code,%status,"namespace HTTP operation failed");
    let mut response = (status, error.code.as_str()).into_response();
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("1"));
    }
    response
}
fn write_response(response: HttpResponse, head: bool) -> Response {
    let (status, headers, body) = response.into_parts();
    let length = body.len();
    let mut response = Response::new(if head {
        Body::empty()
    } else {
        Body::from(body)
    });
    *response.status_mut() = match StatusCode::from_u16(status) {
        Ok(s) => s,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    for (name, value) in headers {
        match (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            (Ok(name), Ok(value)) => {
                response.headers_mut().append(name, value);
            }
            _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
    if status != 204 && status != 304 && !response.headers().contains_key("content-length") {
        match HeaderValue::from_str(&length.to_string()) {
            Ok(value) => {
                response.headers_mut().insert("content-length", value);
            }
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
    response
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

mod common;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use statemcp::{
    State,
    http::{HttpOptions, router},
};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("statemcp-http-{}-{suffix}", std::process::id())))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn app(state: State, token: Option<&str>) -> Router {
    router(
        state,
        HttpOptions::new("127.0.0.1:8000".parse().unwrap(), token.map(str::to_owned)).unwrap(),
    )
}
fn request(message: Value, session: Option<&str>, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "127.0.0.1:8000")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", "2025-06-18");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(message.to_string())).unwrap()
}
async fn response_json(response: axum::response::Response) -> Value {
    let bytes = tokio::time::timeout(
        Duration::from_secs(3),
        to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
            .find(|message| message.get("id").is_some())
            .expect("JSON-RPC response in SSE stream")
    })
}
async fn initialize(app: &Router, token: Option<&str>) -> String {
    let response = app.clone().oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}), None, token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let reply = response_json(response).await;
    assert_eq!(reply["result"]["serverInfo"]["name"], "statemcp");
    assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
    let notification = app
        .clone()
        .oneshot(request(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            Some(&session),
            token,
        ))
        .await
        .unwrap();
    assert_eq!(notification.status(), StatusCode::ACCEPTED);
    session
}
async fn tool(app: &Router, session: &str, name: &str, args: Value, token: Option<&str>) -> Value {
    static REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100);
    let id = REQUEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let response = app.clone().oneshot(request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}), Some(session), token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await["result"].clone()
}

#[tokio::test]
async fn authenticated_http_sessions_execute_and_share_persistent_state() {
    let directory = Directory::new();
    let app = app(State::open(&directory.0).unwrap(), Some("secret-token"));
    let session = initialize(&app, Some("secret-token")).await;
    let listed = app
        .clone()
        .oneshot(request(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            Some(&session),
            Some("secret-token"),
        ))
        .await
        .unwrap();
    assert_eq!(
        response_json(listed).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        30
    );
    let created = tool(&app, &session, "execute", json!({"script":"mcp('state_namespace', {'action': 'create', 'name': 'http'})\nmcp('state_fs', {'action': 'write', 'namespace': 'http', 'path': '/saved', 'text': 'persisted'})\n42"}), Some("secret-token")).await;
    assert_eq!(created["isError"], false);
    assert_eq!(tool_value(&created)["value"], 42);
    let other_session = initialize(&app, Some("secret-token")).await;
    assert_ne!(session, other_session);
    let saved = tool(
        &app,
        &other_session,
        "fs.read",
        json!({"namespace":"http","path":"/saved"}),
        Some("secret-token"),
    )
    .await;
    assert_eq!(tool_value(&saved)["text"], "persisted");
    let failed = tool(
        &app,
        &session,
        "execute",
        json!({"namespace":"http","script":"write_text('/rolled-back', 'no')\n1 / 0"}),
        Some("secret-token"),
    )
    .await;
    assert_eq!(failed["isError"], true);
    let missing = tool(
        &app,
        &session,
        "fs.read",
        json!({"namespace":"http","path":"/rolled-back"}),
        Some("secret-token"),
    )
    .await;
    assert_eq!(missing["isError"], true);
    let delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/mcp")
                .header("host", "127.0.0.1:8000")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", "2025-06-18")
                .header("authorization", "Bearer secret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(delete.status().is_success());
    let expired = app
        .clone()
        .oneshot(request(
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
            Some(&session),
            Some("secret-token"),
        ))
        .await
        .unwrap();
    assert_eq!(expired.status(), StatusCode::NOT_FOUND);
    drop(app);
    let reopened = State::open(&directory.0).unwrap();
    assert_eq!(
        reopened
            .dispatch("fs.read", json!({"namespace":"http","path":"/saved"}))
            .unwrap()["text"],
        "persisted"
    );
}

#[tokio::test]
async fn bearer_auth_covers_every_http_method_and_rejects_ambiguous_credentials() {
    let directory = Directory::new();
    let app = app(State::open(&directory.0).unwrap(), Some("secret-token"));
    for method in ["GET", "POST", "DELETE", "OPTIONS"] {
        for token in [None, Some("wrong-token")] {
            let mut request = request(json!({}), None, token);
            *request.method_mut() = method.parse().unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()["www-authenticate"], "Bearer");
        }
    }
    let mut duplicate = request(json!({}), None, Some("secret-token"));
    duplicate
        .headers_mut()
        .append("authorization", "Bearer secret-token".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(duplicate).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut evil_origin = request(json!({}), None, Some("secret-token"));
    evil_origin
        .headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(evil_origin).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut bad_host = request(json!({}), None, Some("secret-token"));
    bad_host
        .headers_mut()
        .insert("host", "evil.example".parse().unwrap());
    assert_eq!(
        app.oneshot(bad_host).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn http_without_bearer_is_optional_and_protocol_headers_are_checked() {
    let directory = Directory::new();
    let app = app(State::open(&directory.0).unwrap(), None);
    let session = initialize(&app, None).await;
    let mut unsupported = request(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
        Some(&session),
        None,
    );
    unsupported
        .headers_mut()
        .insert("mcp-protocol-version", "unsupported".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(unsupported).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let response = app
        .oneshot(request(
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
            None,
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    assert!(HttpOptions::new("127.0.0.1:8000".parse().unwrap(), Some(String::new())).is_err());
    assert!(HttpOptions::new("127.0.0.1:8000".parse().unwrap(), Some("two words".into())).is_err());
}

struct SlowBackend;
impl statemcp::RuntimeBackend for SlowBackend {
    fn execute(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: Value,
        _: &statemcp::Limits,
        _: &mut state_runtime::HostCallback<'_>,
    ) -> Result<state_runtime::RunResult, state_runtime::RuntimeError> {
        std::thread::sleep(Duration::from_millis(250));
        Ok(state_runtime::RunResult {
            value: json!(42),
            stdout: String::new(),
        })
    }
    fn invoke(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: Value,
        _: &statemcp::Limits,
        _: &mut state_runtime::HostCallback<'_>,
    ) -> Result<state_runtime::RunResult, state_runtime::RuntimeError> {
        unreachable!()
    }
    fn validate_module(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: &statemcp::Limits,
    ) -> Result<(), state_runtime::RuntimeError> {
        unreachable!()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn synchronous_invocation_does_not_block_async_http_requests() {
    let directory = Directory::new();
    let state = State::with_backend(
        statemcp::Store::open(&directory.0).unwrap(),
        std::sync::Arc::new(SlowBackend),
    );
    let app = app(state, None);
    let session = initialize(&app, None).await;
    let worker_app = app.clone();
    let worker_session = session.clone();
    let start = std::time::Instant::now();
    let slow = tokio::spawn(async move {
        tool(
            &worker_app,
            &worker_session,
            "execute",
            json!({"script":"slow"}),
            None,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let response = app
        .oneshot(request(
            json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
            Some(&session),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        response_json(response).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        30
    );
    assert!(
        start.elapsed() < Duration::from_millis(200),
        "blocking transaction stalled async networking"
    );
    assert_eq!(tool_value(&slow.await.unwrap())["value"], 42);
}

fn tool_value(result: &Value) -> Value {
    assert!(result.get("structuredContent").is_none());
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn json_transport_preserves_lifecycle_and_request_limits() {
    let directory = Directory::new();
    let app = app(State::open(&directory.0).unwrap(), None);
    let invalid = app
        .clone()
        .oneshot(request(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            None,
            None,
        ))
        .await
        .unwrap();
    assert!(invalid.headers().get("mcp-session-id").is_none());
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let session = initialize(&app, None).await;
    let notification = app.clone().oneshot(request(json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"namespace.create","arguments":{"name":"must-not-exist"}}}), Some(&session), None)).await.unwrap();
    assert_eq!(notification.status(), StatusCode::ACCEPTED);
    let namespaces = tool(&app, &session, "namespace.list", json!({}), None).await;
    assert_eq!(tool_value(&namespaces)["namespaces"], json!([]));
    assert!(namespaces.get("structuredContent").is_none());
    let mut large = request(json!({}), Some(&session), None);
    *large.body_mut() = Body::from(vec![b'x'; statemcp::protocol::MAX_FRAME_BYTES + 1]);
    assert_eq!(
        app.clone().oneshot(large).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut malformed = request(json!({}), Some(&session), None);
    *malformed.body_mut() = Body::from("not json");
    assert_eq!(
        app.clone().oneshot(malformed).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let mut wrong_type = request(json!({}), Some(&session), None);
    wrong_type
        .headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert_eq!(
        app.oneshot(wrong_type).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn official_http_clients_share_state_across_protocol_versions() {
    use rmcp::{
        ServiceExt,
        model::ClientConfig,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let directory = Directory::new();
    let state = State::open(&directory.0).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let options = HttpOptions::new(address, Some("sdk-token".into())).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state, options)).await.unwrap();
    });
    let transport = || {
        StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/mcp"))
                .auth_header("sdk-token"),
        )
    };
    let first = common::config().serve(transport()).await.unwrap();
    let second = ClientConfig::default().serve(transport()).await.unwrap();
    assert_eq!(first.list_all_tools().await.unwrap().len(), 30);
    assert_eq!(second.list_all_tools().await.unwrap().len(), 30);
    common::tool(&first, "namespace.create", json!({"name":"shared"})).await;
    common::tool(
        &first,
        "db.create",
        json!({"namespace":"shared","database":"app"}),
    )
    .await;
    common::tool(
        &first,
        "db.execute",
        json!({"namespace":"shared","database":"app","sql":"CREATE TABLE notes(text TEXT)"}),
    )
    .await;
    common::tool(&first, "fs.write", json!({"namespace":"shared","path":"/api.py","text":"def add(text):\n    return db_execute('app', 'INSERT INTO notes VALUES (?)', [text])"})).await;
    common::tool(
        &first,
        "function.declare",
        json!({"namespace":"shared","name":"add","file":"/api.py","symbol":"add"}),
    )
    .await;
    common::tool(
        &second,
        "call",
        json!({"namespace":"shared","function":"add","arguments":{"text":"from another session"}}),
    )
    .await;
    assert_eq!(
        common::tool(
            &first,
            "db.query",
            json!({"namespace":"shared","database":"app","sql":"SELECT text FROM notes"})
        )
        .await["rows"],
        json!([["from another session"]])
    );
    first.cancel().await.unwrap();
    second.cancel().await.unwrap();
    server.abort();
}

#[derive(Default)]
struct GatedBackend {
    entered: std::sync::atomic::AtomicUsize,
    started: tokio::sync::Notify,
    released: std::sync::Mutex<bool>,
    gate: std::sync::Condvar,
}
impl GatedBackend {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.gate.notify_all();
    }
}
impl statemcp::RuntimeBackend for GatedBackend {
    fn execute(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: Value,
        _: &statemcp::Limits,
        _: &mut state_runtime::HostCallback<'_>,
    ) -> Result<state_runtime::RunResult, state_runtime::RuntimeError> {
        self.entered
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.gate.wait(released).unwrap();
        }
        Ok(state_runtime::RunResult {
            value: json!(42),
            stdout: String::new(),
        })
    }
    fn invoke(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: Value,
        _: &statemcp::Limits,
        _: &mut state_runtime::HostCallback<'_>,
    ) -> Result<state_runtime::RunResult, state_runtime::RuntimeError> {
        unreachable!()
    }
    fn validate_module(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: &statemcp::Limits,
    ) -> Result<(), state_runtime::RuntimeError> {
        unreachable!()
    }
}

struct ReleaseGate(std::sync::Arc<GatedBackend>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn invocation_limit_is_shared_across_sessions_and_recovers() {
    let directory = Directory::new();
    let backend = std::sync::Arc::new(GatedBackend::default());
    let guard = ReleaseGate(backend.clone());
    let state = State::with_backend(
        statemcp::Store::open(&directory.0).unwrap(),
        backend.clone(),
    );
    let app = app(state, None);
    let session = initialize(&app, None).await;
    let other = initialize(&app, None).await;
    let mut calls = Vec::new();
    for _ in 0..16 {
        let app = app.clone();
        let session = session.clone();
        calls.push(tokio::spawn(async move {
            tool(&app, &session, "execute", json!({"script":"wait"}), None).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while backend.entered.load(std::sync::atomic::Ordering::SeqCst) < 16 {
            backend.started.notified().await;
        }
    })
    .await
    .expect("all blocking jobs started");
    let busy = tool(&app, &other, "execute", json!({"script":"wait"}), None).await;
    assert_eq!(busy["isError"], true);
    assert_eq!(tool_value(&busy)["error"]["code"], "BUSY");
    drop(guard);
    for call in calls {
        assert_eq!(tool_value(&call.await.unwrap())["value"], 42);
    }
    assert_eq!(
        tool_value(&tool(&app, &other, "execute", json!({"script":"next"}), None).await)["value"],
        42
    );
}

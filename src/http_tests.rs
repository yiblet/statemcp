use crate::test_support as common;
use crate::{
    Controller,
    http::{HttpOptions, router},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use common::Fixture;
use serde_json::Map;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

fn app(state: Controller, token: Option<&str>) -> Router {
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

#[test]
fn prefixes_reject_ambiguous_paths_before_serving() {
    use crate::http::HttpPrefix;
    for prefix in ["", "/", "/statemcp", "/statemcp/", "/api/v1"] {
        assert!(prefix.parse::<HttpPrefix>().is_ok(), "{prefix}");
    }
    for prefix in [
        "relative", "//", "/a//b", "/a/../b", "/.", "/a?b", "/{path}", "/a%2fb", "/a b",
    ] {
        assert!(prefix.parse::<HttpPrefix>().is_err(), "{prefix}");
    }
    assert!(HttpOptions::for_addresses(&[], None).is_err());
}

#[tokio::test]
async fn prefixed_routes_share_sessions_and_health_is_public() {
    let directory = Fixture::new();
    let addresses = [
        "127.0.0.1:8000".parse().unwrap(),
        "[::1]:8000".parse().unwrap(),
    ];
    let options = HttpOptions::for_addresses(&addresses, Some("secret-token".into()))
        .unwrap()
        .with_prefix("/statemcp/".parse().unwrap());
    let app = router(directory.state_async().await.unwrap(), options);
    for host in ["127.0.0.1:8000", "[::1]:8000", "localhost:8000"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/statemcp/health")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, json!({"status":"ok"}));
    }
    let mut missing_auth = request(json!({}), None, None);
    *missing_auth.uri_mut() = "/statemcp/mcp/".parse().unwrap();
    assert_eq!(
        app.clone().oneshot(missing_auth).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut init = request(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"prefix-test","version":"1"}}}),
        None,
        Some("secret-token"),
    );
    *init.uri_mut() = "/statemcp/mcp/".parse().unwrap();
    let response = app.clone().oneshot(init).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        response_json(response).await["result"]["serverInfo"]["name"],
        "statemcp"
    );
    // Use the same session through the other path spelling and IPv6 Host.
    let mut initialized = request(
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        Some(&session),
        Some("secret-token"),
    );
    *initialized.uri_mut() = "/statemcp/mcp".parse().unwrap();
    initialized
        .headers_mut()
        .insert("host", "[::1]:8000".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(initialized).await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    let mut listed = request(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        Some(&session),
        Some("secret-token"),
    );
    *listed.uri_mut() = "/statemcp/mcp/".parse().unwrap();
    assert_eq!(
        response_json(app.clone().oneshot(listed).await.unwrap()).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        34
    );
    for uri in ["/health", "/mcp", "/mcp/", "/statemcp/mcpx"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("host", "127.0.0.1:8000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if uri == "/statemcp/mcpx" {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::NOT_FOUND
            },
            "{uri}"
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/statemcp/health")
                .header("host", "evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn routers_for_multiple_addresses_share_state() {
    let fixture = Fixture::new();
    let addresses = [
        "127.0.0.1:8000".parse().unwrap(),
        "127.0.0.1:9000".parse().unwrap(),
    ];
    let options = HttpOptions::for_addresses(&addresses, Some("sdk-token".into())).unwrap();
    let state = fixture.state_async().await.unwrap();
    let first = router(state.clone(), options.clone());
    let second = router(state, options);
    let first_session = initialize(&first, Some("sdk-token")).await;
    let second_session = initialize(&second, Some("sdk-token")).await;
    let created = tool(
        &first,
        &first_session,
        "namespace.create",
        json!({"name":"multi-address"}),
        Some("sdk-token"),
    )
    .await;
    assert_eq!(
        common::value(&serde_json::from_value(created).unwrap())["name"],
        "multi-address"
    );
    let listed = tool(
        &second,
        &second_session,
        "namespace.list",
        json!({}),
        Some("sdk-token"),
    )
    .await;
    assert_eq!(
        common::value(&serde_json::from_value(listed).unwrap())["namespaces"][0]["name"],
        "multi-address"
    );
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
    let directory = Fixture::new();
    let app = app(directory.state_async().await.unwrap(), Some("secret-token"));
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
        34
    );
    let created = tool(&app, &session, "execute", json!({"script":"mcp('namespace.create', { 'name': 'http'})\nmcp('fs.write', { 'namespace': 'http', 'path': '/saved', 'text': 'persisted'})\n42"}), Some("secret-token")).await;
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
    let reopened = directory.state_async().await.unwrap();
    let file = tokio::task::spawn_blocking(move || {
        reopened.read_file(state_contract::file::FsReadInput {
            namespace: "http".into(),
            path: "/saved".into(),
        })
    })
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(file, state_contract::file::FileContents::Text { text, .. } if text == "persisted")
    );
}

#[tokio::test]
async fn bearer_auth_covers_every_http_method_and_rejects_ambiguous_credentials() {
    let directory = Fixture::new();
    let app = app(directory.state_async().await.unwrap(), Some("secret-token"));
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
    let directory = Fixture::new();
    let app = app(directory.state_async().await.unwrap(), None);
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
impl state_core::RuntimeBackend for SlowBackend {
    fn invoke_http(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: state_contract::http::HttpRequest,
        _: &state_runtime::Limits,
        _: &mut state_runtime::HostCallback<'_>,
    ) -> std::result::Result<state_runtime::HttpRunResult, state_runtime::RuntimeError> {
        Err(state_runtime::RuntimeError::new(
            state_contract::ErrorCode::PythonError,
            "test backend HTTP failure",
        ))
    }
    fn validate_http(
        &self,
        source: &str,
        modules: &state_runtime::ModuleSources,
        symbol: &str,
        limits: &state_runtime::Limits,
    ) -> std::result::Result<(), state_runtime::RuntimeError> {
        self.validate_module(source, modules, symbol, limits)
    }

    fn execute(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: Value,
        _: &state_runtime::Limits,
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
        _: Map<String, Value>,
        _: &state_runtime::Limits,
        _: &mut state_runtime::HostCallback<'_>,
    ) -> Result<state_runtime::RunResult, state_runtime::RuntimeError> {
        unreachable!()
    }
    fn validate_module(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: &state_runtime::Limits,
    ) -> Result<(), state_runtime::RuntimeError> {
        unreachable!()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn synchronous_invocation_does_not_block_async_http_requests() {
    let directory = Fixture::new();
    let state = Controller::with_backend(
        directory.store_async().await.unwrap(),
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
        34
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
    let directory = Fixture::new();
    let app = app(directory.state_async().await.unwrap(), None);
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
    *large.body_mut() = Body::from(vec![b'x'; crate::protocol::MAX_FRAME_BYTES + 1]);
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

struct HttpClient {
    app: Router,
    session: String,
    version: &'static str,
}
impl HttpClient {
    async fn new(app: Router, version: &'static str) -> Self {
        let mut init = request(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
            None,
            Some("sdk-token"),
        );
        init.headers_mut()
            .insert("mcp-protocol-version", version.parse().unwrap());
        let response = app.clone().oneshot(init).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            response_json(response).await["result"]["protocolVersion"],
            version
        );
        let client = Self {
            app,
            session,
            version,
        };
        let response = client
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        client
    }
    async fn send(&self, message: Value) -> axum::response::Response {
        let mut request = request(message, Some(&self.session), Some("sdk-token"));
        request
            .headers_mut()
            .insert("mcp-protocol-version", self.version.parse().unwrap());
        self.app.clone().oneshot(request).await.unwrap()
    }
    async fn list_tools(&self) -> Vec<Value> {
        response_json(
            self.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
                .await,
        )
        .await["result"]["tools"]
            .as_array()
            .unwrap()
            .clone()
    }
}
async fn http_tool(client: &HttpClient, name: &str, arguments: Value) -> Value {
    let response = client.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":arguments}})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = serde_json::from_value(response_json(response).await["result"].clone()).unwrap();
    common::value(&result)
}
#[tokio::test]
async fn http_sessions_share_state_across_protocol_versions() {
    let fixture = Fixture::new();
    let app = app(fixture.state_async().await.unwrap(), Some("sdk-token"));
    let first = HttpClient::new(app.clone(), "2025-06-18").await;
    let second = HttpClient::new(app, "2025-03-26").await;
    assert_eq!(first.list_tools().await.len(), 34);
    assert_eq!(second.list_tools().await.len(), 34);
    http_tool(&first, "namespace.create", json!({"name":"shared"})).await;
    http_tool(
        &first,
        "db.create",
        json!({"namespace":"shared","database":"app"}),
    )
    .await;
    http_tool(
        &first,
        "db.execute",
        json!({"namespace":"shared","database":"app","sql":"CREATE TABLE notes(text TEXT)"}),
    )
    .await;
    http_tool(&first, "fs.write", json!({"namespace":"shared","path":"/api.py","text":"def add(text):\n    return db_execute('app', 'INSERT INTO notes VALUES (?)', [text])"})).await;
    http_tool(
        &first,
        "function.declare",
        json!({"namespace":"shared","name":"add","file":"/api.py","symbol":"add"}),
    )
    .await;
    http_tool(
        &second,
        "call",
        json!({"namespace":"shared","function":"add","arguments":{"text":"from another session"}}),
    )
    .await;
    assert_eq!(
        http_tool(
            &first,
            "db.query",
            json!({"namespace":"shared","database":"app","sql":"SELECT text FROM notes"})
        )
        .await["rows"],
        json!([["from another session"]])
    );
}

#[tokio::test]
async fn tools_list_includes_required_cache_fields_for_current_protocol() {
    let directory = Fixture::new();
    let app = app(directory.state_async().await.unwrap(), None);
    let mut list = request(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"cache-validation","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{}
        }}}),
        None,
        None,
    );
    list.headers_mut()
        .insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
    list.headers_mut()
        .insert("mcp-method", "tools/list".parse().unwrap());
    let response = app.oneshot(list).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    assert_eq!(result["result"]["ttlMs"], 0);
    assert_eq!(result["result"]["cacheScope"], "public");
    assert_eq!(result["result"]["resultType"], "complete");
    assert_eq!(result["result"]["tools"].as_array().unwrap().len(), 34);
}

#[tokio::test]
async fn namespace_http_publication_urls_methods_and_binary_static()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = Fixture::new();
    let app = app(directory.state_async().await?, Some("secret"));
    let session = initialize(&app, Some("secret")).await;
    tool(
        &app,
        &session,
        "namespace.create",
        json!({"name":"web"}),
        Some("secret"),
    )
    .await;
    tool(&app,&session,"fs.write",json!({"namespace":"web","path":"/web.py","text":"def handle(request):\n    return request.body, 201, [('X-Method',request.method),('Set-Cookie','a=1'),('Set-Cookie','b=2')]"}),Some("secret")).await;
    let published=tool_value(&tool(&app,&session,"api.publish",json!({"namespace":"web","name":"echo","path":"/echo","target":{"kind":"function","file":"/web.py","symbol":"handle"}}),Some("secret")).await);
    assert_eq!(published["urls"], json!(["http://127.0.0.1:8000/web/echo"]));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("CUSTOM")
                .uri("/web/echo")
                .header("host", "127.0.0.1:8000")
                .header("authorization", "Bearer secret")
                .body(Body::from(vec![0, 255, 1]))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get("x-method")
            .and_then(|v| v.to_str().ok()),
        Some("CUSTOM")
    );
    assert_eq!(response.headers().get_all("set-cookie").iter().count(), 2);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await?.as_ref(),
        [0, 255, 1]
    );
    for method in ["GET", "POST", "HEAD", "OPTIONS", "CUSTOM"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/web/echo")
                    .header("host", "127.0.0.1:8000")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    tool(
        &app,
        &session,
        "fs.write",
        json!({"namespace":"web","path":"/public/index.html","text":"<h1>web</h1>"}),
        Some("secret"),
    )
    .await;
    tool(
        &app,
        &session,
        "fs.write",
        json!({"namespace":"web","path":"/public/data.bin","base64":"AP8="}),
        Some("secret"),
    )
    .await;
    let site=tool_value(&tool(&app,&session,"api.publish",json!({"namespace":"web","name":"site","path":"/site/{*path}","target":{"kind":"static","directory":"/public","index":"index.html"}}),Some("secret")).await);
    assert_eq!(site["urls"], json!(["http://127.0.0.1:8000/web/site/"]));
    let req = |method, path| {
        Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:8000")
            .header("authorization", "Bearer secret")
            .body(Body::empty())
    };
    let redirect = app.clone().oneshot(req("GET", "/web/site")?).await?;
    assert_eq!(redirect.status(), StatusCode::PERMANENT_REDIRECT);
    let response = app.clone().oneshot(req("GET", "/web/site/")?).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response
        .headers()
        .get("etag")
        .cloned()
        .ok_or("missing etag")?;
    assert_eq!(
        to_bytes(response.into_body(), 1024).await?.as_ref(),
        b"<h1>web</h1>"
    );
    let mut conditional = req("GET", "/web/site/")?;
    conditional.headers_mut().insert("if-none-match", etag);
    assert_eq!(
        app.clone().oneshot(conditional).await?.status(),
        StatusCode::NOT_MODIFIED
    );
    let head = app
        .clone()
        .oneshot(req("HEAD", "/web/site/data.bin")?)
        .await?;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()),
        Some("2")
    );
    assert!(to_bytes(head.into_body(), 1024).await?.is_empty());
    let get = app
        .clone()
        .oneshot(req("GET", "/web/site/data.bin")?)
        .await?;
    assert_eq!(to_bytes(get.into_body(), 1024).await?.as_ref(), [0, 255]);
    assert_eq!(
        app.clone()
            .oneshot(req("GET", "/web/site/%2e%2e/web.py")?)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.clone()
            .oneshot(req("GET", "/web/site/missing")?)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn prefixed_namespace_paths_keep_trailing_slashes_and_body_caps()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = Fixture::new();
    let controller = directory.state_async().await?;
    tokio::task::spawn_blocking({let c=controller.clone();move ||->state_core::Result<()> {
        c.create_namespace(serde_json::from_value(json!({"name":"web"})).map_err(|e|state_core::Error::new(state_core::ErrorCode::InvalidArgument,e.to_string()))?)?;
        c.write_file(serde_json::from_value(json!({"namespace":"web","path":"/web.py","text":"def handle(request):\n    write_text('/ran','yes')\n    return {'path':request.path, 'url':request.url}"})).map_err(|e|state_core::Error::new(state_core::ErrorCode::InvalidArgument,e.to_string()))?)?;
        c.publish_api(serde_json::from_value(json!({"namespace":"web","name":"path","path":"/path/","methods":["POST"],"target":{"kind":"function","file":"/web.py","symbol":"handle"}})).map_err(|e|state_core::Error::new(state_core::ErrorCode::InvalidArgument,e.to_string()))?)?;Ok(())
    }}).await??;
    let app = router(
        controller.clone(),
        HttpOptions::new("127.0.0.1:8000".parse()?, None)?.with_prefix("/prefix".parse()?),
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/prefix/web/path/")
                .header("host", "127.0.0.1:8000")
                .body(Body::from(vec![0; 1024 * 1024 + 1]))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    tokio::task::spawn_blocking(move || {
        assert!(
            controller
                .read_file(
                    serde_json::from_value(json!({"namespace":"web","path":"/ran"})).map_err(
                        |e| state_core::Error::new(
                            state_core::ErrorCode::InvalidArgument,
                            e.to_string()
                        )
                    )?
                )
                .is_err()
        );
        Ok::<_, state_core::Error>(())
    })
    .await??;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/prefix/web/path/")
                .header("host", "127.0.0.1:8000")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await?)?;
    assert_eq!(body["path"], "/prefix/web/path/");
    assert_eq!(body["url"], "http://127.0.0.1:8000/prefix/web/path/");
    assert_eq!(
        app.oneshot(
            Request::builder()
                .uri("/prefix/web/path")
                .header("host", "127.0.0.1:8000")
                .body(Body::empty())?
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

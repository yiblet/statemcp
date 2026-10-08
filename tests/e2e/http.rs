use super::common::{self, Directory};
use serde_json::json;
use state_core::State;
use statemcp::http::{HttpOptions, router};
use std::time::Duration;

#[tokio::test]
async fn multiple_listeners_serve_shared_state_with_official_clients() -> anyhow::Result<()> {
    let _serial = super::common::serial_async().await;
    use rmcp::{
        ServiceExt,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let directory = Directory::new();
    let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addresses = [first.local_addr().unwrap(), second.local_addr().unwrap()];
    let path = directory.0.clone();
    let state = tokio::task::spawn_blocking(move || State::open(path)).await??;
    let options = HttpOptions::for_addresses(&addresses, Some("sdk-token".into()))
        .unwrap()
        .with_prefix("/statemcp".parse().unwrap());
    let server = tokio::spawn(statemcp::http::serve_listeners(
        vec![first, second],
        state,
        options,
    ));
    let transport = |address| {
        StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!(
                "http://{address}/statemcp/mcp/"
            ))
            .auth_header("sdk-token"),
        )
    };
    let first = common::config()
        .serve(transport(addresses[0]))
        .await
        .unwrap();
    let second = common::config()
        .serve(transport(addresses[1]))
        .await
        .unwrap();
    common::tool(&first, "namespace.create", json!({"name":"multi-listener"})).await;
    let namespaces = common::tool(&second, "namespace.list", json!({})).await;
    assert_eq!(namespaces["namespaces"][0]["name"], "multi-listener");
    first.cancel().await.unwrap();
    second.cancel().await.unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    // Dropping the server task releases every listener, rather than leaking tasks.
    for address in addresses {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match tokio::net::TcpListener::bind(address).await {
                    Ok(_) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                        tokio::task::yield_now().await;
                    }
                    Err(error) => panic!("cannot rebind {address}: {error}"),
                }
            }
        })
        .await
        .expect("server task leaked a listener");
    }
    Ok(())
}

#[test]
fn all_addresses_must_bind_before_creating_state() {
    let _serial = super::common::serial();
    let directory = Directory::new();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_statemcp"))
        .arg("http")
        .arg(&directory.0)
        .args([
            "--bind",
            "127.0.0.1:0",
            "--bind",
            &occupied.local_addr().unwrap().to_string(),
            "--http-prefix",
            "/statemcp",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot listen"));
    assert!(!directory.0.exists());
}

#[tokio::test]
async fn official_http_clients_share_state_across_protocol_versions() -> anyhow::Result<()> {
    let _serial = super::common::serial_async().await;
    use rmcp::{
        ServiceExt,
        model::ClientConfig,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let directory = Directory::new();
    let path = directory.0.clone();
    let state = tokio::task::spawn_blocking(move || State::open(path)).await??;
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
    Ok(())
}

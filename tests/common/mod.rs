#![allow(dead_code)]
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ProtocolVersion},
    service::RunningService,
};
use serde_json::Value;
use statemcp::{Server, State};
use std::path::PathBuf;

pub type Client = RunningService<RoleClient, ClientConfig>;

pub struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("statemcp-sdk-{}-{unique}", std::process::id())))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn config() -> ClientConfig {
    ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_06_18)
}

pub async fn connect(state: State) -> Client {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        Server::new(state)
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    config().serve(client_io).await.unwrap()
}

pub fn request(name: &str, arguments: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_owned())
        .with_arguments(arguments.as_object().unwrap().clone())
}

pub fn value(result: &CallToolResult) -> Value {
    assert!(result.structured_content.is_none());
    assert_eq!(result.content.len(), 1);
    let block = serde_json::to_value(&result.content[0]).unwrap();
    assert_eq!(block["type"], "text");
    serde_json::from_str(block["text"].as_str().unwrap()).unwrap()
}

pub async fn tool(client: &Client, name: &str, arguments: Value) -> Value {
    let result = client.call_tool(request(name, arguments)).await.unwrap();
    assert_ne!(result.is_error, Some(true), "{result:?}");
    value(&result)
}

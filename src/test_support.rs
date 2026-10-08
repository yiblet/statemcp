use crate::{Controller, Server};
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ProtocolVersion},
    service::RunningService,
};
use serde_json::Value;

pub type Client = RunningService<RoleClient, ClientConfig>;

#[derive(Clone)]
pub struct Fixture(state_fs::MemoryFs);
impl Fixture {
    pub fn new() -> Self {
        Self(state_fs::MemoryFs::new())
    }
    pub fn store(&self) -> state_core::Result<state_domain::Store> {
        let backing = state_store::Store::builder()
            .with_fs(self.0.clone())
            .open("/state")
            .map_err(state_domain::Error::from)?;
        Ok(state_domain::Store::from_storage(backing)?)
    }
    pub fn state(&self) -> state_core::Result<Controller> {
        Ok(Controller::with_backend(
            self.store()?,
            std::sync::Arc::new(state_core::EmbeddedBackend),
        ))
    }
    pub async fn store_async(&self) -> anyhow::Result<state_domain::Store> {
        let fixture = self.clone();
        Ok(tokio::task::spawn_blocking(move || fixture.store()).await??)
    }
    pub async fn state_async(&self) -> anyhow::Result<Controller> {
        let fixture = self.clone();
        Ok(tokio::task::spawn_blocking(move || fixture.state()).await??)
    }
}

pub fn config() -> ClientConfig {
    ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_06_18)
}

pub async fn connect(state: Controller) -> Client {
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

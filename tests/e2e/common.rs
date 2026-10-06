use rmcp::{
    RoleClient,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ProtocolVersion},
    service::RunningService,
};
use serde_json::Value;
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
        let path = self.0.clone();
        let remove = move || match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, path = %path.display(), "cannot remove E2E state"),
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(remove);
            }
            Err(_) => remove(),
        }
    }
}

pub fn config() -> ClientConfig {
    ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_06_18)
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

// Keep real-I/O scenarios serial. Concurrent process spawning can briefly
// inherit another test's open lock descriptors across fork, even with CLOEXEC.
// Use an async acquisition in async tests so waiting never blocks Tokio workers.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.blocking_lock()
}
pub async fn serial_async() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.lock().await
}

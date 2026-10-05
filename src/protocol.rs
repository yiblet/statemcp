//! MCP handlers served by the official Rust SDK (`rmcp`).
use crate::State;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

const SERVER_INSTRUCTIONS: &str = "StateMCP lets you create SQLite databases and publish Python tools that use them. Create a namespace and databases, write Python source with fs.write, then publish with function.declare and invoke with call. Use describe with mode=runtime for Python helpers and mode=full for exact schemas. Namespace selectors accept names or stable UUIDs. Published functions have full access to their own namespace only; root execute scripts have owner access. Use expected_revision and expected_version for optimistic concurrency. Tool content contains a JSON-encoded text block; isError indicates an application failure.";

/// Clones share the store and invocation budget across all transports and clients.
#[derive(Clone)]
pub struct Server {
    state: State,
    slots: Arc<Semaphore>,
}

impl Server {
    pub fn new(state: State) -> Self {
        Self {
            state,
            slots: Arc::new(Semaphore::new(16)),
        }
    }
}

fn tools() -> &'static [Tool] {
    static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        tool_definitions()
            .into_iter()
            .map(|definition| serde_json::from_value(definition).expect("canonical tool schema"))
            .collect()
    })
}

fn tool_error(code: &str, message: &str) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(
        json!({"error":{"code":code,"message":message}}).to_string(),
    )])
    .into()
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("statemcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(SERVER_INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|request| request.cursor.is_some()) {
            return Err(ErrorData::invalid_params(
                "This server does not issue pagination cursors",
                None,
            ));
        }
        Ok(ListToolsResult::with_all_items(tools().to_vec()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !tools().iter().any(|tool| tool.name == request.name) {
            return Err(ErrorData::invalid_params("Unknown tool", None));
        }
        let permit = match self.slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return Ok(tool_error(
                    "BUSY",
                    "Too many active invocations; retry later",
                ));
            }
        };
        let state = self.state.clone();
        let name = request.name.into_owned();
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let span = tracing::info_span!("tool_call", tool = %name);
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _span = span.enter();
            // Keep SQLite and Monty callbacks off the async network workers.
            state.dispatch(&name, arguments)
        })
        .await;
        match result {
            Ok(Ok(value)) => {
                Ok(CallToolResult::success(vec![ContentBlock::text(value.to_string())]).into())
            }
            Ok(Err(error)) => {
                tracing::debug!(code = %error.code, "Tool invocation failed");
                Ok(tool_error(&error.code, &error.message))
            }
            Err(error) => {
                tracing::error!(%error, "Invocation worker failed");
                Err(ErrorData::internal_error("Invocation worker failed", None))
            }
        }
    }
}

/// Fixed discovery surface, shared with application validation.
pub fn tool_definitions() -> Vec<Value> {
    state_core::tool_definitions()
}

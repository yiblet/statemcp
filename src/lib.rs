//! Embeddable State MCP services and a synchronous, newline-delimited MCP adapter.
pub mod protocol;

pub use protocol::{Dispatcher, Server, ToolError, UnsupportedDispatcher, tool_definitions};

/// Transactional State service and runtime configuration for Rust embedding.
pub use state_core::{
    CoreLimits, EmbeddedBackend, Error as CoreError, Limits, RuntimeBackend, State, Store,
    WorkerConfig,
};

impl Dispatcher for State {
    fn dispatch(
        &mut self,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        State::dispatch(self, tool, args).map_err(|error| ToolError::new(error.code, error.message))
    }
}

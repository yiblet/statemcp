//! Embeddable State MCP services and a synchronous, newline-delimited MCP adapter.
pub mod protocol;

pub use protocol::{Dispatcher, Server, ToolError, UnsupportedDispatcher, tool_definitions};

//! StateMCP services and official Rust MCP SDK transports.
pub mod http;
pub mod protocol;

pub use protocol::{Server, tool_definitions};

/// Transactional State service and runtime configuration for Rust embedding.
pub use state_core::{
    CoreLimits, EmbeddedBackend, Error as CoreError, Limits, RuntimeBackend, State, Store, Tool,
    WorkerConfig,
};

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod test_support;

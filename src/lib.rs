//! StateMCP services and official Rust MCP SDK transports.
mod blocking;
pub use blocking::{BlockingError, BlockingExecutor};
pub mod http;
pub mod protocol;

pub use protocol::{Server, tool_definitions};

use state_core::{Controller, Error as CoreError};

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod test_support;

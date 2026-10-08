//! StateMCP services and official Rust MCP SDK transports.
pub mod http;
pub mod protocol;

pub use protocol::{Server, tool_definitions};

use state_core::{Error as CoreError, State};

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod test_support;

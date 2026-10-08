//! Typed feature services with application controllers and explicitly shared execution context.
pub use state_contract::ErrorCode;
mod api;
pub use api::Api;
mod http_application;
pub use http_application::HttpApplication;
mod context;
mod controllers;
pub use controllers::Controller;
mod databases;
mod files;
mod functions;
mod ports;
pub use ports::{CodeExecutor, SourceProvider, Sources};
mod namespaces;
pub use context::Context;
pub use databases::Databases;
pub use execution::Execution;
pub use files::Files;
pub use functions::Functions;
pub use namespaces::Namespaces;
mod discovery;
mod execution;
pub use discovery::{Discovery, DiscoveryService};
mod hosts;

mod host_tools;
mod receipt;
pub use receipt::ReceiptIdentity;
mod policy;
mod runtime;
mod schemas;
mod selectors;

pub use runtime::{EmbeddedBackend, Runtime, RuntimeBackend};
pub use schemas::tool_definitions;
use state_contract::operations::Operation;
use state_contract::operations::{DatabaseAction, FileAction, FunctionAction, NamespaceAction};
pub use state_runtime::{Limits, WorkerConfig};

use serde::{Deserialize, Serialize};
use std::{fmt, time::Instant};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(crate::ErrorCode::InvalidArgument, message)
    }
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::new(crate::ErrorCode::LimitExceeded, message)
    }
    pub(crate) fn denied() -> Self {
        Self::new(
            crate::ErrorCode::PermissionDenied,
            "operation is outside the function namespace",
        )
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<state_domain::Error> for Error {
    fn from(e: state_domain::Error) -> Self {
        Self::new(e.code, e.message)
    }
}
impl From<state_runtime::RuntimeError> for Error {
    fn from(e: state_runtime::RuntimeError) -> Self {
        Self::new(e.code, e.message)
    }
}
impl From<Error> for state_runtime::RuntimeError {
    fn from(e: Error) -> Self {
        Self::new(e.code, e.message)
    }
}

#[derive(Clone, Debug)]
pub struct CoreLimits {
    pub runtime: Limits,
    /// Maximum simultaneously executing scripts/endpoints, including the root.
    pub max_depth: usize,
    /// Direct service results. Runtime JSON keeps its separate, smaller limit.
    pub max_result_bytes: usize,
}
impl Default for CoreLimits {
    fn default() -> Self {
        Self {
            runtime: Limits::default(),
            max_depth: 16,
            max_result_bytes: 16 * 1024 * 1024,
        }
    }
}

pub(crate) fn bounded(value: &impl Serialize, max: usize) -> Result<()> {
    if serde_json::to_vec(value)
        .map_err(|e| Error::invalid(e.to_string()))?
        .len()
        > max
    {
        return Err(Error::limit("JSON value exceeds output byte limit"));
    }
    Ok(())
}
pub(crate) fn remaining(start: Instant, limits: &Limits) -> Result<Limits> {
    let mut runtime = limits.clone();
    runtime.max_duration = runtime
        .max_duration
        .checked_sub(start.elapsed())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| Error::limit("root invocation deadline exceeded"))?;
    Ok(runtime)
}

#[cfg(test)]
mod budgets_receipts_tests;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod test_support;

impl From<state_contract::ParseError> for Error {
    fn from(error: state_contract::ParseError) -> Self {
        Self::new(error.code, error.message)
    }
}

use serde_json::Value;
use state_runtime::{HostCallback, Limits, RunResult, RuntimeError, WorkerConfig};

/// Backends must terminate on callback errors; callbacks are the only state access.
/// Shared references permit nested invocation using the same backend configuration.
pub trait RuntimeBackend: Send + Sync {
    fn execute(
        &self,
        source: &str,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError>;
    fn invoke(
        &self,
        source: &str,
        symbol: &str,
        arguments: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError>;
    fn validate_module(
        &self,
        source: &str,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError>;
}

/// Cooperative in-process runtime. Choose WorkerConfig for process isolation.
pub struct EmbeddedBackend;
impl RuntimeBackend for EmbeddedBackend {
    fn execute(
        &self,
        source: &str,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        state_runtime::execute(source, inputs, limits, host)
    }
    fn invoke(
        &self,
        source: &str,
        symbol: &str,
        arguments: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        state_runtime::invoke(source, symbol, arguments, limits, host)
    }
    fn validate_module(
        &self,
        source: &str,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        state_runtime::validate_module(source, symbol, limits)
    }
}
impl RuntimeBackend for WorkerConfig {
    fn execute(
        &self,
        source: &str,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        WorkerConfig::execute(self, source, inputs, limits, host)
    }
    fn invoke(
        &self,
        source: &str,
        symbol: &str,
        arguments: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        WorkerConfig::invoke(self, source, symbol, arguments, limits, host)
    }
    fn validate_module(
        &self,
        source: &str,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        WorkerConfig::validate_module(self, source, symbol, limits)
    }
}

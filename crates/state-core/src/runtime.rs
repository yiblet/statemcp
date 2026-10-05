use serde_json::Value;
use state_runtime::{HostCallback, Limits, ModuleSources, RunResult, RuntimeError, WorkerConfig};

/// Backends must terminate on callback errors; callbacks are the only state access.
/// Shared references permit nested invocation using the same backend configuration.
pub trait RuntimeBackend: Send + Sync {
    fn execute(
        &self,
        source: &str,
        modules: &ModuleSources,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError>;
    fn invoke(
        &self,
        source: &str,
        modules: &ModuleSources,
        symbol: &str,
        arguments: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError>;
    fn validate_module(
        &self,
        source: &str,
        modules: &ModuleSources,
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
        modules: &ModuleSources,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        state_runtime::execute_with_modules(source, inputs, modules, limits, host)
    }
    fn invoke(
        &self,
        source: &str,
        modules: &ModuleSources,
        symbol: &str,
        arguments: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        state_runtime::invoke_with_modules(source, symbol, arguments, modules, limits, host)
    }
    fn validate_module(
        &self,
        source: &str,
        modules: &ModuleSources,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        state_runtime::validate_module_with_modules(source, symbol, modules, limits)
    }
}
impl RuntimeBackend for WorkerConfig {
    fn execute(
        &self,
        source: &str,
        modules: &ModuleSources,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        WorkerConfig::execute_with_modules(self, source, inputs, modules, limits, host)
    }
    fn invoke(
        &self,
        source: &str,
        modules: &ModuleSources,
        symbol: &str,
        arguments: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        WorkerConfig::invoke_with_modules(self, source, symbol, arguments, modules, limits, host)
    }
    fn validate_module(
        &self,
        source: &str,
        modules: &ModuleSources,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        WorkerConfig::validate_module_with_modules(self, source, symbol, modules, limits)
    }
}

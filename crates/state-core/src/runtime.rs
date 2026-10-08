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
        arguments: serde_json::Map<String, Value>,
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
        arguments: serde_json::Map<String, Value>,
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
        arguments: serde_json::Map<String, Value>,
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

/// Configured interpreter handle shared by publication and execution services.
/// Cloning retains the same backend, including custom backend state.
#[derive(Clone)]
pub struct Runtime {
    backend: std::sync::Arc<dyn RuntimeBackend>,
}
impl Runtime {
    /// Own a configured backend. Backend operations are synchronous and must run off async workers.
    pub fn new(backend: std::sync::Arc<dyn RuntimeBackend>) -> Self {
        Self { backend }
    }
    /// Construct the cooperative embedded interpreter.
    pub fn embedded() -> Self {
        Self::new(std::sync::Arc::new(EmbeddedBackend))
    }
    /// Execute a script with a host adapter supplied only for this run.
    pub fn execute(
        &self,
        source: &str,
        modules: &ModuleSources,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.backend.execute(source, modules, inputs, limits, host)
    }
    /// Invoke a pinned symbol with a host adapter supplied only for this run.
    pub fn invoke(
        &self,
        source: &str,
        modules: &ModuleSources,
        symbol: &str,
        arguments: serde_json::Map<String, Value>,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.backend
            .invoke(source, modules, symbol, arguments, limits, host)
    }
    /// Validate source and symbol without allowing initialization-time host effects.
    pub fn validate_module(
        &self,
        source: &str,
        modules: &ModuleSources,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        self.backend
            .validate_module(source, modules, symbol, limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct CountingBackend(AtomicUsize);
    impl RuntimeBackend for CountingBackend {
        fn execute(
            &self,
            _: &str,
            _: &ModuleSources,
            _: Value,
            _: &Limits,
            _: &mut HostCallback<'_>,
        ) -> Result<RunResult, RuntimeError> {
            Ok(RunResult {
                value: Value::from(self.0.fetch_add(1, Ordering::SeqCst) + 1),
                stdout: String::new(),
            })
        }
        fn invoke(
            &self,
            source: &str,
            modules: &ModuleSources,
            _: &str,
            _: serde_json::Map<String, Value>,
            limits: &Limits,
            host: &mut HostCallback<'_>,
        ) -> Result<RunResult, RuntimeError> {
            self.execute(source, modules, Value::Null, limits, host)
        }
        fn validate_module(
            &self,
            _: &str,
            _: &ModuleSources,
            _: &str,
            _: &Limits,
        ) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    #[test]
    fn service_clone_shares_configured_runtime_backend() -> crate::Result<()> {
        let runtime = Runtime::new(Arc::new(CountingBackend(AtomicUsize::new(0))));
        let clone = runtime.clone();
        let modules = ModuleSources::default();
        let limits = Limits::default();
        let mut host = |_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>| Ok(Value::Null);
        assert_eq!(
            runtime
                .execute("", &modules, Value::Null, &limits, &mut host)?
                .value,
            Value::from(1)
        );
        assert_eq!(
            clone
                .execute("", &modules, Value::Null, &limits, &mut host)?
                .value,
            Value::from(2)
        );
        Ok(())
    }
}

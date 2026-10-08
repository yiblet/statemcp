//! Heavy execution boundaries independent of concrete feature services.
use crate::{Context, Result};
use state_contract::function::FunctionSources;
use std::sync::Arc;

/// Loads the entry and Python modules from one transaction's namespace snapshot.
/// Implementations must preserve that snapshot and enforce the supplied source byte cap.
/// Runtime validation and persisted publication consume the same owned result.
pub trait SourceProvider: Send + Sync {
    /// Load a source snapshot; errors abort publication and poison its context.
    fn snapshot(
        &self,
        namespace: &str,
        file: &str,
        max_source_bytes: usize,
        context: &mut Context,
    ) -> Result<FunctionSources>;
}

/// Configured source loader shared by publication services and their clones.
#[derive(Clone)]
pub struct Sources {
    provider: Arc<dyn SourceProvider>,
}
impl Sources {
    /// Supply real storage reads or a controlled provider for testing.
    pub fn new(provider: Arc<dyn SourceProvider>) -> Self {
        Self { provider }
    }
    pub(crate) fn snapshot(
        &self,
        namespace: &str,
        file: &str,
        max_source_bytes: usize,
        context: &mut Context,
    ) -> Result<FunctionSources> {
        self.provider
            .snapshot(namespace, file, max_source_bytes, context)
    }
}

/// Executes nested code in a context supplied by the host boundary.
/// Implementations must retain its transaction, established authority and budget accounting.
/// This narrow port prevents host argument parsing from depending on its concrete orchestrator.
pub trait CodeExecutor {
    /// Invoke a function without opening or committing a root transaction.
    fn call_function(
        &self,
        input: state_contract::execution::CallInput,
        context: &mut Context,
    ) -> Result<serde_json::Value>;
    /// Run a script without opening or committing a root transaction.
    fn execute_script(
        &self,
        input: state_contract::execution::ExecuteInput,
        context: &mut Context,
    ) -> Result<state_contract::execution::ExecutionResult>;
}

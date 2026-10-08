//! Execution state shared by every direct and nested operation.
use crate::policy::{Access, NamespaceId};
use crate::{CoreLimits, Error, Result, remaining};
use state_domain::Transaction;
use std::time::Instant;

/// One exclusively owned transaction and its execution guarantees.
/// Services borrow this context; only the root controller may finish it.
pub struct Context {
    transaction: Transaction,
    access: Access,
    limits: CoreLimits,
    start: Instant,
    failure: Option<Error>,
    calls: usize,
    depth: usize,
    stdout_bytes: usize,
}
impl Context {
    pub(crate) fn new(transaction: Transaction, limits: CoreLimits) -> Self {
        Self {
            transaction,
            access: Access::Owner,
            limits,
            start: Instant::now(),
            failure: None,
            calls: 0,
            depth: 0,
            stdout_bytes: 0,
        }
    }
    pub(crate) fn transaction(&mut self) -> &mut Transaction {
        &mut self.transaction
    }
    pub(crate) fn finish(self) -> Result<state_contract::publication::CommitOutcome> {
        self.check()?;
        self.transaction.commit().map_err(Into::into)
    }
    pub(crate) fn limits(&self) -> &CoreLimits {
        &self.limits
    }
    pub(crate) fn runtime_limits(&self) -> Result<state_runtime::Limits> {
        remaining(self.start, &self.limits.runtime)
    }
    pub(crate) fn check(&self) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.runtime_limits().map(|_| ())
    }
    pub(crate) fn record<T>(&mut self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    pub(crate) fn operation<T>(
        &mut self,
        action: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let result = (|| {
            self.start_operation()?;
            let value = action(self)?;
            self.check()?;
            Ok(value)
        })();
        self.record(result)
    }
    fn start_operation(&mut self) -> Result<()> {
        self.check()?;
        self.calls = self.calls.saturating_add(1);
        if self.calls > self.limits.runtime.max_calls {
            return Err(Error::limit("root operation budget exceeded"));
        }
        Ok(())
    }
    pub(crate) fn default_namespace(&self) -> Option<&str> {
        match &self.access {
            Access::Owner => None,
            Access::Namespace(id) => Some(id.as_str()),
        }
    }
    pub(crate) fn require_owner(&self) -> Result<()> {
        match self.access {
            Access::Owner => Ok(()),
            Access::Namespace(_) => Err(Error::denied()),
        }
    }
    pub(crate) fn namespace(&self, selector: &str) -> Result<NamespaceId> {
        let selector = match &self.access {
            Access::Namespace(id) if selector == "self" => id.as_str(),
            _ => selector,
        };
        let target = self.transaction.namespace_identity(selector)?;
        if let Access::Namespace(scope) = &self.access {
            crate::policy::check_namespace(scope.as_str(), target)?;
        }
        Ok(NamespaceId::resolved(target.to_owned()))
    }
    pub(crate) fn scoped<T>(
        &mut self,
        namespace: NamespaceId,
        action: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let previous = std::mem::replace(&mut self.access, Access::Namespace(namespace));
        let result = action(self);
        self.access = previous;
        self.record(result)
    }
    pub(crate) fn execution<T>(
        &mut self,
        action: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if self.depth >= self.limits.max_depth {
            return self.record(Err(Error::limit("root endpoint/script depth exceeded")));
        }
        self.depth += 1;
        let result = action(self);
        self.depth -= 1;
        self.record(result)
    }
    pub(crate) fn account_stdout(&mut self, stdout: &str) -> Result<()> {
        self.stdout_bytes = self.stdout_bytes.saturating_add(stdout.len());
        if self.stdout_bytes > self.limits.runtime.max_output_bytes {
            return Err(Error::limit("root diagnostic output budget exceeded"));
        }
        Ok(())
    }
}

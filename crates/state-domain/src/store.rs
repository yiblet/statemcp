//! Domain catalog access over the injected overlay store.
pub(crate) mod catalog;
mod maintenance;
mod migrations;

use crate::{Result, Transaction};
pub use maintenance::MaintenanceReport;
use std::path::Path;

/// Namespace and resource repository backed by atomic overlay invocations.
#[derive(Clone, Debug)]
pub struct Store {
    backing: state_store::Store,
}
impl Store {
    /// Open physical storage and initialize the domain catalog.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_storage(state_store::Store::open(path)?)
    }
    /// Initialize a repository over an existing store, including a memory backend.
    pub fn from_storage(backing: state_store::Store) -> Result<Self> {
        backing.migrate(&migrations::service()?)?;
        Ok(Self { backing })
    }
    /// Begin one exclusive invocation covering metadata and all resource databases.
    pub fn begin(&self) -> Result<Transaction> {
        let backing = self.backing.begin()?;
        let (generation, namespaces) = {
            let catalog = backing.connect(&catalog::name()?)?;
            catalog.execute_batch("PRAGMA foreign_keys=ON")?;
            catalog::load(&catalog)?
        };
        Ok(Transaction::new(backing, generation, namespaces))
    }
    /// Read an idempotency receipt under shared root ownership.
    pub fn receipt(&self, principal: &str, key: &str) -> Result<Option<crate::Receipt>> {
        let reader = self.backing.begin_read()?;
        let catalog = reader.connect(&catalog::name()?)?;
        catalog::receipt(&catalog, principal, key)
    }
}

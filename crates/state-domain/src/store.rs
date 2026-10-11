//! Domain catalog access over the injected overlay store.
pub(crate) mod catalog;
#[cfg(test)]
mod catalog_tests;
mod maintenance;
mod migrations;
mod records;
mod retention;

use crate::{Result, Transaction};
pub use maintenance::MaintenanceReport;
pub use retention::HistorySize;
use std::path::Path;

/// Namespace and resource repository backed by atomic overlay invocations.
#[derive(Clone, Debug)]
pub struct Store {
    backing: state_store::Store,
    history_size: HistorySize,
}
impl Store {
    /// Open physical storage and initialize the domain catalog.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_history_size(path, HistorySize::default())
    }
    /// Open storage with positive, per-namespace revision retention.
    /// A startup migration converts existing catalogs to content-addressed sources.
    pub fn open_with_history_size(
        path: impl AsRef<Path>,
        history_size: HistorySize,
    ) -> Result<Self> {
        Self::from_storage_with_history_size(state_store::Store::open(path)?, history_size)
    }
    /// Initialize a repository over an existing store, including a memory backend.
    pub fn from_storage(backing: state_store::Store) -> Result<Self> {
        Self::from_storage_with_history_size(backing, HistorySize::default())
    }
    /// Initialize injected storage with configurable revision retention.
    pub fn from_storage_with_history_size(
        backing: state_store::Store,
        history_size: HistorySize,
    ) -> Result<Self> {
        backing.migrate(&migrations::service(history_size)?)?;
        Ok(Self {
            backing,
            history_size,
        })
    }
    /// Begin one exclusive invocation covering metadata and all resource databases.
    pub fn begin(&self) -> Result<Transaction> {
        let backing = self.backing.begin()?;
        let (generation, namespaces) = {
            let catalog = backing.connect(&catalog::name()?)?;
            catalog.execute_batch("PRAGMA foreign_keys=ON")?;
            catalog::load(&catalog)?
        };
        Ok(Transaction::new(
            backing,
            generation,
            namespaces,
            self.history_size,
        ))
    }
    /// Read an idempotency receipt under shared root ownership.
    pub fn receipt(&self, principal: &str, key: &str) -> Result<Option<crate::Receipt>> {
        let reader = self.backing.begin_read()?;
        let catalog = reader.connect(&catalog::name()?)?;
        catalog::receipt(&catalog, principal, key)
    }
}

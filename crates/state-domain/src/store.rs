//! Domain catalog access over the injected overlay store.
pub(crate) mod catalog;
#[cfg(test)]
mod catalog_tests;
mod maintenance;
mod migrations;
pub(crate) mod records;
mod retention;

use crate::model::Namespace;
use crate::{Result, Transaction};
pub use maintenance::MaintenanceReport;
pub use retention::HistorySize;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

/// Namespace and resource repository backed by atomic overlay invocations.
#[derive(Clone)]
pub struct Store {
    backing: state_store::Store,
    history_size: HistorySize,
    snapshot: Arc<Mutex<Option<CatalogSnapshot>>>,
    loader: Arc<dyn crate::content::ContentLoader>,
}
struct CatalogSnapshot {
    generation: i64,
    namespaces: BTreeMap<String, Namespace>,
}
impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("backing", &self.backing)
            .field("history_size", &self.history_size)
            .finish_non_exhaustive()
    }
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
            snapshot: Arc::new(Mutex::new(None)),
            loader: Arc::new(crate::content::CachingLoader::new(
                crate::content::DirectLoader,
                32 * 1024 * 1024,
            )),
        })
    }
    /// Select a composable loader for committed file and pinned source content.
    /// Clones of this store share its loader; transactions retain the loader
    /// selected when they begin. The default is a 32 MiB content cache.
    pub fn with_content_loader(
        mut self,
        loader: impl crate::content::ContentLoader + 'static,
    ) -> Self {
        self.loader = Arc::new(loader);
        self
    }
    /// Begin one exclusive invocation covering metadata and all resource databases.
    pub fn begin(&self) -> Result<Transaction> {
        let started = std::time::Instant::now();
        let backing = self.backing.begin()?;
        let locked = std::time::Instant::now();
        let (generation, namespaces) = {
            let catalog = backing.connect(&catalog::name()?)?;
            catalog.execute_batch("PRAGMA foreign_keys=ON")?;
            // Root ownership excludes writers while the generation is checked
            // and the snapshot is used. Other processes publish a new generation,
            // so caching requires no single-process assumption or extra lock mode.
            let current: i64 = catalog.query_row(
                "SELECT generation FROM catalog_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            let mut cached = self.snapshot.lock().map_err(|_| {
                crate::Error::new(
                    crate::ErrorCode::CorruptStore,
                    "catalog snapshot lock is poisoned",
                )
            })?;
            if cached
                .as_ref()
                .is_none_or(|snapshot| snapshot.generation != current)
            {
                let (generation, namespaces) = catalog::load(&catalog)?;
                *cached = Some(CatalogSnapshot {
                    generation,
                    namespaces,
                });
            }
            let snapshot = cached.as_ref().ok_or_else(|| {
                crate::Error::new(
                    crate::ErrorCode::CorruptStore,
                    "catalog snapshot is missing",
                )
            })?;
            // Invocations own a mutable view. Copying it preserves rollback and
            // prevents unpublished edits from entering the shared snapshot.
            (snapshot.generation, snapshot.namespaces.clone())
        };
        tracing::debug!(target: "statemcp::timing", phase = "catalog_load",
            lock_ms = locked.duration_since(started).as_secs_f64() * 1000.0,
            load_ms = locked.elapsed().as_secs_f64() * 1000.0,
            namespaces = namespaces.len(), "store opened invocation");
        Ok(Transaction::new(
            backing,
            generation,
            namespaces,
            self.history_size,
            Arc::clone(&self.loader),
        ))
    }
    /// Read an idempotency receipt under shared root ownership.
    pub fn receipt(&self, principal: &str, key: &str) -> Result<Option<crate::Receipt>> {
        let reader = self.backing.begin_read()?;
        let catalog = reader.connect(&catalog::name()?)?;
        catalog::receipt(&catalog, principal, key)
    }
}

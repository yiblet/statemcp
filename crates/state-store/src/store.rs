//! Store handle and entry point for consistent root invocations.
mod catalog;
mod maintenance;

pub use maintenance::MaintenanceReport;

use crate::{Result, Transaction};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct Store {
    root: Arc<PathBuf>,
}

impl Store {
    pub fn begin(&self) -> Result<Transaction> {
        let guard = crate::durability::root_lock(self.root(), false)?;
        let (generation, namespaces) = self.load()?;
        Transaction::new(self.clone(), generation, namespaces, guard)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

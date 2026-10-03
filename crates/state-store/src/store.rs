//! Store handle and entry point for consistent root invocations.
mod catalog;

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
        let (generation, namespaces) = self.load()?;
        Transaction::new(self.clone(), generation, namespaces)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

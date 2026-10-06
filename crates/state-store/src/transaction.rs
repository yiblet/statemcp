//! Root invocation lifecycle, operation poisoning, and all-or-nothing publication.
mod domain;
mod files;
mod functions;
mod namespaces;
mod objects;
mod snapshots;

use self::snapshots::WorkingDatabase;
use crate::{Error, Receipt, Result, Store, identity::id, model::Namespace, sql};
use std::{collections::BTreeMap, fs, path::PathBuf};

/// A consistent root invocation. Any failed operation poisons publication.
/// Dropping it discards all logical changes; unreachable sealed files are retained.
pub struct Transaction {
    store: Store,
    // Held through publication and the Drop implementation's staging cleanup.
    _root_guard: std::fs::File,
    generation: i64,
    namespaces: BTreeMap<String, Namespace>,
    objects: BTreeMap<String, Vec<u8>>,
    staging: PathBuf,
    poisoned: bool,
    working_databases: BTreeMap<(String, String), WorkingDatabase>,
    receipt: Option<(String, String, Receipt)>,
}
impl Drop for Transaction {
    fn drop(&mut self) {
        self.working_databases.clear();
        let _ = fs::remove_dir_all(&self.staging);
    }
}

impl Transaction {
    pub(crate) fn new(
        store: Store,
        generation: i64,
        namespaces: BTreeMap<String, Namespace>,
        guard: std::fs::File,
    ) -> Result<Self> {
        let staging = store.root().join("staging").join(id());
        fs::create_dir(&staging)?;
        Ok(Transaction {
            store,
            _root_guard: guard,
            generation,
            namespaces,
            objects: BTreeMap::new(),
            staging,
            poisoned: false,
            working_databases: BTreeMap::new(),
            receipt: None,
        })
    }
    pub fn set_receipt(
        &mut self,
        principal: &str,
        key: &str,
        request_hash: &str,
        result: crate::records::ReceiptResult,
    ) -> Result<()> {
        if principal.is_empty() || key.is_empty() || key.len() > 256 {
            self.poisoned = true;
            return Err(Error::invalid(
                "receipt principal/key must be nonempty; key is limited to 256 bytes",
            ));
        }
        if serde_json::to_vec(&result)?.len() > sql::MAX_RESULT {
            self.poisoned = true;
            return Err(Error::limit("receipt exceeds 1 MiB"));
        }
        self.receipt = Some((
            principal.into(),
            key.into(),
            Receipt {
                request_hash: request_hash.into(),
                result,
            },
        ));
        Ok(())
    }
    /// A failed operation permanently aborts this root transaction, including caught errors.
    fn operation<T>(&mut self, action: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.poisoned {
            return Err(Error::new(
                crate::ErrorCode::TransactionAborted,
                "a prior operation failed",
            ));
        }
        let result = action(self);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn commit(mut self) -> Result<crate::records::CommitOutcome> {
        if self.poisoned {
            return Err(Error::new(
                crate::ErrorCode::TransactionAborted,
                "a prior operation failed; no changes were published",
            ));
        }
        self.seal_working_databases()?;
        let changed = self.namespaces.values().any(|n| n.dirty);
        if !changed && self.receipt.is_none() {
            return Ok(crate::records::CommitOutcome::Published(
                crate::records::CommitMetadata {
                    generation: self.generation,
                    revisions: BTreeMap::new(),
                    changed: false,
                },
            ));
        }
        self.store.publish(
            self.generation,
            &self.namespaces,
            &self.objects,
            &self.receipt,
        )
    }
}

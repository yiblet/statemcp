//! Root invocation lifecycle, operation poisoning, and all-or-nothing publication.
pub(crate) mod databases;
mod domain;
mod files;
mod functions;
mod namespaces;
mod objects;

use crate::{Error, Receipt, Result, model::Namespace, sql};
use std::collections::{BTreeMap, BTreeSet};

/// Domain invocation. Any failed operation poisons publication. Dropping it
/// discards both catalog and resource database changes through the overlay.
pub struct Transaction {
    backing: state_store::Transaction,
    generation: i64,
    namespaces: BTreeMap<String, Namespace>,
    objects: BTreeMap<String, Vec<u8>>,
    poisoned: bool,
    changed_databases: BTreeSet<String>,
    receipt: Option<(String, String, Receipt)>,
}
impl Transaction {
    pub(crate) fn new(
        backing: state_store::Transaction,
        generation: i64,
        namespaces: BTreeMap<String, Namespace>,
    ) -> Self {
        Self {
            backing,
            generation,
            namespaces,
            objects: BTreeMap::new(),
            poisoned: false,
            changed_databases: BTreeSet::new(),
            receipt: None,
        }
    }
    pub fn set_receipt(
        &mut self,
        principal: &str,
        key: &str,
        request_hash: &str,
        result: state_contract::execution::ReceiptResult,
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
    pub fn commit(self) -> Result<state_contract::publication::CommitOutcome> {
        if self.poisoned {
            return Err(Error::new(
                crate::ErrorCode::TransactionAborted,
                "a prior operation failed; no changes were published",
            ));
        }
        let changed = self.namespaces.values().any(|n| n.dirty);
        if !changed && self.receipt.is_none() {
            return Ok(state_contract::publication::CommitOutcome::Published(
                state_contract::publication::CommitMetadata {
                    generation: self.generation,
                    revisions: BTreeMap::new(),
                    changed: false,
                },
            ));
        }
        let outcome = {
            let catalog = self.backing.connect(&crate::store::catalog::name()?)?;
            catalog.execute_batch("PRAGMA foreign_keys=ON")?;
            crate::store::catalog::publish(
                &catalog,
                self.generation,
                &self.namespaces,
                &self.objects,
                &self.receipt,
            )?
        };
        if matches!(
            outcome,
            state_contract::publication::CommitOutcome::Published(_)
        ) {
            self.backing.commit()?;
        }
        Ok(outcome)
    }
}

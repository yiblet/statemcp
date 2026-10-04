//! Root invocation lifecycle, dispatch poisoning, and all-or-nothing publication.
mod databases;
mod files;
mod functions;
mod namespaces;
mod objects;
mod snapshots;

use self::snapshots::WorkingDatabase;
use crate::{
    Arguments, Error, Operation, Receipt, Result, Store, Tool, identity::id, model::Namespace, sql,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf};

/// A consistent root invocation. Any failed dispatch poisons publication.
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
        result: Value,
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
    pub fn dispatch(&mut self, tool: &str, args: Value) -> Result<Value> {
        if self.poisoned {
            return Err(Error::new(
                "TRANSACTION_ABORTED",
                "a prior operation failed",
            ));
        }
        let result = tool
            .parse::<Tool>()
            .map_err(|_| {
                Error::new(
                    "UNSUPPORTED_FEATURE",
                    format!("storage does not handle {tool}"),
                )
            })
            .and_then(|tool| tool.operation(&args))
            .and_then(|operation| self.dispatch_operation(operation, args));
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn dispatch_operation(&mut self, operation: Operation, args: Value) -> Result<Value> {
        if self.poisoned {
            return Err(Error::new(
                "TRANSACTION_ABORTED",
                "a prior operation failed",
            ));
        }
        let result = (|| {
            let mut args = args;
            if let Some(action) = operation.action_name() {
                args.as_object_mut()
                    .ok_or_else(|| Error::invalid("arguments must be an object"))?
                    .insert("action".into(), json!(action));
            }
            let request = Arguments::parse(operation.tool(), args)?;
            self.dispatch_request(&request)
        })();

        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn dispatch_request(&mut self, request: &Arguments) -> Result<Value> {
        if self.poisoned {
            return Err(Error::new(
                "TRANSACTION_ABORTED",
                "a prior operation failed",
            ));
        }
        let result = match request {
            Arguments::Namespace(args) => self.namespace(args),
            Arguments::File(args) => self.file(args),
            Arguments::Database(args) => self.database(args),
            Arguments::Function(args) => self.function(args),
            Arguments::Call(_) | Arguments::Execute(_) | Arguments::Describe(_) => Err(Error::new(
                "UNSUPPORTED_FEATURE",
                format!(
                    "storage does not handle {}",
                    request.operation().tool().as_str()
                ),
            )),
        };
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn commit(mut self) -> Result<Value> {
        if self.poisoned {
            return Err(Error::new(
                "TRANSACTION_ABORTED",
                "a prior operation failed; no changes were published",
            ));
        }
        self.seal_working_databases()?;
        let changed = self.namespaces.values().any(|n| n.dirty);
        if !changed && self.receipt.is_none() {
            return Ok(json!({"generation":self.generation,"revisions":{},"changed":false}));
        }
        self.store.publish(
            self.generation,
            &self.namespaces,
            &self.objects,
            &self.receipt,
        )
    }
}

//! Namespace, virtual file, function, and SQL domain operations over overlay storage.
pub use state_contract::ErrorCode;
mod declarations;
mod description;
mod error;
mod identity;
mod model;
mod operations;
mod sql;
mod store;
mod transaction;
mod validation;

pub use error::{Error, Result};
pub use model::Receipt;
pub use operations::{
    DatabaseAction, FileAction, FunctionAction, NamespaceAction, Operation, Tool,
};
pub use store::{MaintenanceReport, Store};
pub use transaction::Transaction;

pub use validation::{PathComponent, VirtualPath};

pub use description::DescriptionChange;

pub use declarations::FunctionDeclaration;

pub use schemars::Schema;
pub mod inputs;
pub mod records;

#[cfg(test)]
mod tests;

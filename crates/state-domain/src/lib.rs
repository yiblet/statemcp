//! Namespace, virtual file, function, and SQL domain operations over overlay storage.
pub use state_contract::ErrorCode;
mod error;
mod identity;
mod model;
mod sql;
mod store;
mod transaction;
mod validation;

pub use error::{Error, Result};
pub use model::Receipt;
pub use store::{HistorySize, MaintenanceReport, Store};
pub use transaction::Transaction;

use state_contract::function::FunctionDeclaration;

#[cfg(test)]
mod tests;

#[cfg(test)]
use state_contract::operations::*;

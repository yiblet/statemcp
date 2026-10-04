//! Immutable namespace metadata and isolated, file-backed SQLite snapshots.
mod durability;
mod error;
mod grants;
mod identity;
mod model;
mod sql;
mod store;
mod transaction;
mod validation;

pub use error::{Error, Result};
pub use grants::{CallGrant, DatabaseAccess, DatabaseGrant, FileAccess, FileGrant, Grants};
pub use model::Receipt;
pub use store::{MaintenanceReport, Store};
pub use transaction::Transaction;

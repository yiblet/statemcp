//! Immutable namespace metadata and isolated, file-backed SQLite snapshots.
mod durability;
mod error;
mod identity;
mod model;
mod sql;
mod store;
mod transaction;
mod validation;

pub use error::{Error, Result};
pub use model::Receipt;
pub use store::Store;
pub use transaction::Transaction;

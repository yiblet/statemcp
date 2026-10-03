//! Immutable namespace metadata and isolated, file-backed SQLite snapshots.
mod sql;
mod store;

pub use store::{Error, Receipt, Result, Store, Transaction};

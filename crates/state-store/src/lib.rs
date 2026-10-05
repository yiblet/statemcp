//! Immutable namespace metadata and isolated, file-backed SQLite snapshots.
mod declarations;
mod durability;
mod error;
mod identity;
mod model;
mod operations;
mod requests;
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

pub use requests::{
    Arguments, CallRequest, DatabaseRequest, DescribeRequest, DiscoveryMode, ExecuteRequest,
    FileRequest, FunctionRequest, MigrationSource, NamespaceRequest,
};

pub use declarations::FunctionDeclaration;

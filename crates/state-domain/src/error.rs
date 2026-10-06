//! Stable storage error codes and conversions.
use crate::ErrorCode;
use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(crate::ErrorCode::InvalidArgument, message)
    }
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::new(crate::ErrorCode::LimitExceeded, message)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(crate::ErrorCode::IoError, e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::invalid(e.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        let code = match e.sqlite_error_code() {
            Some(
                rusqlite::ErrorCode::AuthorizationForStatementDenied
                | rusqlite::ErrorCode::PermissionDenied,
            ) => ErrorCode::PermissionDenied,
            Some(
                rusqlite::ErrorCode::OperationInterrupted
                | rusqlite::ErrorCode::TooBig
                | rusqlite::ErrorCode::DiskFull
                | rusqlite::ErrorCode::OutOfMemory,
            ) => ErrorCode::LimitExceeded,
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                crate::ErrorCode::Conflict
            }
            _ => ErrorCode::SqlError,
        };
        Self::new(code, e.to_string())
    }
}

impl From<state_migrations::Error> for Error {
    fn from(error: state_migrations::Error) -> Self {
        match error {
            state_migrations::Error::Sqlite(source) => source.into(),
            state_migrations::Error::Forward { migration, source } => {
                if let Some(error) = source.downcast_ref::<Self>() {
                    return error.clone();
                }
                Self::new(
                    ErrorCode::MigrationMismatch,
                    format!("migration {} failed: {source}", migration.name()),
                )
            }
            state_migrations::Error::HistoryMismatch => {
                Self::new(ErrorCode::MigrationMismatch, error.to_string())
            }
            state_migrations::Error::Definition(_) => Self::invalid(error.to_string()),
        }
    }
}
impl From<state_store::Error> for Error {
    fn from(error: state_store::Error) -> Self {
        use state_store::Error as StorageError;
        let code = match &error {
            StorageError::Busy | StorageError::ConnectionsOpen => ErrorCode::Conflict,
            StorageError::ReadOnly => ErrorCode::PermissionDenied,
            StorageError::InvalidName(_) => ErrorCode::InvalidArgument,
            StorageError::Io(_) | StorageError::RecoveryRequired(_) => ErrorCode::IoError,
            StorageError::Registration(_) | StorageError::Sqlite(_) => ErrorCode::SqlError,
            StorageError::CorruptJournal(_) => ErrorCode::CorruptStore,
            StorageError::Limit(_) => ErrorCode::LimitExceeded,
            StorageError::Aborted => ErrorCode::TransactionAborted,
            StorageError::Migration(_) => ErrorCode::MigrationMismatch,
        };
        match error {
            StorageError::Sqlite(source) => source.into(),
            StorageError::Migration(source) => source.into(),
            error => Self::new(code, error.to_string()),
        }
    }
}

//! Stable storage error codes and conversions.
use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
}
impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_ARGUMENT", message)
    }
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::new("LIMIT_EXCEEDED", message)
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
        Self::new("IO_ERROR", e.to_string())
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
            ) => "PERMISSION_DENIED",
            Some(
                rusqlite::ErrorCode::OperationInterrupted
                | rusqlite::ErrorCode::TooBig
                | rusqlite::ErrorCode::DiskFull
                | rusqlite::ErrorCode::OutOfMemory,
            ) => "LIMIT_EXCEEDED",
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                "CONFLICT"
            }
            _ => "SQL_ERROR",
        };
        Self::new(code, e.to_string())
    }
}

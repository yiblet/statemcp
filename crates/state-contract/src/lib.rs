//! Typed capability contracts shared by services, runtimes, storage adapters, and transports.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    AbiMismatch,
    AlreadyExists,
    Busy,
    Conflict,
    CorruptStore,
    IdempotencyMismatch,
    InvalidArgument,
    IoError,
    LimitExceeded,
    MigrationMismatch,
    NotFound,
    PermissionDenied,
    PythonError,
    SchemaValidation,
    SqlError,
    TransactionAborted,
    UnsupportedFeature,
    WorkerFailed,
}
impl ErrorCode {
    /// Stable wire representation of this error category.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AbiMismatch => "ABI_MISMATCH",
            Self::AlreadyExists => "ALREADY_EXISTS",
            Self::Busy => "BUSY",
            Self::Conflict => "CONFLICT",
            Self::CorruptStore => "CORRUPT_STORE",
            Self::IdempotencyMismatch => "IDEMPOTENCY_MISMATCH",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::IoError => "IO_ERROR",
            Self::LimitExceeded => "LIMIT_EXCEEDED",
            Self::MigrationMismatch => "MIGRATION_MISMATCH",
            Self::NotFound => "NOT_FOUND",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::PythonError => "PYTHON_ERROR",
            Self::SchemaValidation => "SCHEMA_VALIDATION",
            Self::SqlError => "SQL_ERROR",
            Self::TransactionAborted => "TRANSACTION_ABORTED",
            Self::UnsupportedFeature => "UNSUPPORTED_FEATURE",
            Self::WorkerFailed => "WORKER_FAILED",
        }
    }
}
impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub mod database;
pub mod discovery;
pub mod execution;
pub mod file;
pub mod function;
pub mod namespace;
pub mod operations;
mod path;
pub mod publication;
pub mod schema;
pub use path::{PathComponent, VirtualPath};

/// A malformed contract value, independent of storage or service implementations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub code: ErrorCode,
    pub message: String,
}
impl ParseError {
    /// Construct a structured error for a contract that cannot be parsed.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }
}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ParseError {}

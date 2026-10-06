//! Known result and publication envelopes. Only SQL cells and user-defined schemas are JSON.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub files: usize,
    pub databases: usize,
    pub functions: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceInfo {
    #[serde(flatten)]
    pub summary: NamespaceSummary,
    pub revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Namespaces {
    pub namespaces: Vec<NamespaceInfo>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceDeleted {
    pub deleted: bool,
    pub id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Committed<T> {
    #[serde(flatten)]
    pub result: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Directory,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub kind: FileKind,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntries {
    pub entries: Vec<FileEntry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileStat {
    File {
        path: String,
        kind: FileKind,
        size: usize,
        hash: String,
    },
    Directory {
        path: String,
        kind: FileKind,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileContents {
    Text {
        path: String,
        kind: FileKind,
        size: usize,
        text: String,
    },
    Binary {
        path: String,
        kind: FileKind,
        size: usize,
        base64: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileWritten {
    pub path: String,
    pub size: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileMoved {
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilesDeleted {
    pub deleted: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Migration {
    pub id: String,
    pub checksum: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub name: String,
    pub id: String,
    pub migrations: Vec<Migration>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Databases {
    pub databases: Vec<DatabaseInfo>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseCreated {
    pub name: String,
    pub created: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseDropped {
    pub dropped: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlCell>>,
    pub rows_affected: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexInspection {
    pub name: String,
    pub unique: i64,
    pub origin: String,
    pub partial: i64,
    pub columns: SqlResult,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TableInspection {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: SchemaObjectKind,
    pub sql: Option<String>,
    pub columns: SqlResult,
    pub indexes: Vec<IndexInspection>,
    pub foreign_keys: SqlResult,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaObjectKind {
    Table,
    View,
    Index,
    Trigger,
}
impl SchemaObjectKind {
    pub(crate) fn parse(value: &str) -> crate::Result<Self> {
        match value {
            "table" => Ok(Self::Table),
            "view" => Ok(Self::View),
            "index" => Ok(Self::Index),
            "trigger" => Ok(Self::Trigger),
            _ => Err(crate::Error::new(
                crate::ErrorCode::CorruptStore,
                "unknown SQLite schema object kind",
            )),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchemaInspection {
    pub schema: SqlResult,
    pub tables: Vec<TableInspection>,
    pub schema_fingerprint: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseInspection {
    pub name: String,
    pub id: String,
    pub snapshot: String,
    pub staged: bool,
    pub migrations: Vec<Migration>,
    #[serde(flatten)]
    pub schema: SchemaInspection,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Migrations {
    pub migrations: Vec<Migration>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationsApplied {
    pub applied: usize,
    pub migrations: Vec<Migration>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub name: String,
    pub file: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<crate::inputs::EndpointSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<crate::inputs::EndpointSchema>,
    pub version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Functions {
    pub functions: Vec<FunctionSummary>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionRemoved {
    pub removed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionPublished {
    pub name: String,
    pub version: String,
    pub published: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitMetadata {
    pub generation: i64,
    pub revisions: BTreeMap<String, String>,
    pub changed: bool,
}
#[derive(Clone, Debug)]
pub enum CommitOutcome {
    Published(CommitMetadata),
    Replayed(ReceiptResult),
}
impl CommitOutcome {
    pub fn revision(&self, namespace: &str) -> Option<String> {
        match self {
            Self::Published(commit) => commit.revisions.get(namespace).cloned(),
            Self::Replayed(_) => None,
        }
    }
}

/// SQLite has a fixed set of cell types; tagged values preserve binary data and large integers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum SqlCell {
    Null(()),
    Boolean(bool),
    Integer(i64),
    Unsigned(u64),
    Real(f64),
    Text(String),
    Binary(BinaryCell),
    LargeInteger(IntegerCell),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BinaryCell {
    #[serde(rename = "$base64")]
    pub base64: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegerCell {
    #[serde(rename = "$integer")]
    pub integer: String,
}
impl SqlCell {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(integer) => Some(*integer),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub value: Value,
    pub stdout: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReceiptResult {
    Function(Value),
    Script(ExecutionResult),
}

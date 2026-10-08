//! Shared database contracts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MigrationInput {
    pub id: String,
    pub sql: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbListInput {
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbCreateInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbDropInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbQueryInput {
    pub database: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<crate::database::SqlCell>")]
    pub params: Option<Vec<crate::database::SqlCell>>,
    pub sql: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbExecuteInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<crate::database::SqlCell>")]
    pub params: Option<Vec<crate::database::SqlCell>>,
    pub sql: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbInspectInput {
    pub database: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbMigrationsInput {
    pub database: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbMigrateInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub migrations: Vec<MigrationInput>,
    pub namespace: String,
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
    pub fn parse(value: &str) -> Result<Self, crate::ParseError> {
        match value {
            "table" => Ok(Self::Table),
            "view" => Ok(Self::View),
            "index" => Ok(Self::Index),
            "trigger" => Ok(Self::Trigger),
            _ => Err(crate::ParseError::new(
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

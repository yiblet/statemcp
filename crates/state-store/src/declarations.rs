//! Published functions have fixed metadata and dynamic JSON Schema contracts.
use crate::{
    CallGrant, DatabaseAccess, DatabaseGrant, Error, FileAccess, FileGrant, Grants, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "StoredDeclaration")]
pub struct FunctionDeclaration {
    pub name: String,
    pub file: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(flatten)]
    pub grants: Grants,
    pub database_ids: BTreeMap<String, String>,
    pub source: String,
    pub source_hash: String,
    pub abi_version: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
}

// Old manifests used permission maps. Translate them once while reading storage.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredDatabases {
    Entries(Vec<DatabaseGrant>),
    Legacy(BTreeMap<String, DatabaseAccess>),
}
impl Default for StoredDatabases {
    fn default() -> Self {
        Self::Entries(Vec::new())
    }
}
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredFiles {
    Entries(Vec<FileGrant>),
    Legacy(BTreeMap<String, FileAccess>),
}
impl Default for StoredFiles {
    fn default() -> Self {
        Self::Entries(Vec::new())
    }
}
#[derive(Deserialize)]
struct StoredDeclaration {
    name: String,
    file: String,
    symbol: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    input_schema: Option<Value>,
    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    databases: StoredDatabases,
    #[serde(default)]
    files: StoredFiles,
    #[serde(default)]
    calls: Vec<CallGrant>,
    database_ids: BTreeMap<String, String>,
    source: String,
    source_hash: String,
    abi_version: u32,
    version: String,
}
impl TryFrom<StoredDeclaration> for FunctionDeclaration {
    type Error = Error;
    fn try_from(value: StoredDeclaration) -> Result<Self> {
        let databases = match value.databases {
            StoredDatabases::Entries(entries) => entries,
            StoredDatabases::Legacy(map) => map
                .into_iter()
                .map(|(database, access)| DatabaseGrant { database, access })
                .collect(),
        };
        let files = match value.files {
            StoredFiles::Entries(entries) => entries,
            StoredFiles::Legacy(map) => map
                .into_iter()
                .map(|(path, access)| FileGrant { path, access })
                .collect(),
        };
        let mut grants = Grants {
            databases,
            files,
            calls: value.calls,
        };
        grants
            .normalize()
            .map_err(|error| Error::new("CORRUPT_STORE", error.message))?;
        Ok(Self {
            name: value.name,
            file: value.file,
            symbol: value.symbol,
            description: value.description,
            input_schema: value.input_schema,
            output_schema: value.output_schema,
            grants,
            database_ids: value.database_ids,
            source: value.source,
            source_hash: value.source_hash,
            abi_version: value.abi_version,
            version: value.version,
        })
    }
}

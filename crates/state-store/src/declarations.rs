//! Published functions have fixed metadata and dynamic JSON Schema contracts.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, String>,
    pub source: String,
    pub source_hash: String,
    pub abi_version: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
}

// Serde ignores removed grant fields in old manifests; pinned source and versions remain intact.

//! Published functions have fixed metadata and dynamic JSON Schema contracts.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
// Field order matches the established canonical declaration hash.
#[serde(deny_unknown_fields)]
pub struct FunctionDeclaration {
    pub abi_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<crate::inputs::EndpointSchema>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<crate::inputs::EndpointSchema>,
    pub source: String,
    pub source_hash: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
}

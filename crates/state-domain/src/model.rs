//! Persisted manifests and the mutable namespace view of a root invocation.
use crate::FunctionDeclaration;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub request_hash: String,
    pub result: crate::records::ReceiptResult,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) files: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) databases: BTreeMap<String, Database>,
    #[serde(default)]
    pub(crate) functions: BTreeMap<String, FunctionDeclaration>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Database {
    pub(crate) id: String,
    pub(crate) snapshot: String,
    #[serde(default)]
    pub(crate) migrations: Vec<Migration>,
}
pub(crate) use crate::records::Migration;
#[derive(Clone)]
pub(crate) struct Namespace {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) revision: String,
    pub(crate) deleted: bool,
    pub(crate) manifest: Manifest,
    pub(crate) dirty: bool,
}

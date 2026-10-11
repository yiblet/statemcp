//! Persisted manifests and the mutable namespace view of a root invocation.
use crate::FunctionDeclaration;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub request_hash: String,
    pub result: state_contract::execution::ReceiptResult,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Manifest {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) routes: BTreeMap<String, RouteRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) files: BTreeMap<String, crate::content::ContentHash>,
    #[serde(default)]
    pub(crate) databases: BTreeMap<String, Database>,
    #[serde(default)]
    pub(crate) functions: BTreeMap<String, FunctionRecord>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Database {
    pub(crate) id: String,
    pub(crate) snapshot: String,
    #[serde(default)]
    pub(crate) migrations: Vec<Migration>,
}
pub(crate) use state_contract::database::Migration;
#[derive(Clone)]
pub(crate) struct Namespace {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) revision: String,
    pub(crate) deleted: bool,
    pub(crate) manifest: Manifest,
    pub(crate) dirty: bool,
}

/// A publication is either newly validated source or persisted content addresses.
/// Loading one declaration never expands the other declarations in its manifest.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum FunctionRecord {
    Loaded(FunctionDeclaration),
    Stored(crate::store::records::StoredFunction),
}
impl From<FunctionDeclaration> for FunctionRecord {
    fn from(value: FunctionDeclaration) -> Self {
        Self::Loaded(value)
    }
}
impl FunctionRecord {
    pub(crate) fn version(&self) -> &str {
        match self {
            Self::Loaded(v) => &v.version,
            Self::Stored(v) => &v.version,
        }
    }
    pub(crate) fn summary(&self) -> state_contract::function::FunctionSummary {
        match self {
            Self::Loaded(v) => state_contract::function::FunctionSummary {
                name: v.name.clone(),
                file: v.file.clone(),
                symbol: v.symbol.clone(),
                description: v.description.clone(),
                input_schema: v.input_schema.clone(),
                output_schema: v.output_schema.clone(),
                version: v.version.clone(),
            },
            Self::Stored(v) => v.summary(),
        }
    }
    pub(crate) fn resolve(
        &self,
        resolver: &mut crate::store::records::SourceResolver<'_>,
    ) -> crate::Result<FunctionDeclaration> {
        match self {
            Self::Loaded(v) => Ok(v.clone()),
            Self::Stored(v) => v.clone().resolve(resolver),
        }
    }
    pub(crate) fn capture(
        &self,
        conn: &rusqlite::Connection,
    ) -> crate::Result<crate::store::records::StoredFunction> {
        match self {
            Self::Loaded(v) => crate::store::records::StoredFunction::capture(conn, v),
            Self::Stored(v) => Ok(v.clone()),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum RouteRecord {
    Loaded(state_contract::http::Route),
    Stored(crate::store::records::StoredRoute),
}
impl From<state_contract::http::Route> for RouteRecord {
    fn from(value: state_contract::http::Route) -> Self {
        Self::Loaded(value)
    }
}
impl RouteRecord {
    pub(crate) fn metadata(&self, namespace: &str, id: &str) -> state_contract::http::ApiPublished {
        match self {
            Self::Loaded(v) => v.metadata(namespace, id),
            Self::Stored(v) => v.metadata(namespace, id),
        }
    }
    pub(crate) fn resolve(
        &self,
        resolver: &mut crate::store::records::SourceResolver<'_>,
    ) -> crate::Result<state_contract::http::Route> {
        match self {
            Self::Loaded(v) => Ok(v.clone()),
            Self::Stored(v) => v.clone().resolve(resolver),
        }
    }
    pub(crate) fn capture(
        &self,
        conn: &rusqlite::Connection,
    ) -> crate::Result<crate::store::records::StoredRoute> {
        match self {
            Self::Loaded(v) => crate::store::records::StoredRoute::capture(conn, v),
            Self::Stored(v) => Ok(v.clone()),
        }
    }
}

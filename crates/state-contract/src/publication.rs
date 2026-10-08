//! Shared publication contracts.
use crate::execution::ReceiptResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Committed<T> {
    #[serde(flatten)]
    pub result: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
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

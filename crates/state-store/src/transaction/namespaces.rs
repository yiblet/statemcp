//! Namespace identity, lookup, revision guards, rename, and cheap copy.
use super::Transaction;
use crate::{Error, Result};

impl Transaction {
    /// Resolve a namespace selector against this transaction's current facts.
    pub fn namespace_identity(&self, selector: &str) -> Result<&str> {
        self.namespaces
            .get(selector)
            .filter(|n| !n.deleted)
            .or_else(|| {
                self.namespaces
                    .values()
                    .find(|n| !n.deleted && n.name == selector)
            })
            .map(|n| n.id.as_str())
            .ok_or_else(|| {
                Error::new(
                    crate::ErrorCode::NotFound,
                    format!("namespace {selector} does not exist"),
                )
            })
    }
    pub(super) fn selected(&self, namespace: &str) -> Result<String> {
        self.namespace_identity(namespace).map(str::to_owned)
    }
    pub(super) fn check_revision(&self, ns: &str, expected: Option<&str>) -> Result<()> {
        if let Some(expected) = expected
            && expected != self.staged_namespace(ns)?.revision
        {
            return Err(Error::new(
                crate::ErrorCode::Conflict,
                "namespace revision does not match",
            ));
        }
        Ok(())
    }
    pub(super) fn available(&self, name: &str) -> Result<()> {
        if self
            .namespaces
            .values()
            .any(|n| n.name == name || n.id == name)
        {
            Err(Error::new(
                crate::ErrorCode::AlreadyExists,
                format!("namespace name {name} is reserved"),
            ))
        } else {
            Ok(())
        }
    }
}

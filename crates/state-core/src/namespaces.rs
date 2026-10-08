//! Typed namespaces operations, shared by controllers and runtime hosts.
use crate::{Context, Result};
use state_contract::namespace::*;

/// Namespaces capability; transaction state belongs to the supplied context.
#[derive(Clone, Copy, Default)]
pub struct Namespaces;
impl Namespaces {
    /// List namespaces with their names, descriptions, stable IDs, revisions, and resource counts.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn list_namespaces(
        &self,
        _input: NamespaceListInput,
        context: &mut Context,
    ) -> Result<state_contract::namespace::Namespaces> {
        context.operation(|context| {
            context.require_owner()?;
            Ok(context.transaction().list_namespaces()?)
        })
    }
    /// Get a namespace’s name, description, stable ID, revision, and resource counts.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn get_namespace(
        &self,
        mut input: state_contract::namespace::NamespaceGetInput,
        context: &mut Context,
    ) -> Result<NamespaceInfo> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().get_namespace(&input)?)
        })
    }
    /// Create an empty namespace with an optional description for files, SQLite databases, and published functions.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn create_namespace(
        &self,
        input: state_contract::namespace::NamespaceCreateInput,
        context: &mut Context,
    ) -> Result<NamespaceSummary> {
        context.operation(|context| {
            context.require_owner()?;
            Ok(context.transaction().create_namespace(&input)?)
        })
    }
    /// Update a namespace name or description while preserving its stable ID and contents. Omitted description is preserved; null clears it.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn update_namespace(
        &self,
        mut input: state_contract::namespace::NamespaceUpdateInput,
        context: &mut Context,
    ) -> Result<NamespaceSummary> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().update_namespace(&input)?)
        })
    }
    /// Copy a namespace and its contents to a new name. Subsequent changes are independent.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn copy_namespace(
        &self,
        input: state_contract::namespace::NamespaceCopyInput,
        context: &mut Context,
    ) -> Result<NamespaceSummary> {
        context.operation(|context| {
            context.require_owner()?;
            Ok(context.transaction().copy_namespace(&input)?)
        })
    }
    /// Delete a namespace and its contents from active use. Its name remains reserved.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn delete_namespace(
        &self,
        mut input: state_contract::namespace::NamespaceDeleteInput,
        context: &mut Context,
    ) -> Result<NamespaceDeleted> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().delete_namespace(&input)?)
        })
    }
}

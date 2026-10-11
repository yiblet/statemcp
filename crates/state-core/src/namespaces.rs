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
    /// List retained revisions newest first, including named pins.
    pub fn namespace_history(
        &self,
        mut input: NamespaceGetInput,
        context: &mut Context,
    ) -> Result<NamespaceHistory> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().namespace_history(&input)?)
        })
    }

    /// Pin a retained namespace revision with an optional unique name. At most seven pins; unpin one before adding an eighth.
    pub fn pin_revision(
        &self,
        mut input: NamespacePinInput,
        context: &mut Context,
    ) -> Result<RevisionInfo> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().pin_revision(&input)?)
        })
    }

    /// Unpin a retained revision. Content outside recent history is collected at commit.
    pub fn unpin_revision(
        &self,
        mut input: NamespaceRevisionInput,
        context: &mut Context,
    ) -> Result<RevisionInfo> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().unpin_revision(&input)?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ToolCalls;
    use serde_json::json;
    use state_contract::execution::ExecuteInput;

    #[test]
    fn python_pin_helpers_preserve_names_and_enforce_function_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let fs = state_fs::MemoryFs::new();
        let state = crate::test_support::memory_controller(&fs)?;
        state.tool("namespace.create", json!({"name":"n"}))?;
        state.tool("namespace.create", json!({"name":"other"}))?;
        let current = state
            .get_namespace(state_contract::namespace::NamespaceGetInput {
                namespace: "n".into(),
            })?
            .revision;
        let result = state.execute_script(ExecuteInput {
            namespace: Some("n".into()), inputs: Some(json!({"revision": current})), idempotency_key: None,
            script: "mcp('namespace.pin', {'namespace': 'n', 'revision': inputs['revision'], 'name': 'release'})".into(),
        }, None)?;
        assert_eq!(
            result.value.get("retention").and_then(|v| v.get("name")),
            Some(&json!("release"))
        );
        state.tool("fs.write", json!({"namespace":"n","path":"/api.py","text":"def endpoint():\n    return mcp('namespace.history', {'namespace': 'other'})\n"}))?;
        state.tool(
            "function.declare",
            json!({"namespace":"n","file":"/api.py","symbol":"endpoint","name":"escape"}),
        )?;
        let error = state.tool("call", json!({"namespace":"n","function":"escape"}));
        assert_eq!(
            error.err().map(|error| error.code),
            Some(crate::ErrorCode::PermissionDenied)
        );
        state.execute_script(
            ExecuteInput {
                namespace: Some("n".into()),
                inputs: Some(json!({"revision": current})),
                idempotency_key: None,
                script:
                    "mcp('namespace.unpin', {'namespace': 'n', 'revision': inputs['revision']})"
                        .into(),
            },
            None,
        )?;
        Ok(())
    }
}

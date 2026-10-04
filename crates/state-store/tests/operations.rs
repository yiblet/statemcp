use serde_json::json;
use state_store::{
    DatabaseAction, FileAction, FunctionAction, NamespaceAction, Operation, Store, Tool,
};

#[test]
fn public_names_and_legacy_aliases_keep_the_same_surface() {
    let operations: Vec<_> = NamespaceAction::ALL
        .iter()
        .copied()
        .map(Operation::Namespace)
        .chain(FileAction::ALL.iter().copied().map(Operation::File))
        .chain(DatabaseAction::ALL.iter().copied().map(Operation::Database))
        .chain(FunctionAction::ALL.iter().copied().map(Operation::Function))
        .chain([Operation::Call, Operation::Execute, Operation::Describe])
        .collect();
    assert_eq!(operations.len(), 30);
    for operation in operations {
        assert_eq!(
            Operation::from_public_name(&operation.public_name()).unwrap(),
            operation
        );
        let tool: Tool = operation.tool().as_str().parse().unwrap();
        assert_eq!(
            tool.with_action(operation.action_name()).unwrap(),
            operation
        );
    }
    assert_eq!(
        Tool::Namespace.with_action(Some("rename")).unwrap(),
        Operation::Namespace(NamespaceAction::Update)
    );
    for name in [
        "namespace.rename",
        "namespace",
        "db",
        "fs.unknown",
        "call.extra",
        "execute.extra",
        "describe.extra",
        "unknown",
    ] {
        assert!(Operation::from_public_name(name).is_err(), "{name}");
    }
}

#[test]
fn invalid_boundary_names_poison_staged_changes() {
    for (tool, args) in [
        ("unknown", json!({})),
        ("state_fs", json!({"action":"unknown"})),
        ("state_db", json!({})),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut tx = store.begin().unwrap();
        tx.dispatch_operation(
            Operation::Namespace(NamespaceAction::Create),
            json!({"name":"staged"}),
        )
        .unwrap();
        assert!(tx.dispatch(tool, args).is_err());
        assert_eq!(tx.commit().unwrap_err().code, "TRANSACTION_ABORTED");
        let mut tx = store.begin().unwrap();
        let result = tx
            .dispatch_operation(Operation::Namespace(NamespaceAction::List), json!({}))
            .unwrap();
        assert!(result["namespaces"].as_array().unwrap().is_empty());
    }
}

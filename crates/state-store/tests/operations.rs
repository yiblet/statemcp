use state_store::{DatabaseAction, FileAction, FunctionAction, NamespaceAction, Operation, Tool};

#[test]
fn public_names_are_exact_and_aliases_are_rejected() {
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
    assert!(Tool::Namespace.with_action(Some("rename")).is_err());
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

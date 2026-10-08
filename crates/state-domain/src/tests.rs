use crate::*;
use state_contract::VirtualPath;
use state_contract::{database::*, execution::*, namespace::*, publication::*};
use state_store::fs::{FileSystem, MemoryFs};
use std::path::Path;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
fn repository(fs: &MemoryFs) -> Result<Store> {
    Store::from_storage(
        state_store::Store::builder()
            .with_fs(fs.clone())
            .open("/state")?,
    )
}
fn namespace(transaction: &mut Transaction, name: &str) -> Result<()> {
    transaction.create_namespace(&NamespaceCreateInput {
        name: name.into(),
        description: DescriptionChange::Unchanged,
    })?;
    Ok(())
}
fn execute(transaction: &mut Transaction, namespace: &str, sql: &str) -> Result<()> {
    transaction.execute_database(&DbExecuteInput {
        namespace: namespace.into(),
        database: "app".into(),
        sql: sql.into(),
        params: None,
        expected_revision: None,
    })?;
    Ok(())
}
fn rows(transaction: &mut Transaction, namespace: &str, sql: &str) -> Result<Vec<Vec<SqlCell>>> {
    Ok(transaction
        .query_database(&DbQueryInput {
            namespace: namespace.into(),
            database: "app".into(),
            sql: sql.into(),
            params: None,
        })?
        .rows)
}
fn seed(store: &Store) -> Result<()> {
    let mut transaction = store.begin()?;
    namespace(&mut transaction, "source")?;
    transaction.create_database(&DbCreateInput {
        namespace: "source".into(),
        database: "app".into(),
        expected_revision: None,
    })?;
    execute(&mut transaction, "source", "CREATE TABLE t(x)")?;
    execute(&mut transaction, "source", "INSERT INTO t VALUES(1)")?;
    transaction.commit()?;
    Ok(())
}

#[test]
fn copies_never_share_database_files_and_include_pending_writes() -> TestResult {
    let fs = MemoryFs::new();
    let store = repository(&fs)?;
    seed(&store)?;
    let mut transaction = store.begin()?;
    execute(&mut transaction, "source", "UPDATE t SET x=2")?;
    transaction.copy_namespace(&NamespaceCopyInput {
        namespace: "source".into(),
        name: "copy".into(),
        description: DescriptionChange::Unchanged,
        expected_revision: None,
    })?;
    execute(&mut transaction, "source", "UPDATE t SET x=3")?;
    let source = transaction.inspect_database(&DbInspectInput {
        namespace: "source".into(),
        database: "app".into(),
    })?;
    let copy = transaction.inspect_database(&DbInspectInput {
        namespace: "copy".into(),
        database: "app".into(),
    })?;
    assert_ne!(source.id, copy.id);
    assert_ne!(source.snapshot, copy.snapshot);
    assert_eq!(
        rows(&mut transaction, "copy", "SELECT x FROM t")?,
        vec![vec![SqlCell::Integer(2)]]
    );
    transaction.commit()?;
    let mut reopened = repository(&fs)?.begin()?;
    assert_eq!(
        rows(&mut reopened, "source", "SELECT x FROM t")?,
        vec![vec![SqlCell::Integer(3)]]
    );
    assert_eq!(
        rows(&mut reopened, "copy", "SELECT x FROM t")?,
        vec![vec![SqlCell::Integer(2)]]
    );
    // Domain storage establishes no staging or snapshot directories.
    assert!(
        matches!(fs.metadata(Path::new("/state/staging")), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    );
    assert!(
        matches!(fs.metadata(Path::new("/state/snapshots")), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    );
    Ok(())
}

#[test]
fn caught_sql_error_aborts_catalog_database_and_receipt_changes() -> TestResult {
    let fs = MemoryFs::new();
    let store = repository(&fs)?;
    seed(&store)?;
    let mut transaction = store.begin()?;
    namespace(&mut transaction, "discarded")?;
    execute(&mut transaction, "source", "UPDATE t SET x=2")?;
    transaction.set_receipt(
        "owner",
        "failure",
        "request",
        ReceiptResult::Function(serde_json::json!(2)),
    )?;
    assert!(execute(&mut transaction, "source", "SELECT * FROM missing").is_err());
    assert!(matches!(
        transaction.commit(),
        Err(Error {
            code: ErrorCode::TransactionAborted,
            ..
        })
    ));
    assert!(store.receipt("owner", "failure")?.is_none());
    let mut transaction = store.begin()?;
    assert_eq!(
        rows(&mut transaction, "source", "SELECT x FROM t")?,
        vec![vec![SqlCell::Integer(1)]]
    );
    assert!(transaction.namespace_identity("discarded").is_err());
    Ok(())
}

#[test]
fn retained_connections_preserve_temp_tables_and_sql_policy() -> TestResult {
    let fs = MemoryFs::new();
    let store = repository(&fs)?;
    seed(&store)?;
    let mut transaction = store.begin()?;
    execute(&mut transaction, "source", "CREATE TEMP TABLE scratch(x)")?;
    execute(&mut transaction, "source", "INSERT INTO scratch VALUES(42)")?;
    assert_eq!(
        rows(&mut transaction, "source", "SELECT x FROM scratch")?,
        vec![vec![SqlCell::Integer(42)]]
    );
    assert!(execute(&mut transaction, "source", "COMMIT").is_err());
    assert!(transaction.commit().is_err());
    let mut transaction = store.begin()?;
    assert!(rows(&mut transaction, "source", "SELECT * FROM scratch").is_err());
    Ok(())
}

#[test]
fn replay_discards_new_database_changes_and_maintenance_retains_live_objects() -> TestResult {
    let fs = MemoryFs::new();
    let store = repository(&fs)?;
    seed(&store)?;
    let mut transaction = store.begin()?;
    transaction.set_receipt(
        "owner",
        "key",
        "request",
        ReceiptResult::Function(serde_json::json!(1)),
    )?;
    transaction.commit()?;
    let mut transaction = store.begin()?;
    execute(&mut transaction, "source", "UPDATE t SET x=2")?;
    transaction.set_receipt(
        "owner",
        "key",
        "request",
        ReceiptResult::Function(serde_json::json!(2)),
    )?;
    assert!(matches!(transaction.commit()?, CommitOutcome::Replayed(_)));
    let mut transaction = store.begin()?;
    assert_eq!(
        rows(&mut transaction, "source", "SELECT x FROM t")?,
        vec![vec![SqlCell::Integer(1)]]
    );
    drop(transaction);
    assert_eq!(store.maintenance(0)?.receipts_removed, 1);
    assert!(store.receipt("owner", "key")?.is_none());
    Ok(())
}

#[test]
fn database_writes_change_revision_and_missing_files_are_not_recreated() -> TestResult {
    let fs = MemoryFs::new();
    let store = repository(&fs)?;
    seed(&store)?;
    let mut transaction = store.begin()?;
    let before = transaction
        .get_namespace(&NamespaceGetInput {
            namespace: "source".into(),
        })?
        .revision;
    let database = transaction.inspect_database(&DbInspectInput {
        namespace: "source".into(),
        database: "app".into(),
    })?;
    execute(&mut transaction, "source", "UPDATE t SET x=2")?;
    transaction.commit()?;
    let mut transaction = store.begin()?;
    let after = transaction
        .get_namespace(&NamespaceGetInput {
            namespace: "source".into(),
        })?
        .revision;
    assert_ne!(before, after);
    drop(transaction);
    let backing = state_store::Store::builder()
        .with_fs(fs.clone())
        .open("/state")?;
    let transaction = backing.begin()?;
    let filename = state_store::DatabaseName::parse(format!("{}.sqlite", database.snapshot))?;
    transaction.delete_database(&filename)?;
    transaction.commit()?;
    let mut transaction = store.begin()?;
    assert!(matches!(
        rows(&mut transaction, "source", "SELECT x FROM t"),
        Err(Error {
            code: ErrorCode::CorruptStore,
            ..
        })
    ));
    assert!(
        matches!(fs.metadata(&Path::new("/state").join(filename.as_str())), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    );
    Ok(())
}

#[test]
fn persisted_database_sharing_is_rejected_before_operations() -> TestResult {
    let fs = MemoryFs::new();
    let store = repository(&fs)?;
    seed(&store)?;
    let mut transaction = store.begin()?;
    transaction.copy_namespace(&NamespaceCopyInput {
        namespace: "source".into(),
        name: "copy".into(),
        description: DescriptionChange::Unchanged,
        expected_revision: None,
    })?;
    transaction.commit()?;
    let backing = state_store::Store::builder().with_fs(fs).open("/state")?;
    let transaction = backing.begin()?;
    {
        let catalog = transaction.connect(&crate::store::catalog::name()?)?;
        // Corrupt the stored record to simulate an invalid external catalog.
        catalog.execute("UPDATE revisions SET manifest=(SELECT r.manifest FROM revisions r JOIN namespaces n ON n.head_revision=r.id WHERE n.name='source') WHERE id=(SELECT head_revision FROM namespaces WHERE name='copy')", [])?;
    }
    transaction.commit()?;
    assert!(matches!(
        store.begin(),
        Err(Error {
            code: ErrorCode::CorruptStore,
            ..
        })
    ));
    Ok(())
}

#[test]
fn public_operation_names_and_virtual_path_boundaries_are_preserved() -> TestResult {
    let operations = NamespaceAction::ALL
        .iter()
        .copied()
        .map(Operation::Namespace)
        .chain(FileAction::ALL.iter().copied().map(Operation::File))
        .chain(DatabaseAction::ALL.iter().copied().map(Operation::Database))
        .chain(FunctionAction::ALL.iter().copied().map(Operation::Function))
        .chain([Operation::Call, Operation::Execute, Operation::Describe]);
    for operation in operations {
        assert_eq!(
            Operation::from_public_name(&operation.public_name())?,
            operation
        );
        let tool: Tool = operation.tool().as_str().parse()?;
        assert_eq!(tool.with_action(operation.action_name())?, operation);
    }
    for name in [
        "namespace.rename",
        "namespace",
        "db",
        "call.extra",
        "unknown",
    ] {
        assert!(Operation::from_public_name(name).is_err());
    }
    assert_eq!(VirtualPath::parse("/a//./b/")?.as_str(), "/a/b");
    for path in ["relative", "/a/../b", "/a\\b", "/a\0b"] {
        assert!(VirtualPath::parse(path).is_err());
    }
    let prefix = VirtualPath::parse("/notes")?;
    assert!(prefix.contains(&VirtualPath::parse("/notes/file")?));
    assert!(!prefix.contains(&VirtualPath::parse("/notes-other/file")?));
    Ok(())
}

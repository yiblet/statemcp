//! Observable retention, shared content lifetime, and rollback checks.
use super::*;
use crate::{Result, identity::hash};
use state_contract::{file::*, namespace::*};
use state_store::{DatabaseName, fs::MemoryFs};

fn repository(history: usize) -> Result<Store> {
    let backing = state_store::Store::builder()
        .with_fs(MemoryFs::new())
        .open("/state")?;
    let store = Store::from_storage_with_history_size(backing, HistorySize::new(history)?)?;
    let mut tx = store.begin()?;
    tx.create_namespace(&NamespaceCreateInput {
        name: "ns".into(),
        description: DescriptionChange::Unchanged,
    })?;
    tx.commit()?;
    Ok(store)
}
fn write(store: &Store, namespace: &str, path: &str, text: &str) -> Result<()> {
    let mut tx = store.begin()?;
    tx.write_file(&FsWriteInput::Text(FileTextInput {
        namespace: namespace.into(),
        path: path.into(),
        text: text.into(),
        expected_revision: None,
    }))?;
    tx.commit()?;
    Ok(())
}
fn check_counts(store: &Store) -> Result<(i64, i64)> {
    let tx = store.backing.begin_read()?;
    let db = tx.connect(&DatabaseName::parse("catalog.sqlite")?)?;
    let mismatches: i64 = db.query_row("SELECT count(*) FROM objects o WHERE ref_count != coalesce((SELECT sum(uses) FROM revision_objects WHERE hash=o.hash),0)", [], |row| row.get(0))?;
    let dangling: i64 = db.query_row("SELECT count(*) FROM revision_objects r LEFT JOIN objects o ON r.hash=o.hash LEFT JOIN revisions v ON r.revision_id=v.id WHERE o.hash IS NULL OR v.id IS NULL", [], |row| row.get(0))?;
    assert_eq!(mismatches, 0);
    assert_eq!(dangling, 0);
    assert!(
        db.query_row(
            "SELECT count(*) FROM objects WHERE ref_count=0",
            [],
            |row| row.get::<_, i64>(0)
        )? == 0
    );
    Ok(db.query_row(
        "SELECT (SELECT count(*) FROM revisions),(SELECT count(*) FROM objects)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}
#[test]
fn retention_includes_current_and_preserves_historical_file_references() -> Result<()> {
    let store = repository(7)?;
    for i in 0..20 {
        write(&store, "ns", "/file", &format!("version-{i}"))?;
    }
    assert_eq!(check_counts(&store)?, (7, 7));
    let tx = store.backing.begin_read()?;
    let db = tx.connect(&catalog::name()?)?;
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM objects WHERE hash=?",
            [hash(b"version-12")],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM objects WHERE hash=?",
            [hash(b"version-13")],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    Ok(())
}
#[test]
fn copied_namespaces_share_bytes_until_the_last_owner_is_deleted() -> Result<()> {
    let store = repository(1)?;
    write(&store, "ns", "/a", "shared")?;
    write(&store, "ns", "/b", "shared")?;
    let mut tx = store.begin()?;
    tx.copy_namespace(&NamespaceCopyInput {
        namespace: "ns".into(),
        name: "copy".into(),
        description: DescriptionChange::Unchanged,
        expected_revision: None,
    })?;
    tx.commit()?;
    assert_eq!(check_counts(&store)?, (2, 1));
    for (namespace, expected_objects) in [("ns", 1), ("copy", 0)] {
        let mut tx = store.begin()?;
        tx.delete_namespace(&NamespaceDeleteInput {
            namespace: namespace.into(),
            expected_revision: None,
        })?;
        tx.commit()?;
        assert_eq!(check_counts(&store)?.1, expected_objects);
    }
    Ok(())
}
#[test]
fn failed_operations_do_not_change_reference_counts_or_history() -> Result<()> {
    let store = repository(1)?;
    write(&store, "ns", "/a", "old")?;
    let before = check_counts(&store)?;
    let mut tx = store.begin()?;
    tx.write_file(&FsWriteInput::Text(FileTextInput {
        namespace: "ns".into(),
        path: "/a".into(),
        text: "new".into(),
        expected_revision: None,
    }))?;
    assert!(
        tx.read_file(&FsReadInput {
            namespace: "missing".into(),
            path: "/a".into()
        })
        .is_err()
    );
    assert!(tx.commit().is_err());
    assert_eq!(check_counts(&store)?, before);
    let mut tx = store.begin()?;
    let contents = tx.read_file(&FsReadInput {
        namespace: "ns".into(),
        path: "/a".into(),
    })?;
    assert!(matches!(contents, FileContents::Text { text, .. } if text == "old"));
    Ok(())
}
#[test]
fn history_size_rejects_zero_and_maintenance_obeys_configuration() -> Result<()> {
    assert!(HistorySize::new(0).is_err());
    assert!("0".parse::<HistorySize>().is_err());
    assert_eq!(HistorySize::default().get(), 7);
    let mut store = repository(5)?;
    for i in 0..8 {
        write(&store, "ns", "/file", &i.to_string())?;
    }
    store.history_size = HistorySize::new(2)?;
    assert_eq!(store.maintenance(10000)?.revisions_removed, 3);
    assert_eq!(check_counts(&store)?, (2, 2));
    Ok(())
}

#[test]
fn deleting_source_files_preserves_published_functions_until_removed() -> Result<()> {
    use state_contract::function::{
        FunctionDeclareInput, FunctionGetInput, FunctionRemoveInput, FunctionSources,
    };
    let store = repository(1)?;
    let source = "def answer(): return 42";
    write(&store, "ns", "/api.py", source)?;
    let mut tx = store.begin()?;
    let published = tx.declare_function(
        &FunctionDeclareInput {
            description: None,
            expected_revision: None,
            expected_version: None,
            file: "/api.py".into(),
            input_schema: None,
            name: "answer".into(),
            namespace: "ns".into(),
            output_schema: None,
            symbol: "answer".into(),
        },
        FunctionSources {
            source: source.into(),
            modules: [("/api.py".into(), source.into())].into(),
        },
    )?;
    tx.commit()?;
    assert_eq!(check_counts(&store)?.1, 1);
    let mut tx = store.begin()?;
    tx.delete_file(&FsDeleteInput {
        namespace: "ns".into(),
        path: "/api.py".into(),
        expected_revision: None,
        recursive: None,
    })?;
    tx.commit()?;
    assert_eq!(check_counts(&store)?.1, 1);
    let mut tx = store.begin()?;
    let pinned = tx.get_function(&FunctionGetInput {
        namespace: "ns".into(),
        name: "answer".into(),
        expected_version: Some(published.version),
    })?;
    assert_eq!(pinned.source, source);
    tx.remove_function(&FunctionRemoveInput {
        namespace: "ns".into(),
        name: "answer".into(),
        expected_revision: None,
        expected_version: Some(pinned.version),
    })?;
    tx.commit()?;
    assert_eq!(check_counts(&store)?.1, 0);
    Ok(())
}

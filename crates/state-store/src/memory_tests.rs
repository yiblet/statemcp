//! Behavior tests run solely through an injected in-memory filesystem.
use super::*;
use fs::{FileSystem, MemoryFs, Metadata, OpenMode};
use std::{
    io::{self, Write},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[test]
fn memory_transactions_reopen_rollback_delete_and_enforce_locks() -> Result<()> {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/unit/state")?;
    let name = DatabaseName::parse("app.sqlite")?;
    let write = store.begin()?;
    write.connect(&name)?.execute_batch(
        "CREATE TABLE t(x); INSERT INTO t VALUES(1); BEGIN; INSERT INTO t VALUES(9); ROLLBACK;",
    )?;
    // Base files do not exist until publication.
    assert!(
        matches!(fs.metadata(Path::new("/unit/state/app.sqlite")), Err(e) if e.kind() == io::ErrorKind::NotFound)
    );
    write.commit()?;
    let same = Store::builder().with_fs(fs.clone()).open("/unit/state")?;
    let first = store.begin_read()?;
    let second = same.begin_read()?;
    assert!(matches!(store.begin(), Err(Error::Busy)));
    let connection = first.connect(&name)?;
    assert_eq!(
        connection.query_row("SELECT SUM(x) FROM t", [], |r| r.get::<_, i64>(0))?,
        1
    );
    assert!(connection.execute("INSERT INTO t VALUES(2)", []).is_err());
    drop(connection);
    first.finish()?;
    assert!(matches!(store.begin(), Err(Error::Busy)));
    second.finish()?;
    let write = store.begin()?;
    write.connect(&name)?.execute("UPDATE t SET x=2", [])?;
    write.rollback();
    let write = store.begin()?;
    assert!(write.delete_database(&name)?);
    write.rollback();
    let reader = same.begin_read()?;
    assert_eq!(
        reader
            .connect(&name)?
            .query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
        1
    );
    reader.finish()?;
    let write = store.begin()?;
    assert!(write.delete_database(&name)?);
    write.commit()?;
    let reader = same.begin_read()?;
    assert!(reader.connect(&name).is_err());
    reader.finish()?;
    Ok(())
}

struct Initialize {
    migration: state_migrations::Migration,
}
impl state_migrations::Migrator for Initialize {
    fn migration(&self) -> state_migrations::Migration {
        self.migration.clone()
    }
    fn forward(
        &self,
        context: &state_migrations::Context<'_, '_>,
    ) -> std::result::Result<(), state_migrations::ForwardError> {
        context
            .catalog()
            .execute_batch("CREATE TABLE example(value); INSERT INTO example VALUES(42);")?;
        // Retry-safe external file changes use the same injected filesystem.
        let mut marker = context
            .fs()
            .open(&context.root().join("marker"), OpenMode::Update)?;
        marker.set_len(0)?;
        marker.write_all(b"migrated")?;
        marker.sync_all()?;
        Ok(())
    }
}
#[test]
fn memory_migrations_use_injected_filesystem_and_persist_history()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let fs = MemoryFs::new();
    let service = state_migrations::Service::new(vec![Box::new(Initialize {
        migration: state_migrations::Migration::new("initialize", 1)?,
    })])?;
    let store = Store::builder()
        .with_fs(fs.clone())
        .open_with_migrations("/unit/state", &service)?;
    let reader = store.begin_read()?;
    let catalog = reader.connect(&DatabaseName::parse("catalog.sqlite")?)?;
    assert_eq!(
        catalog.query_row(
            "SELECT version FROM migrations WHERE name='initialize'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        1
    );
    drop(catalog);
    reader.finish()?;
    assert!(matches!(
        fs.metadata(Path::new("/unit/state/marker"))?,
        Metadata::File(8)
    ));
    Store::builder()
        .with_fs(fs)
        .open_with_migrations("/unit/state", &service)?;
    Ok(())
}

#[derive(Debug, Clone)]
enum FailurePoint {
    Prepare,
    Apply,
    InvalidDestination,
    Rename,
    DecisionSync,
    AppliedSync,
    Cleanup,
}

#[derive(Debug, Clone)]
struct FailingFs {
    fs: MemoryFs,
    point: FailurePoint,
    armed: Arc<AtomicBool>,
    syncs: Arc<AtomicUsize>,
}
impl FileSystem for FailingFs {
    fn prepare_root(&self, path: &Path) -> io::Result<PathBuf> {
        self.fs.prepare_root(path)
    }
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        if matches!(self.point, FailurePoint::InvalidDestination)
            && path.file_name().is_some_and(|name| name == "b.sqlite")
            && self.armed.load(Ordering::SeqCst)
        {
            return Ok(Metadata::Other);
        }
        self.fs.metadata(path)
    }
    fn open(&self, path: &Path, mode: OpenMode) -> io::Result<Box<dyn fs::File>> {
        let target = match self.point {
            FailurePoint::InvalidDestination
            | FailurePoint::Rename
            | FailurePoint::DecisionSync
            | FailurePoint::AppliedSync
            | FailurePoint::Cleanup => false,
            FailurePoint::Prepare => {
                matches!(mode, OpenMode::CreateNew)
                    && path
                        .file_name()
                        .is_some_and(|name| name == "overlay.pending")
            }
            FailurePoint::Apply => {
                matches!(mode, OpenMode::Update)
                    && path.file_name().is_some_and(|name| name == "b.sqlite")
            }
        };
        if target && self.armed.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("injected publication failure"));
        }
        self.fs.open(path, mode)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if matches!(self.point, FailurePoint::Rename) && self.armed.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("injected rename failure"));
        }
        self.fs.rename(from, to)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        if matches!(self.point, FailurePoint::Cleanup)
            && path.file_name().is_some_and(|name| name == "overlay.redo")
            && self.armed.swap(false, Ordering::SeqCst)
        {
            return Err(io::Error::other("injected cleanup failure"));
        }
        self.fs.remove_file(path)
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        if self.armed.load(Ordering::SeqCst)
            && matches!(
                self.point,
                FailurePoint::DecisionSync | FailurePoint::AppliedSync
            )
        {
            let count = self.syncs.fetch_add(1, Ordering::SeqCst) + 1;
            let target = match self.point {
                FailurePoint::DecisionSync => 1,
                FailurePoint::AppliedSync => 2,
                _ => 0,
            };
            if count == target && self.armed.swap(false, Ordering::SeqCst) {
                return Err(io::Error::other("injected directory sync failure"));
            }
        }
        self.fs.sync_dir(path)
    }
    fn lock(&self, path: &Path, mode: LockMode) -> io::Result<Box<dyn LockGuard>> {
        self.fs.lock(path, mode)
    }
}
#[test]
fn memory_recovery_replays_partially_applied_multifile_commit() -> Result<()> {
    let fs = MemoryFs::new();
    let armed = Arc::new(AtomicBool::new(false));
    let backend = FailingFs {
        point: FailurePoint::Apply,
        fs: fs.clone(),
        armed: Arc::clone(&armed),
        syncs: Arc::default(),
    };
    let store = Store::builder()
        .with_fs(backend.clone())
        .open("/unit/state")?;
    let transaction = store.begin()?;
    for name in ["a.sqlite", "b.sqlite"] {
        transaction
            .connect(&DatabaseName::parse(name)?)?
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1);")?;
    }
    transaction.commit()?;
    let transaction = store.begin()?;
    for name in ["a.sqlite", "b.sqlite"] {
        transaction
            .connect(&DatabaseName::parse(name)?)?
            .execute("UPDATE t SET x=2", [])?;
    }
    armed.store(true, Ordering::SeqCst);
    assert!(matches!(
        transaction.commit(),
        Err(Error::RecoveryRequired(_))
    ));
    assert!(fs.metadata(Path::new("/unit/state/overlay.redo")).is_ok());
    // Open runs the same replay path used by disk stores, before exposing data.
    let reopened = Store::builder().with_fs(backend).open("/unit/state")?;
    let reader = reopened.begin_read()?;
    for name in ["a.sqlite", "b.sqlite"] {
        assert_eq!(
            reader
                .connect(&DatabaseName::parse(name)?)?
                .query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
            2
        );
    }
    reader.finish()?;
    assert!(
        matches!(fs.metadata(Path::new("/unit/state/overlay.redo")), Err(e) if e.kind() == io::ErrorKind::NotFound)
    );
    Ok(())
}

#[test]
fn memory_recovery_discards_pending_and_rejects_corrupt_decision() -> Result<()> {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/unit/state")?;
    let pending = Path::new("/unit/state/overlay.pending");
    fs.open(pending, OpenMode::CreateNew)?
        .write_all(b"incomplete")?;
    store.begin_read()?.finish()?;
    assert!(matches!(fs.metadata(pending), Err(e) if e.kind() == io::ErrorKind::NotFound));
    let committed = Path::new("/unit/state/overlay.redo");
    fs.open(committed, OpenMode::CreateNew)?
        .write_all(b"corrupt")?;
    assert!(matches!(store.begin_read(), Err(Error::CorruptJournal(_))));
    assert!(fs.metadata(committed).is_ok());
    Ok(())
}

#[test]
fn memory_preparation_failure_preserves_committed_data() -> Result<()> {
    let armed = Arc::new(AtomicBool::new(false));
    let backend = FailingFs {
        fs: MemoryFs::new(),
        point: FailurePoint::Prepare,
        armed: Arc::clone(&armed),
        syncs: Arc::default(),
    };
    let store = Store::builder().with_fs(backend).open("/unit/state")?;
    let name = DatabaseName::parse("app.sqlite")?;
    let transaction = store.begin()?;
    transaction
        .connect(&name)?
        .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1);")?;
    transaction.commit()?;
    let transaction = store.begin()?;
    transaction
        .connect(&name)?
        .execute("UPDATE t SET x=2", [])?;
    armed.store(true, Ordering::SeqCst);
    assert!(matches!(transaction.commit(), Err(Error::Io(_))));
    let reader = store.begin_read()?;
    assert_eq!(
        reader
            .connect(&name)?
            .query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
        1
    );
    reader.finish()?;
    Ok(())
}

#[test]
fn recovery_checks_all_destinations_before_applying_and_read_begin_recovers() -> Result<()> {
    let fs = MemoryFs::new();
    let armed = Arc::new(AtomicBool::new(false));
    let backend = FailingFs {
        fs: fs.clone(),
        point: FailurePoint::InvalidDestination,
        armed: Arc::clone(&armed),
        syncs: Arc::default(),
    };
    let store = Store::builder().with_fs(backend).open("/state")?;
    let transaction = store.begin()?;
    for name in ["a.sqlite", "b.sqlite"] {
        transaction
            .connect(&DatabaseName::parse(name)?)?
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(42)")?;
    }
    armed.store(true, Ordering::SeqCst);
    assert!(matches!(
        transaction.commit(),
        Err(Error::RecoveryRequired(_))
    ));
    assert!(
        matches!(fs.metadata(Path::new("/state/a.sqlite")), Err(e) if e.kind() == io::ErrorKind::NotFound)
    );
    assert!(matches!(store.begin_read(), Err(Error::CorruptJournal(_))));
    armed.store(false, Ordering::SeqCst);
    let reader = store.begin_read()?;
    for name in ["a.sqlite", "b.sqlite"] {
        assert_eq!(
            reader
                .connect(&DatabaseName::parse(name)?)?
                .query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
            42
        );
    }
    reader.finish()?;
    Ok(())
}

#[test]
fn interrupted_publication_boundaries_recover_the_decided_state() -> Result<()> {
    for point in [
        FailurePoint::Rename,
        FailurePoint::DecisionSync,
        FailurePoint::AppliedSync,
        FailurePoint::Cleanup,
    ] {
        let armed = Arc::new(AtomicBool::new(false));
        let before_decision = matches!(point, FailurePoint::Rename);
        let backend = FailingFs {
            fs: MemoryFs::new(),
            point,
            armed: Arc::clone(&armed),
            syncs: Arc::default(),
        };
        let store = Store::builder().with_fs(backend).open("/state")?;
        let transaction = store.begin()?;
        for name in ["a.sqlite", "b.sqlite"] {
            transaction
                .connect(&DatabaseName::parse(name)?)?
                .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1)")?;
        }
        transaction.commit()?;
        let transaction = store.begin()?;
        for name in ["a.sqlite", "b.sqlite"] {
            transaction
                .connect(&DatabaseName::parse(name)?)?
                .execute("UPDATE t SET x=2", [])?;
        }
        armed.store(true, Ordering::SeqCst);
        let outcome = transaction.commit();
        if before_decision {
            assert!(matches!(outcome, Err(Error::Io(_))));
        } else {
            assert!(matches!(outcome, Err(Error::RecoveryRequired(_))));
        }
        // Recovery also runs on existing handles, before reads or another write.
        let recovered = store.begin()?;
        for name in ["a.sqlite", "b.sqlite"] {
            let connection = recovered.connect(&DatabaseName::parse(name)?)?;
            assert_eq!(
                connection.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
                if before_decision { 1 } else { 2 }
            );
            assert_eq!(
                connection.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))?,
                "ok"
            );
        }
        recovered.rollback();
        store.begin_read()?.finish()?;
    }
    Ok(())
}

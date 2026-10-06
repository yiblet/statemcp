use crate::fs::{FileSystem, MemoryFs, OpenMode};
use std::{io::Read, path::Path};

fn read(fs: &MemoryFs, path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs.open(path, OpenMode::Read)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn exists(fs: &MemoryFs, path: &Path) -> Result<bool> {
    match fs.metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
use crate::{DatabaseName, Error, Store};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn seed(store: &Store, name: &DatabaseName) -> Result {
    let tx = store.begin()?;
    tx.connect(name)?
        .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1)")?;
    tx.commit()?;
    Ok(())
}

#[test]
fn shared_readers_coexist_and_exclude_writers_until_all_finish() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let app = name("app.sqlite")?;
    seed(&store, &app)?;
    let first = store.begin_read()?;
    let independent = Store::builder().with_fs(fs.clone()).open("/state")?;
    let second = independent.begin_read()?;
    {
        let a = first.connect(&app)?;
        let b = second.connect(&app)?;
        for db in [&a, &b] {
            assert_eq!(
                db.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
                1
            );
        }
    }
    assert!(matches!(store.begin(), Err(Error::Busy)));
    first.finish()?;
    assert!(matches!(store.begin(), Err(Error::Busy)));
    second.finish()?;
    let writer = store.begin()?;
    assert!(matches!(store.begin_read(), Err(Error::Busy)));
    writer.connect(&app)?.execute_batch("UPDATE t SET x=2")?;
    writer.commit()?;
    let reader = store.begin_read()?;
    assert_eq!(
        reader
            .connect(&app)?
            .query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
        2
    );
    reader.finish()?;
    Ok(())
}

#[test]
fn read_invocations_cannot_write_create_or_reopen_with_write_flags() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let app = name("app.sqlite")?;
    seed(&store, &app)?;
    let original = read(&fs, &Path::new("/state").join(app.as_str()))?;
    let reader = store.begin_read()?;
    assert!(reader.connect(&name("missing.sqlite")?).is_err());
    {
        let db = reader.connect(&app)?;
        assert!(
            db.execute_batch("PRAGMA query_only=OFF; INSERT INTO t VALUES (2)")
                .is_err()
        );
        assert!(db.execute_batch("CREATE TABLE forbidden(x)").is_err());
        // VFS enforcement survives direct rusqlite use with write flags.
        let path = db.path().ok_or("connection has no path")?;
        assert!(
            rusqlite::Connection::open_with_flags_and_vfs(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                "state-sqlite-overlay-v1",
            )
            .is_err()
        );
        // Read query scratch space is still allowed and stays in memory.
        db.execute_batch("CREATE TEMP TABLE scratch(x); INSERT INTO scratch VALUES (42)")?;
        assert_eq!(
            db.query_row("SELECT x FROM scratch", [], |r| r.get::<_, i64>(0))?,
            42
        );
        assert_eq!(
            db.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))?,
            1
        );
    }
    reader.finish()?;
    assert_eq!(
        read(&fs, &Path::new("/state").join(app.as_str()))?,
        original
    );
    assert!(!exists(&fs, &Path::new("/state").join("missing.sqlite"))?);
    assert!(!exists(&fs, &Path::new("/state").join("overlay.pending"))?);
    assert!(!exists(&fs, &Path::new("/state").join("overlay.redo"))?);
    Ok(())
}

fn name(value: &str) -> Result<DatabaseName> {
    Ok(DatabaseName::parse(value)?)
}

#[test]
fn sqlite_commits_are_private_until_outer_commit_and_outer_abort_discards_them() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let app = name("app.sqlite")?;
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        db.execute_batch("CREATE TABLE items(value); INSERT INTO items VALUES (1);")?;
        assert_eq!(
            db.query_row("SELECT value FROM items", [], |r| r.get::<_, i64>(0))?,
            1
        );
        assert!(!exists(&fs, &Path::new("/state").join(app.as_str()))?);
    }
    tx.commit()?;
    let original = read(&fs, &Path::new("/state").join(app.as_str()))?;
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        db.execute_batch(
            "BEGIN; UPDATE items SET value=2; COMMIT; BEGIN; UPDATE items SET value=3; ROLLBACK;",
        )?;
        assert_eq!(
            db.query_row("SELECT value FROM items", [], |r| r.get::<_, i64>(0))?,
            2
        );
        assert_eq!(
            read(&fs, &Path::new("/state").join(app.as_str()))?,
            original
        );
    }
    tx.rollback();
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        assert_eq!(
            db.query_row("SELECT value FROM items", [], |r| r.get::<_, i64>(0))?,
            1
        );
    }
    tx.commit()?;
    Ok(())
}

#[test]
fn multi_database_commit_survives_reopen_and_uncommitted_sqlite_work_rolls_back() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let a = name("a.sqlite")?;
    let b = name("b.sqlite")?;
    let tx = store.begin()?;
    {
        let first = tx.connect(&a)?;
        let second = tx.connect(&b)?;
        first.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (10)")?;
        second.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (20)")?;
        first.execute_batch("BEGIN; INSERT INTO t VALUES (99)")?;
        // Closing first rolls back its open SQLite transaction, retaining 10.
    }
    tx.commit()?;
    let reopened = Store::builder().with_fs(fs.clone()).open("/state")?;
    let tx = reopened.begin()?;
    for (db_name, expected) in [(a, 10), (b, 20)] {
        let db = tx.connect(&db_name)?;
        assert_eq!(
            db.query_row("SELECT sum(x) FROM t", [], |r| r.get::<_, i64>(0))?,
            expected
        );
        assert_eq!(
            db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))?,
            "ok"
        );
    }
    tx.commit()?;
    Ok(())
}

#[test]
fn root_lock_excludes_independent_handles_and_connections_are_unique() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let other = store.clone();
    let tx = store.begin()?;
    assert!(matches!(other.begin(), Err(Error::Busy)));
    assert!(matches!(
        Store::builder().with_fs(fs.clone()).open("/state"),
        Err(Error::Busy)
    ));
    let app = name("app.sqlite")?;
    {
        let db = tx.connect(&app)?;
        assert!(tx.connect(&app).is_err());
        db.execute_batch("CREATE TABLE t(x)")?;
    }
    // Closing permits another connection within the same invocation.
    drop(tx.connect(&app)?);
    tx.rollback();
    other.begin()?.rollback();
    Ok(())
}

#[test]
fn small_update_materializes_only_dirty_blocks() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let app = name("app.sqlite")?;
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        db.execute_batch("CREATE TABLE payload(id INTEGER PRIMARY KEY, data BLOB); INSERT INTO payload VALUES(1, zeroblob(1048576)); CREATE TABLE counter(x); INSERT INTO counter VALUES (0)")?;
    }
    tx.commit()?;
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        assert_eq!(tx.memory_bytes()?, 0);
        db.execute_batch("UPDATE counter SET x=1")?;
    }
    assert!(tx.memory_bytes()? < 64 * 1024);
    tx.commit()?;
    Ok(())
}

#[test]
fn vacuum_truncation_and_memory_temp_tables_work() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let app = name("app.sqlite")?;
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        db.execute_batch("CREATE TABLE t(data); INSERT INTO t VALUES (zeroblob(32768)); CREATE TEMP TABLE scratch(x); INSERT INTO scratch VALUES (42)")?;
    }
    tx.commit()?;
    let before = fs.metadata(&Path::new("/state").join(app.as_str()))?.len();
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        db.execute_batch("DELETE FROM t; VACUUM")?;
    }
    tx.commit()?;
    assert!(fs.metadata(&Path::new("/state").join(app.as_str()))?.len() < before);
    let tx = store.begin()?;
    {
        let db = tx.connect(&app)?;
        assert_eq!(
            db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))?,
            "ok"
        );
    }
    tx.rollback();
    Ok(())
}

#[test]
fn invalid_names_and_external_sidecars_are_rejected() -> Result {
    for invalid in [
        "../app.sqlite",
        "/app.sqlite",
        ".sqlite",
        "app",
        "a/b.sqlite",
        "é.sqlite",
    ] {
        assert!(DatabaseName::parse(invalid).is_err());
    }
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    fs.open(Path::new("/state/app.sqlite"), OpenMode::CreateNew)?;
    fs.open(Path::new("/state/app.sqlite-wal"), OpenMode::CreateNew)?;
    let tx = store.begin()?;
    assert!(tx.connect(&name("app.sqlite")?).is_err());
    tx.rollback();
    Ok(())
}

#[test]
fn sqlite_clock_is_preserved_by_the_vfs() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let tx = store.begin()?;
    let app = name("app.sqlite")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    {
        let db = tx.connect(&app)?;
        let sqlite_now: i64 = db.query_row("SELECT unixepoch('now')", [], |r| r.get(0))?;
        assert!(
            (sqlite_now - now).abs() < 60,
            "SQLite clock: {sqlite_now}, system clock: {now}"
        );
    }
    tx.rollback();
    Ok(())
}

#[test]
fn deletion_and_recreation_publish_or_rollback_with_other_databases() -> Result {
    let fs = MemoryFs::new();
    let store = Store::builder().with_fs(fs.clone()).open("/state")?;
    let app = name("app.sqlite")?;
    let tx = store.begin()?;
    {
        tx.connect(&app)?
            .execute_batch("CREATE TABLE old(x); INSERT INTO old VALUES (1)")?;
    }
    tx.commit()?;
    let tx = store.begin()?;
    assert!(tx.delete_database(&app)?);
    assert!(!tx.delete_database(&app)?);
    tx.rollback();
    assert!(exists(&fs, &Path::new("/state").join(app.as_str()))?);
    let tx = store.begin()?;
    assert!(tx.delete_database(&app)?);
    {
        let db = tx.connect(&app)?;
        assert!(db.prepare("SELECT * FROM old").is_err());
        db.execute_batch("CREATE TABLE fresh(x); INSERT INTO fresh VALUES (2)")?;
    }
    tx.commit()?;
    let tx = store.begin()?;
    assert!(tx.delete_database(&app)?);
    tx.commit()?;
    assert!(!exists(&fs, &Path::new("/state").join(app.as_str()))?);
    Ok(())
}

#[test]
fn independent_roots_can_use_the_registered_vfs_concurrently() -> Result {
    let mut handles = Vec::new();
    for value in 0..4_i64 {
        handles.push(std::thread::spawn(move || -> crate::Result<()> {
            let fs = MemoryFs::new();
            let store = Store::builder().with_fs(fs.clone()).open("/state")?;
            let app = DatabaseName::parse("app.sqlite")?;
            for _ in 0..8 {
                let tx = store.begin()?;
                {
                    let db = tx.connect(&app)?;
                    db.execute_batch("CREATE TABLE IF NOT EXISTS t(x)")?;
                    db.execute("INSERT INTO t VALUES (?1)", [value])?;
                }
                tx.commit()?;
            }
            let tx = store.begin()?;
            {
                let db = tx.connect(&app)?;
                assert_eq!(
                    db.query_row("SELECT sum(x) FROM t", [], |r| r.get::<_, i64>(0))?,
                    value * 8
                );
            }
            tx.rollback();
            Ok(())
        }));
    }
    for handle in handles {
        handle.join().map_err(|_| "VFS thread panicked")??;
    }
    Ok(())
}

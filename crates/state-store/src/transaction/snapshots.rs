//! Staging connections, immutable SQLite snapshots, and durable sealing.
//! Connections close before publication; namespace copies freeze pending writes.
use super::Transaction;
use crate::{
    Error, Result,
    durability::sync_dir,
    identity::id,
    model::{Database, Manifest},
    sql,
};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::Duration,
};

// Use a separate internal connection: application connections deliberately deny
// transaction-control SQL. Dropping this connection rolls back the empty write
// transaction and releases the lock, including when copying fails.
fn lock_copy_source(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(Error::invalid(
            "database file copy requires DELETE journal mode",
        ));
    }
    Ok(connection)
}

fn copy_database(source: &Path, destination: &Path) -> Result<()> {
    let _lock = lock_copy_source(source)?;
    fs::copy(source, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_lock_excludes_writers_and_releases_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let writer = Connection::open(&source).unwrap();
        writer
            .execute_batch("CREATE TABLE t(value); INSERT INTO t VALUES (42)")
            .unwrap();
        writer.busy_timeout(Duration::ZERO).unwrap();
        let lock = lock_copy_source(&source).unwrap();
        let error = writer
            .execute_batch("INSERT INTO t VALUES (43)")
            .unwrap_err();
        assert_eq!(Error::from(error).code, "CONFLICT");
        drop(lock);
        writer.execute_batch("INSERT INTO t VALUES (43)").unwrap();
    }

    #[test]
    fn copy_refuses_active_writer_and_wal() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let destination = directory.path().join("copy.sqlite");
        let writer = Connection::open(&source).unwrap();
        writer
            .execute_batch("CREATE TABLE t(value); BEGIN IMMEDIATE; INSERT INTO t VALUES (42)")
            .unwrap();
        assert_eq!(
            copy_database(&source, &destination).unwrap_err().code,
            "CONFLICT"
        );
        assert!(!destination.exists());
        writer
            .execute_batch("ROLLBACK; PRAGMA journal_mode=WAL; INSERT INTO t VALUES (43)")
            .unwrap();
        assert_eq!(
            copy_database(&source, &destination).unwrap_err().code,
            "INVALID_ARGUMENT"
        );
        assert!(!destination.exists());
        writer.execute_batch("INSERT INTO t VALUES (44)").unwrap();
    }

    #[test]
    fn copy_preserves_data_and_releases_lock_on_io_failure() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let destination = directory.path().join("copy.sqlite");
        let writer = Connection::open(&source).unwrap();
        writer
            .execute_batch("CREATE TABLE t(value); INSERT INTO t VALUES (42)")
            .unwrap();
        copy_database(&source, &destination).unwrap();
        let copy = Connection::open(&destination).unwrap();
        assert_eq!(
            copy.query_row("SELECT value FROM t", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert_eq!(
            copy_database(&source, directory.path()).unwrap_err().code,
            "IO_ERROR"
        );
        writer.busy_timeout(Duration::ZERO).unwrap();
        writer.execute_batch("INSERT INTO t VALUES (43)").unwrap();
    }
}

pub(super) struct WorkingDatabase {
    pub(super) connection: Connection,
    pub(super) path: PathBuf,
}

impl Transaction {
    pub(super) fn snapshot_path(&self, snapshot: &str) -> PathBuf {
        self.store
            .root()
            .join("snapshots")
            .join(format!("{snapshot}.sqlite"))
    }
    pub(super) fn working(&self, db: Option<&Database>) -> Result<PathBuf> {
        let path = self.staging.join(format!("{}.sqlite", id()));
        if let Some(db) = db {
            copy_database(&self.snapshot_path(&db.snapshot), &path)?;
        }
        Ok(path)
    }
    pub(super) fn seal(&self, path: &Path) -> Result<String> {
        if fs::metadata(path)?.len() > sql::MAX_DB {
            return Err(Error::limit("database exceeds 256 MiB disk quota"));
        }
        File::open(path)?.sync_all()?;
        let snapshot = id();
        fs::rename(path, self.snapshot_path(&snapshot))?;
        sync_dir(&self.store.root().join("snapshots"))?;
        #[cfg(test)]
        crate::durability::publication_failpoint("after_snapshot_seal");
        Ok(snapshot)
    }
    pub(super) fn ensure_working(
        &mut self,
        namespace: &str,
        database: &str,
        db: &Database,
    ) -> Result<&Connection> {
        let key = (namespace.to_string(), database.to_string());
        if !self.working_databases.contains_key(&key) {
            let path = self.working(Some(db))?;
            let connection = sql::open(&path, true)?;
            self.working_databases
                .insert(key.clone(), WorkingDatabase { connection, path });
        }
        Ok(&self.working_databases[&key].connection)
    }
    pub(super) fn freeze_databases(&self, namespace: &str, manifest: &mut Manifest) -> Result<()> {
        for (database_name, database) in &mut manifest.databases {
            if let Some(working) = self
                .working_databases
                .get(&(namespace.to_string(), database_name.clone()))
            {
                let frozen = self.staging.join(format!("{}.sqlite", id()));
                // Lock the main database while preserving the source connection's temporary state.
                copy_database(&working.path, &frozen)?;
                database.snapshot = self.seal(&frozen)?;
            }
        }
        Ok(())
    }
    pub(super) fn seal_working_databases(&mut self) -> Result<()> {
        for ((namespace, database), working) in std::mem::take(&mut self.working_databases) {
            working.connection.close().map_err(|(_, e)| e)?;
            let snapshot = self.seal(&working.path)?;
            if let Some(db) = self
                .namespaces
                .get_mut(&namespace)
                .and_then(|ns| ns.manifest.databases.get_mut(&database))
            {
                db.snapshot = snapshot;
            }
        }
        Ok(())
    }
}

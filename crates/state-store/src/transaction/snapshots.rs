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
use rusqlite::Connection;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

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
            fs::copy(self.snapshot_path(&db.snapshot), &path)?;
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
                // All statements have completed and DELETE journals are closed.
                // Copy the main database while preserving the source connection's temporary state.
                fs::copy(&working.path, &frozen)?;
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

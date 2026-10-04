//! Explicit retention and orphan collection, coordinated with every root invocation.
use super::Store;
use crate::{Result, durability, identity, model::Manifest};
use rusqlite::TransactionBehavior;
use serde::Serialize;
use std::{collections::BTreeSet, fs};

/// Counts removed by one successful explicit maintenance pass.
#[derive(Debug, Default, Serialize)]
pub struct MaintenanceReport {
    pub revisions_removed: usize,
    pub objects_removed: usize,
    pub snapshots_removed: usize,
    pub staging_directories_removed: usize,
    pub receipts_removed: usize,
    pub tombstones_compacted: usize,
}

impl Store {
    /// Prune all historical revisions and orphan payloads, compact deleted
    /// namespaces, and retain only the newest `retain_receipts` receipts globally.
    /// Expired idempotency keys may execute again. No active invocation may overlap
    /// collection; this returns CONFLICT immediately if one holds a shared pin.
    pub fn maintenance(&self, retain_receipts: usize) -> Result<MaintenanceReport> {
        let retain = i64::try_from(retain_receipts)
            .map_err(|_| crate::Error::invalid("receipt retention exceeds signed 64-bit range"))?;
        let _guard = durability::root_lock(self.root(), true)?;
        let mut conn = self.catalog()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = MaintenanceReport::default();
        let empty = serde_json::to_string(&Manifest::default())?;
        let tombstones: i64 = tx.query_row(
            "SELECT count(*) FROM namespaces n JOIN revisions r ON r.id=n.head_revision WHERE n.deleted=1 AND r.manifest != ?",
            [&empty], |row| row.get(0),
        )?;
        if tombstones > 0 {
            let revision = identity::id();
            tx.execute(
                "INSERT INTO revisions(id,manifest) VALUES(?,?)",
                [&revision, &empty],
            )?;
            report.tombstones_compacted = tx.execute(
                "UPDATE namespaces SET head_revision=? WHERE deleted=1 AND head_revision IN (SELECT id FROM revisions WHERE manifest != ?)",
                [&revision, &empty],
            )?;
        }
        let manifests = {
            let mut stmt = tx.prepare("SELECT DISTINCT r.manifest FROM revisions r JOIN namespaces n ON n.head_revision=r.id")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut snapshots = BTreeSet::new();
        tx.execute_batch("CREATE TEMP TABLE live_objects(hash TEXT PRIMARY KEY)")?;
        for manifest in manifests {
            let manifest: Manifest = serde_json::from_str(&manifest)?;
            for hash in manifest.files.values() {
                tx.execute("INSERT OR IGNORE INTO live_objects VALUES(?)", [hash])?;
            }
            // Source text is embedded, but retain its object too for consistency
            // with pinned declarations after editing or deleting the source file.
            for function in manifest.functions.values() {
                tx.execute(
                    "INSERT OR IGNORE INTO live_objects VALUES(?)",
                    [&function.source_hash],
                )?;
            }
            snapshots.extend(manifest.databases.into_values().map(|db| db.snapshot));
        }
        report.revisions_removed = tx.execute(
            "DELETE FROM revisions WHERE id NOT IN (SELECT head_revision FROM namespaces)",
            [],
        )?;
        report.objects_removed = tx.execute(
            "DELETE FROM objects WHERE hash NOT IN (SELECT hash FROM live_objects)",
            [],
        )?;
        report.receipts_removed = tx.execute(
            "DELETE FROM receipts WHERE rowid NOT IN (SELECT rowid FROM receipts ORDER BY rowid DESC LIMIT ?)", [retain],
        )?;
        tx.execute(
            "UPDATE catalog_state SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        // Remove catalog references first. A crash after this commit leaves
        // harmless orphans that the next maintenance pass can collect.
        tx.commit()?;
        for entry in fs::read_dir(self.root().join("snapshots"))? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(snapshot) = name.to_str().and_then(|s| s.strip_suffix(".sqlite")) else {
                continue;
            };
            if uuid::Uuid::parse_str(snapshot).is_ok()
                && !snapshots.contains(snapshot)
                && entry.file_type()?.is_file()
            {
                fs::remove_file(entry.path())?;
                report.snapshots_removed += 1;
            }
        }
        for entry in fs::read_dir(self.root().join("staging"))? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
                && entry.file_type()?.is_dir()
            {
                fs::remove_dir_all(entry.path())?;
                report.staging_directories_removed += 1;
            }
        }
        durability::sync_dir(&self.root().join("snapshots"))?;
        durability::sync_dir(&self.root().join("staging"))?;
        // Reuse freed catalog pages and trim the WAL; do not require a full VACUUM
        // (which needs another database-sized allocation during maintenance).
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(report)
    }
}

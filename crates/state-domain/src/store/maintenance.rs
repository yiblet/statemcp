//! Domain metadata retention within one overlay invocation.
use super::{Store, catalog};
use crate::{Result, identity, model::Manifest};
use serde::Serialize;

/// Counts removed by an explicit maintenance pass. The legacy snapshot/staging
/// fields stay wire-compatible and are zero: the overlay creates neither kind.
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
    /// Prune obsolete domain metadata and receipts under exclusive root ownership.
    /// Resource databases are deleted atomically when resources are removed;
    /// maintenance has no filesystem collection or independent publication path.
    pub fn maintenance(&self, retain_receipts: usize) -> Result<MaintenanceReport> {
        let retain = i64::try_from(retain_receipts)
            .map_err(|_| crate::Error::invalid("receipt retention exceeds signed 64-bit range"))?;
        let backing = self.backing.begin()?;
        let report = {
            let connection = backing.connect(&catalog::name()?)?;
            connection.execute_batch("PRAGMA foreign_keys=ON")?;
            let tx = connection.unchecked_transaction()?;
            let mut report = MaintenanceReport::default();
            let empty = serde_json::to_string(&Manifest::default())?;
            let tombstones: i64 = tx.query_row("SELECT count(*) FROM namespaces n JOIN revisions r ON r.id=n.head_revision WHERE n.deleted=1 AND r.manifest != ?", [&empty], |row| row.get(0))?;
            if tombstones > 0 {
                let revision = identity::id();
                tx.execute(
                    "INSERT INTO revisions(id,manifest) VALUES(?,?)",
                    [&revision, &empty],
                )?;
                report.tombstones_compacted = tx.execute("UPDATE namespaces SET head_revision=? WHERE deleted=1 AND head_revision IN (SELECT id FROM revisions WHERE manifest != ?)", [&revision, &empty])?;
            }
            let manifests = {
                let mut statement = tx.prepare("SELECT DISTINCT r.manifest FROM revisions r JOIN namespaces n ON n.head_revision=r.id")?;
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            tx.execute_batch("CREATE TEMP TABLE live_objects(hash TEXT PRIMARY KEY)")?;
            for manifest in manifests {
                let manifest: Manifest = serde_json::from_str(&manifest)?;
                for hash in manifest.files.values() {
                    tx.execute("INSERT OR IGNORE INTO live_objects VALUES(?)", [hash])?;
                }
                for function in manifest.functions.values() {
                    tx.execute(
                        "INSERT OR IGNORE INTO live_objects VALUES(?)",
                        [&function.source_hash],
                    )?;
                }
            }
            report.revisions_removed = tx.execute(
                "DELETE FROM revisions WHERE id NOT IN (SELECT head_revision FROM namespaces)",
                [],
            )?;
            report.objects_removed = tx.execute(
                "DELETE FROM objects WHERE hash NOT IN (SELECT hash FROM live_objects)",
                [],
            )?;
            report.receipts_removed = tx.execute("DELETE FROM receipts WHERE rowid NOT IN (SELECT rowid FROM receipts ORDER BY rowid DESC LIMIT ?)", [retain])?;
            tx.execute(
                "UPDATE catalog_state SET generation=generation+1 WHERE singleton=1",
                [],
            )?;
            tx.commit()?;
            report
        };
        backing.commit()?;
        Ok(report)
    }
}

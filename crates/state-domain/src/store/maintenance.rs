//! Domain metadata retention within one overlay invocation.
use super::{Store, catalog};
use crate::Result;
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
            let pruned = super::retention::prune(&tx, self.history_size)?;
            report.revisions_removed = pruned.revisions;
            report.objects_removed = pruned.objects;
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

//! Bounded revision ancestry and transactional content reclamation.
use crate::{Error, ErrorCode, Result};
use rusqlite::Connection;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    num::NonZeroUsize,
    str::FromStr,
};

/// Revisions retained per live namespace, including its current revision.
/// Deleted namespaces retain only their empty tombstone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistorySize(NonZeroUsize);
impl Default for HistorySize {
    fn default() -> Self {
        Self(NonZeroUsize::MIN.saturating_add(6))
    }
}
impl HistorySize {
    /// Establish a positive retention count. Zero would discard current state.
    pub fn new(value: usize) -> Result<Self> {
        NonZeroUsize::new(value)
            .map(Self)
            .ok_or_else(|| Error::invalid("history size must be at least 1"))
    }
    /// Number of retained revisions, including the current revision.
    pub fn get(self) -> usize {
        self.0.get()
    }
}
impl fmt::Display for HistorySize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}
impl FromStr for HistorySize {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        Self::new(
            value
                .parse()
                .map_err(|_| Error::invalid("history size must be a positive integer"))?,
        )
    }
}

pub(super) struct Pruned {
    pub revisions: usize,
    pub objects: usize,
}

/// Must execute inside the same catalog transaction as publication or migration.
pub(super) fn prune(conn: &Connection, history: HistorySize) -> Result<Pruned> {
    let revisions = prune_revisions(conn, history)?;
    // Cascading edge deletion decrements reference counts before zero-reference GC.
    let objects = conn.execute("DELETE FROM objects WHERE ref_count=0", [])?;
    Ok(Pruned { revisions, objects })
}

/// Migration prunes legacy history before interning sources, but must preserve
/// objects until every retained manifest has registered its references.
pub(super) fn prune_revisions(conn: &Connection, history: HistorySize) -> Result<usize> {
    let parents: BTreeMap<String, Option<String>> = {
        let mut statement = conn.prepare("SELECT id,parent_id FROM revisions")?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let heads: Vec<(String, bool)> = {
        let mut statement = conn.prepare("SELECT head_revision,deleted FROM namespaces")?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut retained = BTreeSet::new();
    for (head, deleted) in &heads {
        let mut current = Some(head.as_str());
        let mut visited = BTreeSet::new();
        for _ in 0..if *deleted { 1 } else { history.get() } {
            let Some(revision) = current else { break };
            if !visited.insert(revision) {
                return Err(Error::new(
                    ErrorCode::CorruptStore,
                    "revision ancestry contains a cycle",
                ));
            }
            let parent = parents.get(revision).ok_or_else(|| {
                Error::new(ErrorCode::CorruptStore, "revision ancestor is missing")
            })?;
            retained.insert(revision);
            current = parent.as_deref();
        }
    }
    conn.execute_batch("CREATE TEMP TABLE retained_revisions(id TEXT PRIMARY KEY)")?;
    for revision in retained {
        conn.execute("INSERT INTO retained_revisions VALUES(?)", [revision])?;
    }
    // Cut ancestry before deleting parents; every remaining chain is self-contained.
    conn.execute("UPDATE revisions SET parent_id=NULL WHERE id IN (SELECT id FROM retained_revisions) AND parent_id NOT IN (SELECT id FROM retained_revisions)", [])?;
    let revisions = conn.execute(
        "DELETE FROM revisions WHERE id NOT IN (SELECT id FROM retained_revisions)",
        [],
    )?;
    conn.execute_batch("DROP TABLE retained_revisions")?;
    Ok(revisions)
}

//! Revision ownership and bounded, named retention pins.
use super::Transaction;
use crate::{Error, ErrorCode, Result, store::catalog};
use rusqlite::{Connection, OptionalExtension, params};
use state_contract::namespace::*;

const PIN_LIMIT: u32 = 7;
fn retention(pinned: bool, name: Option<String>) -> Result<RevisionRetention> {
    match (pinned, name) {
        (false, None) => Ok(RevisionRetention::Recent),
        (false, Some(_)) => Err(Error::new(
            ErrorCode::CorruptStore,
            "an unpinned revision has a pin name",
        )),
        (true, name) => Ok(RevisionRetention::Pinned {
            name: name
                .map(PinName::parse)
                .transpose()
                .map_err(|error| Error::new(ErrorCode::CorruptStore, error.to_string()))?,
        }),
    }
}
fn selected_revision(
    conn: &Connection,
    namespace: &str,
    revision: &str,
) -> Result<RevisionRetention> {
    let record = conn.query_row(
        "SELECT pinned,pin_name FROM namespace_revisions WHERE namespace_id=? AND revision_id=?",
        params![namespace, revision], |row| Ok((row.get::<_, bool>(0)?, row.get::<_, Option<String>>(1)?)),
    ).optional()?.ok_or_else(|| Error::new(ErrorCode::NotFound, "revision is not retained in this namespace"))?;
    retention(record.0, record.1)
}
impl Transaction {
    /// List retained metadata revisions newest first, including detached pins.
    /// Historical database contents are not captured by these revisions.
    pub fn namespace_history(&mut self, input: &NamespaceGetInput) -> Result<NamespaceHistory> {
        self.operation(|tx| {
            let namespace = tx.namespace_identity(&input.namespace)?;
            let current = &tx.staged_namespace(namespace)?.revision;
            let conn = tx.backing.connect(&catalog::name()?)?;
            let mut statement = conn.prepare("SELECT r.id,n.pinned,n.pin_name FROM namespace_revisions n JOIN revisions r ON r.id=n.revision_id WHERE n.namespace_id=? ORDER BY r.rowid DESC")?;
            let rows = statement.query_map([namespace], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?, row.get::<_, Option<String>>(2)?)))?;
            let mut revisions = Vec::new();
            for row in rows {
                let (revision, pinned, name) = row?;
                revisions.push(RevisionInfo { current: revision == *current, revision, retention: retention(pinned, name)? });
            }
            Ok(NamespaceHistory { revisions, pin_limit: PIN_LIMIT })
        })
    }
    /// Protect a retained revision from rolling pruning. At most seven pins per
    /// namespace; names must be unique. Repeating a pin without a name preserves
    /// its existing name. An error poisons the enclosing transaction.
    pub fn pin_revision(&mut self, input: &NamespacePinInput) -> Result<RevisionInfo> {
        self.operation(|tx| {
            let namespace = tx.namespace_identity(&input.namespace)?;
            let current = tx.staged_namespace(namespace)?.revision == input.revision;
            let conn = tx.backing.connect(&catalog::name()?)?;
            let previous = selected_revision(&conn, namespace, &input.revision)?;
            if matches!(previous, RevisionRetention::Recent) {
                let count: u32 = conn.query_row("SELECT count(*) FROM namespace_revisions WHERE namespace_id=? AND pinned=1", [namespace], |row| row.get(0))?;
                if count >= PIN_LIMIT { return Err(Error::limit("namespace already has seven pinned revisions; unpin one first")); }
            }
            if let Some(name) = &input.name {
                let duplicate: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM namespace_revisions WHERE namespace_id=? AND pin_name=? AND revision_id!=?)", params![namespace,name.as_str(),input.revision], |row| row.get(0))?;
                if duplicate { return Err(Error::new(ErrorCode::AlreadyExists, "pin name already exists in this namespace")); }
            }
            conn.execute("UPDATE namespace_revisions SET pinned=1,pin_name=coalesce(?,pin_name) WHERE namespace_id=? AND revision_id=?", params![input.name.as_ref().map(PinName::as_str),namespace,input.revision])?;
            let retention = selected_revision(&conn, namespace, &input.revision)?;
            tx.retention_changed = true;
            Ok(RevisionInfo { revision: input.revision.clone(), current, retention })
        })
    }
    /// Remove a retention pin. Content outside the recent window is collected on
    /// publication; recent content remains eligible for ordinary rolling retention.
    pub fn unpin_revision(&mut self, input: &NamespaceRevisionInput) -> Result<RevisionInfo> {
        self.operation(|tx| {
            let namespace = tx.namespace_identity(&input.namespace)?;
            let current = tx.staged_namespace(namespace)?.revision == input.revision;
            let conn = tx.backing.connect(&catalog::name()?)?;
            selected_revision(&conn, namespace, &input.revision)?;
            conn.execute("UPDATE namespace_revisions SET pinned=0,pin_name=NULL WHERE namespace_id=? AND revision_id=?", params![namespace,input.revision])?;
            tx.retention_changed = true;
            Ok(RevisionInfo { revision: input.revision.clone(), current, retention: RevisionRetention::Recent })
        })
    }
}

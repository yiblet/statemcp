//! Catalog metadata reads and logical publication within an overlay invocation.
use super::{
    records::StoredManifest,
    retention::{self, HistorySize},
};
use crate::{Error, Receipt, Result, identity::id, model::Namespace};
use rusqlite::{Connection, OptionalExtension, params};
use state_contract::publication::CommitMetadata;
use state_contract::publication::CommitOutcome;
use std::collections::BTreeMap;

pub(crate) fn name() -> Result<state_store::DatabaseName> {
    Ok(state_store::DatabaseName::parse("catalog.sqlite")?)
}
pub(crate) fn load(conn: &Connection) -> Result<(i64, BTreeMap<String, Namespace>)> {
    let tx = conn.unchecked_transaction()?;
    let generation = tx.query_row(
        "SELECT generation FROM catalog_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let namespaces = {
        let mut stmt=tx.prepare("SELECT n.id,n.name,n.head_revision,n.deleted,r.manifest FROM namespaces n JOIN revisions r ON r.id=n.head_revision")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut namespaces = BTreeMap::new();
        for row in rows {
            let (id, name, revision, deleted, manifest) = row?;
            namespaces.insert(
                id.clone(),
                Namespace {
                    id,
                    name,
                    revision,
                    deleted,
                    manifest: serde_json::from_str::<StoredManifest>(&manifest)
                        .map_err(|source| {
                            Error::new(
                                crate::ErrorCode::CorruptStore,
                                format!("invalid namespace manifest: {source}"),
                            )
                        })?
                        .resolve(&tx)?,
                    dirty: false,
                },
            );
        }
        namespaces
    };
    let mut ownership = std::collections::BTreeSet::new();
    for namespace in namespaces.values() {
        for database in namespace.manifest.databases.values() {
            let filename = crate::transaction::databases::filename(database).map_err(|_| {
                Error::new(
                    crate::ErrorCode::CorruptStore,
                    "invalid database file identity",
                )
            })?;
            if !ownership.insert(filename) {
                return Err(Error::new(
                    crate::ErrorCode::CorruptStore,
                    "namespaces cannot share backing databases",
                ));
            }
        }
    }
    tx.commit()?;
    Ok((generation, namespaces))
}

pub(crate) fn publish(
    conn: &Connection,
    generation: i64,
    namespaces: &BTreeMap<String, Namespace>,
    objects: &BTreeMap<String, Vec<u8>>,
    pending_receipt: &Option<(String, String, Receipt)>,
    history_size: HistorySize,
    retention_changed: bool,
) -> Result<CommitOutcome> {
    let changed = retention_changed || namespaces.values().any(|n| n.dirty);
    let tx = conn.unchecked_transaction()?;
    if let Some((principal, key, desired)) = pending_receipt
        && let Some(existing) = receipt(&tx, principal, key)?
    {
        if existing.request_hash != desired.request_hash {
            return Err(Error::new(
                crate::ErrorCode::IdempotencyMismatch,
                "idempotency key was used for another request",
            ));
        }
        return Ok(CommitOutcome::Replayed(existing.result));
    }
    let current: i64 = tx.query_row(
        "SELECT generation FROM catalog_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let next_generation = current
        .checked_add(1)
        .ok_or_else(|| Error::limit("catalog generation overflow"))?;
    if current != generation {
        return Err(Error::new(
            crate::ErrorCode::Conflict,
            "catalog changed during invocation; retry from a fresh snapshot",
        ));
    }
    for (hash, bytes) in objects {
        tx.execute(
            "INSERT OR IGNORE INTO objects(hash,bytes) VALUES(?,?)",
            params![hash, bytes],
        )?;
    }
    let mut revisions = BTreeMap::new();
    for ns in namespaces.values().filter(|n| n.dirty) {
        let stored = StoredManifest::capture(&tx, &ns.manifest)?;
        let manifest = serde_json::to_string(&stored)?;
        // Mutable database pages may change while manifest file identities stay
        // fixed. Every dirty namespace must invalidate its optimistic revision.
        let revision = id();
        tx.execute(
            "INSERT INTO revisions(id,parent_id,manifest) VALUES(?,?,?)",
            params![
                revision,
                if ns.revision.is_empty() {
                    None
                } else {
                    Some(&ns.revision)
                },
                manifest
            ],
        )?;
        stored.record_references(&tx, &revision)?;
        tx.execute("INSERT INTO namespaces(id,name,head_revision,deleted) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,head_revision=excluded.head_revision,deleted=excluded.deleted",params![ns.id,ns.name,revision,ns.deleted])?;
        tx.execute(
            "INSERT INTO namespace_revisions(namespace_id,revision_id) VALUES(?,?)",
            params![ns.id, revision],
        )?;
        revisions.insert(ns.id.clone(), revision);
    }
    if let Some((principal, key, receipt)) = pending_receipt {
        tx.execute(
            "INSERT INTO receipts(principal,key,request_hash,result_json) VALUES(?,?,?,?)",
            params![
                principal,
                key,
                receipt.request_hash,
                serde_json::to_string(&receipt.result)?
            ],
        )?;
    }
    retention::prune(&tx, history_size)?;
    tx.execute(
        "UPDATE catalog_state SET generation=generation+1 WHERE singleton=1",
        [],
    )?;
    tx.commit()?;
    Ok(CommitOutcome::Published(CommitMetadata {
        generation: next_generation,
        revisions,
        changed,
    }))
}
pub(crate) fn receipt(conn: &Connection, principal: &str, key: &str) -> Result<Option<Receipt>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT request_hash,result_json FROM receipts WHERE principal=? AND key=?",
            params![principal, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(request_hash, result)| {
        Ok(Receipt {
            request_hash,
            result: serde_json::from_str(&result)?,
        })
    })
    .transpose()
}

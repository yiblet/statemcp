//! Internal catalog schema, consistent metadata reads, and atomic publication.
//! This connection is never exposed to application SQL.
use super::Store;
use crate::{Error, Receipt, Result, durability::sync_dir, identity::id, model::Namespace};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc, time::Duration};

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let root = crate::durability::create_root(path.as_ref())?;
        let _guard = crate::durability::root_lock(&root, false)?;
        fs::create_dir_all(root.join("snapshots"))?;
        fs::create_dir_all(root.join("staging"))?;
        // Verify durable directory flush support before accepting writes.
        sync_dir(&root)?;
        let store = Self {
            root: Arc::new(root),
        };
        let conn = store.catalog()?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != 0 && version != 1 {
            return Err(Error::new(
                "UNSUPPORTED_FEATURE",
                format!("unsupported catalog format {version}"),
            ));
        }
        conn.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS catalog_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1), generation INTEGER NOT NULL);
            INSERT OR IGNORE INTO catalog_state VALUES(1,0);
            CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY, bytes BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS revisions(id TEXT PRIMARY KEY, parent_id TEXT, manifest TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS namespaces(id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, head_revision TEXT NOT NULL REFERENCES revisions(id), deleted INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS receipts(principal TEXT NOT NULL, key TEXT NOT NULL, request_hash TEXT NOT NULL, result_json TEXT NOT NULL, PRIMARY KEY(principal,key));
            PRAGMA user_version=1; COMMIT;")?;
        sync_dir(&store.root)?;
        Ok(store)
    }
    pub(super) fn catalog(&self) -> Result<Connection> {
        let conn = Connection::open(self.root.join("catalog.sqlite"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        Ok(conn)
    }
    pub(crate) fn load(&self) -> Result<(i64, BTreeMap<String, Namespace>)> {
        let mut conn = self.catalog()?;
        let tx = conn.transaction()?;
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
                        manifest: serde_json::from_str(&manifest)?,
                        dirty: false,
                    },
                );
            }
            namespaces
        };
        tx.commit()?;
        Ok((generation, namespaces))
    }
    pub fn receipt(&self, principal: &str, key: &str) -> Result<Option<Receipt>> {
        let _guard = crate::durability::root_lock(self.root(), false)?;
        receipt(&self.catalog()?, principal, key)
    }
    pub(crate) fn object(&self, hash: &str) -> Result<Vec<u8>> {
        self.catalog()?
            .query_row("SELECT bytes FROM objects WHERE hash=?", [hash], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| Error::new("CORRUPT_STORE", "file object is missing"))
    }
    pub(crate) fn publish(
        &self,
        generation: i64,
        namespaces: &BTreeMap<String, Namespace>,
        objects: &BTreeMap<String, Vec<u8>>,
        pending_receipt: &Option<(String, String, Receipt)>,
    ) -> Result<Value> {
        let changed = namespaces.values().any(|n| n.dirty);
        let mut conn = self.catalog()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((principal, key, desired)) = pending_receipt
            && let Some(existing) = receipt(&tx, principal, key)?
        {
            if existing.request_hash != desired.request_hash {
                return Err(Error::new(
                    "IDEMPOTENCY_MISMATCH",
                    "idempotency key was used for another request",
                ));
            }
            return Ok(json!({"replayed":true,"result":existing.result}));
        }
        let current: i64 = tx.query_row(
            "SELECT generation FROM catalog_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if current != generation {
            return Err(Error::new(
                "CONFLICT",
                "catalog changed during invocation; retry from a fresh snapshot",
            ));
        }
        for (hash, bytes) in objects {
            tx.execute(
                "INSERT OR IGNORE INTO objects(hash,bytes) VALUES(?,?)",
                params![hash, bytes],
            )?;
        }
        let mut revisions = serde_json::Map::new();
        for ns in namespaces.values().filter(|n| n.dirty) {
            let manifest = serde_json::to_string(&ns.manifest)?;
            let previous: Option<String> = if ns.revision.is_empty() {
                None
            } else {
                tx.query_row(
                    "SELECT manifest FROM revisions WHERE id=?",
                    [&ns.revision],
                    |r| r.get(0),
                )
                .optional()?
            };
            let revision = if previous.as_deref() == Some(&manifest) {
                ns.revision.clone()
            } else {
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
                revision
            };
            tx.execute("INSERT INTO namespaces(id,name,head_revision,deleted) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,head_revision=excluded.head_revision,deleted=excluded.deleted",params![ns.id,ns.name,revision,ns.deleted])?;
            revisions.insert(ns.id.clone(), json!(revision));
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
        tx.execute(
            "UPDATE catalog_state SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        #[cfg(test)]
        crate::durability::publication_failpoint("before_catalog_commit");
        tx.commit()?;
        #[cfg(test)]
        crate::durability::publication_failpoint("after_catalog_commit");
        Ok(json!({"generation":current+1,"revisions":revisions,"changed":changed}))
    }
}
fn receipt(conn: &Connection, principal: &str, key: &str) -> Result<Option<Receipt>> {
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

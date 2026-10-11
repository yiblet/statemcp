//! Rust migration establishing the namespace catalog schema.
use super::{
    records::StoredManifest,
    retention::{self, HistorySize},
};
use crate::{Error, ErrorCode};
use state_migrations::{Context, ForwardError, Migration, Migrator, Service};
struct InitializeCatalog {
    migration: Migration,
}
impl Migrator for InitializeCatalog {
    fn migration(&self) -> Migration {
        self.migration.clone()
    }
    fn forward(&self, context: &Context<'_, '_>) -> std::result::Result<(), ForwardError> {
        // The initial transition assumes no concurrent old-store users. An old
        // catalog with resource databases needs an explicit forward data migration;
        // accepting its snapshot references would create missing empty databases.
        let old_revisions: bool = context.catalog().query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='revisions')",
            [],
            |r| r.get(0),
        )?;
        if old_revisions {
            let mut statement = context
                .catalog()
                .prepare("SELECT manifest FROM revisions")?;
            let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
            for row in rows {
                let manifest: crate::model::Manifest = serde_json::from_str(&row?)?;
                if !manifest.databases.is_empty() {
                    return Err(Box::new(Error::new(
                        ErrorCode::UnsupportedFeature,
                        "snapshot databases require a forward format migration",
                    )));
                }
            }
        }
        context.catalog().execute_batch(
            "CREATE TABLE IF NOT EXISTS catalog_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1), generation INTEGER NOT NULL);
            INSERT OR IGNORE INTO catalog_state VALUES(1,0);
            CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY, bytes BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS revisions(id TEXT PRIMARY KEY, parent_id TEXT, manifest TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS namespaces(id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, head_revision TEXT NOT NULL REFERENCES revisions(id), deleted INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS receipts(principal TEXT NOT NULL, key TEXT NOT NULL, request_hash TEXT NOT NULL, result_json TEXT NOT NULL, PRIMARY KEY(principal,key));"
        )?;
        Ok(())
    }
}
struct ContentAddressedCatalog {
    migration: Migration,
    history_size: HistorySize,
}
impl Migrator for ContentAddressedCatalog {
    fn migration(&self) -> Migration {
        self.migration.clone()
    }
    fn forward(&self, context: &Context<'_, '_>) -> std::result::Result<(), ForwardError> {
        let conn = context.catalog();
        conn.execute_batch(
            "ALTER TABLE objects ADD COLUMN ref_count INTEGER NOT NULL DEFAULT 0 CHECK(ref_count >= 0);
             CREATE INDEX objects_ref_count ON objects(ref_count);
             CREATE TABLE revision_objects(
                 revision_id TEXT NOT NULL REFERENCES revisions(id) ON DELETE CASCADE,
                 hash TEXT NOT NULL REFERENCES objects(hash),
                 uses INTEGER NOT NULL CHECK(uses > 0),
                 PRIMARY KEY(revision_id,hash)
             );
             CREATE INDEX revision_objects_hash ON revision_objects(hash);
             CREATE TRIGGER revision_objects_insert AFTER INSERT ON revision_objects BEGIN
                 UPDATE objects SET ref_count=ref_count+NEW.uses WHERE hash=NEW.hash;
             END;
             CREATE TRIGGER revision_objects_delete AFTER DELETE ON revision_objects BEGIN
                 UPDATE objects SET ref_count=ref_count-OLD.uses WHERE hash=OLD.hash;
             END;"
        )?;
        // Legacy maintenance removed ancestors without cutting the parent pointer.
        conn.execute(
            "UPDATE revisions SET parent_id=NULL WHERE parent_id NOT IN (SELECT id FROM revisions)",
            [],
        )?;
        // Free obsolete inline snapshots first so migration can reuse their pages
        // even when the legacy catalog is close to its storage size limit.
        retention::prune_revisions(conn, self.history_size)?;
        // A deleted namespace no longer owns pinned content. Its head remains as
        // an empty tombstone so identities and deletion state stay compatible.
        let tombstones: std::collections::BTreeSet<String> = {
            let mut statement =
                conn.prepare("SELECT head_revision FROM namespaces WHERE deleted=1")?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut statement = conn.prepare("SELECT id,manifest FROM revisions ORDER BY rowid")?;
        let mut rows = statement.query([])?;
        // Convert one manifest at a time; never hold all legacy source snapshots in memory.
        while let Some(row) = rows.next()? {
            let revision: String = row.get(0)?;
            let wire: String = row.get(1)?;
            let manifest = if tombstones.contains(&revision) {
                crate::model::Manifest::default()
            } else {
                serde_json::from_str::<crate::model::Manifest>(&wire)?
            };
            let stored = StoredManifest::capture(conn, &manifest)?;
            conn.execute(
                "UPDATE revisions SET manifest=? WHERE id=?",
                rusqlite::params![serde_json::to_string(&stored)?, revision],
            )?;
            stored.record_references(conn, &revision)?;
        }
        drop(rows);
        drop(statement);
        retention::prune(conn, self.history_size)?;
        Ok(())
    }
}
pub(super) fn service(
    history_size: HistorySize,
) -> std::result::Result<Service, state_migrations::Error> {
    let migration = Migration::new("initialize_domain_catalog", 1)?;
    Service::new(vec![
        Box::new(InitializeCatalog { migration }),
        Box::new(ContentAddressedCatalog {
            migration: Migration::new("content_addressed_catalog", 2)?,
            history_size,
        }),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{identity::hash, model::Manifest};
    use state_contract::{
        function::FunctionDeclaration,
        http::{Handler, Method, Route, RoutePattern, RouteTarget},
    };
    use std::{collections::BTreeMap, path::Path};

    fn legacy() -> Result<rusqlite::Connection, Box<dyn std::error::Error>> {
        let mut conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON")?;
        Service::new(vec![Box::new(InitializeCatalog {
            migration: Migration::new("initialize_domain_catalog", 1)?,
        })])?
        .run(Path::new("/tmp"), &mut conn)?;
        Ok(conn)
    }
    #[test]
    fn migration_deduplicates_pinned_sources_and_preserves_wire_declarations()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut conn = legacy()?;
        let source = "def answer(): return 42";
        let modules = BTreeMap::from([("helper".into(), source.into())]);
        let declaration = FunctionDeclaration {
            abi_version: 1,
            description: None,
            file: "/api.py".into(),
            input_schema: None,
            modules: modules.clone(),
            name: "answer".into(),
            output_schema: None,
            source: source.into(),
            source_hash: hash(source.as_bytes()),
            symbol: "answer".into(),
            version: "preserved-version".into(),
        };
        let mut manifest = Manifest::default();
        manifest
            .files
            .insert("/api.py".into(), hash(source.as_bytes()));
        manifest
            .functions
            .insert("answer".into(), declaration.clone());
        manifest.routes.insert(
            "answer".into(),
            Route {
                name: "answer".into(),
                path: RoutePattern::parse("/answer")?,
                methods: [Method::parse("GET")?].into(),
                target: RouteTarget::Function(Handler {
                    file: "/api.py".into(),
                    symbol: "answer".into(),
                    sources: state_contract::function::FunctionSources {
                        source: source.into(),
                        modules,
                    },
                }),
            },
        );
        let original = serde_json::to_value(&manifest)?;
        conn.execute(
            "INSERT INTO objects VALUES(?,?)",
            rusqlite::params![hash(source.as_bytes()), source.as_bytes()],
        )?;
        conn.execute(
            "INSERT INTO objects VALUES(?,?)",
            rusqlite::params![hash(b"orphan"), b"orphan".as_slice()],
        )?;
        for i in 0..12 {
            conn.execute(
                "INSERT INTO revisions VALUES(?,?,?)",
                rusqlite::params![
                    format!("r{i}"),
                    if i == 0 {
                        None
                    } else {
                        Some(format!("r{}", i - 1))
                    },
                    serde_json::to_string(&manifest)?
                ],
            )?;
        }
        conn.execute("INSERT INTO namespaces VALUES('ns','ns','r11',0)", [])?;
        let service = service(HistorySize::default())?;
        service.run(Path::new("/tmp"), &mut conn)?;
        service.run(Path::new("/tmp"), &mut conn)?;
        assert_eq!(
            conn.query_row("SELECT count(*) FROM revisions", [], |r| r.get::<_, i64>(0))?,
            7
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            conn.query_row("SELECT ref_count FROM objects", [], |r| r.get::<_, i64>(0))?,
            35
        );
        let wire: String =
            conn.query_row("SELECT manifest FROM revisions WHERE id='r11'", [], |r| {
                r.get(0)
            })?;
        assert!(!wire.contains(source));
        let resolved = serde_json::from_str::<StoredManifest>(&wire)?.resolve(&conn)?;
        assert_eq!(serde_json::to_value(resolved)?, original);
        conn.execute("UPDATE namespaces SET deleted=1", [])?;
        conn.execute(
            "UPDATE revisions SET manifest=? WHERE id='r11'",
            [serde_json::to_string(&StoredManifest::capture(
                &conn,
                &Manifest::default(),
            )?)?],
        )?;
        conn.execute("DELETE FROM revision_objects WHERE revision_id='r11'", [])?;
        retention::prune(&conn, HistorySize::default())?;
        assert_eq!(
            conn.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))?,
            0
        );
        Ok(())
    }
    #[test]
    fn missing_legacy_object_rolls_back_migration_and_ledger()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut conn = legacy()?;
        let manifest = Manifest {
            files: BTreeMap::from([("/missing".into(), hash(b"missing"))]),
            ..Manifest::default()
        };
        conn.execute(
            "INSERT INTO revisions VALUES('r',NULL,?)",
            [serde_json::to_string(&manifest)?],
        )?;
        conn.execute("INSERT INTO namespaces VALUES('ns','ns','r',0)", [])?;
        assert!(
            service(HistorySize::default())?
                .run(Path::new("/tmp"), &mut conn)
                .is_err()
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM migrations", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='revision_objects'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }
}

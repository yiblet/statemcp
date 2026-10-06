//! Rust migration establishing the namespace catalog schema.
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
pub(super) fn service() -> std::result::Result<Service, state_migrations::Error> {
    let migration = Migration::new("initialize_domain_catalog", 1)?;
    Service::new(vec![Box::new(InitializeCatalog { migration })])
}

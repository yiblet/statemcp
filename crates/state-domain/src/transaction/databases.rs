//! Typed database selection and independent namespace copies via overlay storage.
use super::Transaction;
use crate::{
    Result,
    identity::id,
    model::{Database, Manifest},
    sql,
};

pub(crate) fn filename(database: &Database) -> Result<state_store::DatabaseName> {
    Ok(state_store::DatabaseName::parse(format!(
        "{}.sqlite",
        database.snapshot
    ))?)
}
impl Transaction {
    pub(super) fn database<T>(
        &self,
        database: &Database,
        action: impl FnOnce(&rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        if !self.backing.database_exists(&filename(database)?)? {
            return Err(crate::Error::new(
                crate::ErrorCode::CorruptStore,
                "catalog references a missing database",
            ));
        }
        self.backing
            .with_connection(&filename(database)?, |connection| {
                sql::configure(connection)?;
                action(connection)
            })
    }
    pub(super) fn copy_databases(&self, manifest: &mut Manifest) -> Result<()> {
        // Every copy gets fresh database identities and files, even when unchanged.
        // No two namespaces may share a mutable backing database.
        for database in manifest.databases.values_mut() {
            let source = filename(database)?;
            let identity = id();
            let destination = state_store::DatabaseName::parse(format!("{identity}.sqlite"))?;
            self.backing.copy_database(&source, &destination)?;
            database.id = identity.clone();
            database.snapshot = identity;
        }
        Ok(())
    }
}

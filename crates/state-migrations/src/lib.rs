//! Forward migrations implemented in Rust and recorded in a SQLite catalog.
//!
//! Run synchronously before accepting requests. The caller guarantees exclusive
//! access to the storage root; this service does not acquire or transition locks.
use rusqlite::{
    Connection, Transaction, TransactionBehavior,
    hooks::{AuthAction, AuthContext, Authorization},
};
use std::{collections::BTreeSet, path::Path};
use thiserror::Error;

/// An implementor's application-specific migration failure.
pub type ForwardError = Box<dyn std::error::Error + Send + Sync>;

/// Stable identity and positive, globally ordered version of one migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    name: &'static str,
    version: u32,
}
impl Migration {
    /// Establish a stable ASCII identity and a nonzero version.
    pub fn new(name: &'static str, version: u32) -> Result<Self, Error> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || version == 0
        {
            return Err(Error::Definition(
                "migration names must be nonempty ASCII identifiers and versions must be positive",
            ));
        }
        Ok(Self { name, version })
    }
    /// The migration's persistent identity.
    pub fn name(&self) -> &'static str {
        self.name
    }
    /// The migration's persistent version.
    pub fn version(&self) -> u32 {
        self.version
    }
}

/// A forward migration expressed as normal Rust code.
///
/// Catalog writes are transactional. Filesystem side effects must be safe to
/// retry after an error or process crash. Implementors must not alter the
/// migrations ledger or replace the catalog's authorizer.
pub trait Migrator {
    /// Return the immutable identity of this migration.
    fn migration(&self) -> Migration;
    /// Apply forward changes; an error leaves this migration unrecorded.
    fn forward(&self, context: &Context<'_, '_>) -> Result<(), ForwardError>;
}

/// Root paths and the catalog transaction available to a migration.
pub struct Context<'a, 'conn> {
    root: &'a Path,
    fs: &'a dyn state_fs::FileSystem,
    catalog: &'a Transaction<'conn>,
}
impl<'a, 'conn> Context<'a, 'conn> {
    /// Storage root for retry-safe filesystem changes.
    pub fn root(&self) -> &Path {
        self.root
    }
    /// Filesystem supplied by the store. Use it for retry-safe file changes
    /// so migrations also work with an in-memory backend.
    pub fn fs(&self) -> &dyn state_fs::FileSystem {
        self.fs
    }
    /// Catalog transaction. Explicit SQL transaction control is denied.
    pub fn catalog(&self) -> &Transaction<'conn> {
        self.catalog
    }
}

/// Typed definition, history, persistence, and implementation failures.
#[derive(Debug, Error)]
pub enum Error {
    /// The supplied migration list cannot describe a forward history.
    #[error("invalid migration definition: {0}")]
    Definition(&'static str),
    /// Persisted history is not a prefix of the supplied migration list.
    #[error("catalog migration history does not match the supplied migrations")]
    HistoryMismatch,
    /// Catalog access or commit failed.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// A forward implementation failed, including its stable identity.
    #[error("migration {migration:?} failed: {source}")]
    Forward {
        /// Migration that failed.
        migration: Migration,
        /// Implementation-specific cause.
        #[source]
        source: ForwardError,
    },
}

struct Entry {
    migration: Migration,
    migrator: Box<dyn Migrator>,
}

/// Ordered forward migration service, constructed from Rust implementors.
pub struct Service {
    entries: Vec<Entry>,
}
impl Service {
    /// Validate unique names and strictly increasing versions in execution order.
    pub fn new(migrators: Vec<Box<dyn Migrator>>) -> Result<Self, Error> {
        let mut names = BTreeSet::new();
        let mut previous = 0;
        let mut entries = Vec::with_capacity(migrators.len());
        for migrator in migrators {
            let migration = migrator.migration();
            if !names.insert(migration.name) || migration.version <= previous {
                return Err(Error::Definition(
                    "names must be unique and versions must strictly increase",
                ));
            }
            previous = migration.version;
            entries.push(Entry {
                migration,
                migrator,
            });
        }
        Ok(Self { entries })
    }

    /// Read the catalog history and execute pending migrations in order.
    ///
    /// Each migration and its ledger row commit together. Earlier successful
    /// migrations survive a later failure. Unknown, reordered, or newer history
    /// is rejected. Use a dedicated catalog connection with no authorizer;
    /// exclusive root access must already be guaranteed by the caller.
    pub fn run(&self, root: &Path, catalog: &mut Connection) -> Result<(), Error> {
        self.run_with_fs(root, &state_fs::PhysicalFs, catalog)
    }

    /// Run using the caller's filesystem for forward migration file operations.
    /// Catalog transaction and history rules are the same as [`Self::run`].
    pub fn run_with_fs(
        &self,
        root: &Path,
        fs: &dyn state_fs::FileSystem,
        catalog: &mut Connection,
    ) -> Result<(), Error> {
        catalog.execute_batch(
            "CREATE TABLE IF NOT EXISTS migrations (
            name TEXT PRIMARY KEY NOT NULL,
            version INTEGER NOT NULL UNIQUE CHECK(version > 0)
        );",
        )?;
        let history = {
            let mut statement =
                catalog.prepare("SELECT name, version FROM migrations ORDER BY version")?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        if history.len() > self.entries.len()
            || history
                .iter()
                .zip(&self.entries)
                .any(|((name, version), entry)| {
                    name != entry.migration.name || *version != i64::from(entry.migration.version)
                })
        {
            return Err(Error::HistoryMismatch);
        }
        for entry in self.entries.iter().skip(history.len()) {
            let transaction = catalog.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Keep the implementation inside our transaction, including when it
            // executes SQL strings rather than using Transaction's Rust methods.
            transaction.authorizer(Some(deny_transaction_control))?;
            let outcome = entry.migrator.forward(&Context {
                root,
                fs,
                catalog: &transaction,
            });
            transaction.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            outcome.map_err(|source| Error::Forward {
                migration: entry.migration.clone(),
                source,
            })?;
            transaction.execute(
                "INSERT INTO migrations(name, version) VALUES (?, ?)",
                rusqlite::params![entry.migration.name, entry.migration.version],
            )?;
            transaction.commit()?;
        }
        Ok(())
    }
}

fn deny_transaction_control(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Transaction { .. } | AuthAction::Savepoint { .. } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

#[cfg(test)]
mod tests;

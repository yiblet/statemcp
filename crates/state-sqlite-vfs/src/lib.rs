//! Blocking SQLite storage with transaction-local copy-on-write overlays.
//!
//! SQLite commits affect only the overlay. [`Transaction::commit`] publishes all
//! managed databases together through a durable redo journal. Dropping a
//! transaction discards its unpublished changes. All access to a root must use
//! this crate: ordinary SQLite connections bypass its root lock and recovery.
//! Execute these APIs on a dedicated thread or in `spawn_blocking` in async code.
#![doc = include_str!("../README.md")]
pub use state_fs as fs;
mod journal;
#[cfg(test)]
mod memory_tests;
mod overlay;
mod vfs;

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use fs::{FileSystem, LockGuard, LockMode, PhysicalFs};
use overlay::OverlayFile;

/// Recoverable storage, SQLite, and invocation errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Catalog migration definitions, history, or a forward migration failed.
    #[error(transparent)]
    Migration(#[from] state_migrations::Error),
    /// Another invocation owns the root lock.
    #[error("another transaction owns the overlay root")]
    Busy,
    /// A read-only invocation attempted to mutate persistent state.
    #[error("read-only transaction cannot modify databases")]
    ReadOnly,
    /// Database names must be plain ASCII filenames ending in `.sqlite`.
    #[error("invalid database filename: {0}")]
    InvalidName(String),
    /// Persistent state could not be read, written, or synced.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// SQLite rejected an operation.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// The VFS could not be registered with SQLite.
    #[error("cannot register overlay VFS: {0}")]
    Registration(rusqlite::ffi::Error),
    /// A durable journal is malformed; it is retained for inspection.
    #[error("corrupt overlay journal: {0}")]
    CorruptJournal(&'static str),
    /// A bounded allocation or file size would exceed the supported limit.
    #[error("overlay limit exceeded: {0}")]
    Limit(&'static str),
    /// Close every connection before committing the outer transaction.
    #[error("SQLite connections are still open")]
    ConnectionsOpen,
    /// An I/O failure has made publication unsafe.
    #[error("transaction is aborted or its overlay encountered an I/O failure")]
    Aborted,
    /// The commit decision may already be durable. Recover this root before
    /// reading it; do not rerun the application invocation as a rollback retry.
    #[error("commit requires recovery: {0}")]
    RecoveryRequired(Box<Error>),
}

/// Result of a blocking storage operation.
pub type Result<T> = std::result::Result<T, Error>;

/// A parsed database filename, confined to one root directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DatabaseName(String);

impl DatabaseName {
    /// Parse a filename such as `catalog.sqlite` or `app_123.sqlite`.
    /// Rejects paths, dotfiles, non-ASCII names, and names over 128 bytes.
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let valid = value.strip_suffix(".sqlite").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        });
        if !valid || value.len() > 128 {
            return Err(Error::InvalidName(value));
        }
        Ok(Self(value))
    }

    /// Return the validated filename.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Handle to a directory of mutable, exclusively managed SQLite databases.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    fs: Arc<dyn FileSystem>,
}

impl Store {
    /// Create or open a root. Clean roots use shared ownership; interrupted
    /// commits require exclusive recovery before any database can be read.
    /// Existing SQLite WAL/journal sidecars are rejected when opening a database;
    /// import only clean, closed databases. Never modify managed files externally.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::builder().open(path)
    }

    /// Configure the filesystem used by every storage operation.
    pub fn builder() -> StoreBuilder {
        StoreBuilder {
            fs: Arc::new(PhysicalFs),
        }
    }

    /// Open a root and run its supplied forward migrations before returning.
    /// The entire batch and catalog history publish through the overlay redo
    /// journal under the exclusive `overlay.lock`. No old-lock transition occurs.
    pub fn open_with_migrations(
        path: impl AsRef<Path>,
        service: &state_migrations::Service,
    ) -> Result<Self> {
        let store = Self::open(path)?;
        store.migrate(service)?;
        Ok(store)
    }

    /// Run pending Rust migrations against `catalog.sqlite` under a full root lock.
    /// Catalog changes and history are discarded together if any migration fails.
    /// The batch is published only after every forward migration succeeds.
    /// Direct filesystem side effects remain the implementor's retry responsibility.
    pub fn migrate(&self, service: &state_migrations::Service) -> Result<()> {
        let transaction = self.begin()?;
        {
            let name = DatabaseName::parse("catalog.sqlite")?;
            let mut catalog = transaction.connect(&name)?;
            service.run_with_fs(&self.root, self.fs.as_ref(), &mut catalog.inner)?;
        }
        transaction.commit()
    }

    /// Acquire exclusive root ownership and begin an in-memory invocation.
    /// Returns [`Error::Busy`] immediately on contention. Recovery always runs
    /// before reading base files, including after a failed previous commit.
    pub fn begin(&self) -> Result<Transaction> {
        let guard = self.lock(Access::ReadWrite)?;
        journal::recover(self.fs.as_ref(), &self.root)?;
        Ok(Transaction {
            session: self.session(guard)?,
        })
    }

    /// Begin a read-only invocation under a shared root lock. Readers may coexist;
    /// writers and recovery require exclusive ownership. Returns `Busy` on contention.
    pub fn begin_read(&self) -> Result<ReadTransaction> {
        let guard = self.clean_read_lock()?;
        Ok(ReadTransaction {
            session: self.session(guard)?,
        })
    }

    fn session(&self, guard: RootGuard) -> Result<Session> {
        let state = Arc::new(Mutex::new(State {
            root: self.root.clone(),
            fs: Arc::clone(&self.fs),
            _guard: guard,
            files: BTreeMap::new(),
            deleted: BTreeSet::new(),
            open_databases: BTreeSet::new(),
            memory_bytes: 0,
            status: InvocationStatus::Active,
        }));
        let id = vfs::register_session(&state)?;
        Ok(Session { id, state })
    }

    fn clean_read_lock(&self) -> Result<RootGuard> {
        loop {
            let guard = self.lock(Access::ReadOnly)?;
            if !journal::needs_recovery(self.fs.as_ref(), &self.root)? {
                return Ok(guard);
            }
            // Release shared ownership before acquiring a fresh exclusive handle.
            // File-lock conversion semantics vary across platforms. Recheck after
            // reacquiring shared ownership so a new writer cannot slip past recovery.
            drop(guard);
            let exclusive = self.lock(Access::ReadWrite)?;
            journal::recover(self.fs.as_ref(), &self.root)?;
            drop(exclusive);
        }
    }

    fn lock(&self, access: Access) -> Result<RootGuard> {
        let mode = match access {
            Access::ReadOnly => LockMode::Shared,
            Access::ReadWrite => LockMode::Exclusive,
        };
        match self.fs.lock(&self.root.join("overlay.lock"), mode) {
            Ok(guard) => Ok(RootGuard {
                _guard: guard,
                access,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Err(Error::Busy),
            Err(error) => Err(error.into()),
        }
    }
}

/// A root invocation. Owns memory overlays and the exclusive root lock.
///
/// Only one connection per database may be open at a time. SQLite journals and
/// temporary files are also kept in memory. WAL mode is deliberately unsupported.
/// Limits: 256 MiB per file, 64 MiB of overlay blocks, and 1,024 managed files.
/// These APIs implement storage, not SQL authorization or namespace policy.
pub struct Transaction {
    session: Session,
}

/// A stable root view shared with other read-only invocations. Writers cannot
/// change its base files until it finishes. Connections and the VFS reject writes
/// to databases; in-memory SQLite scratch files remain available for queries.
pub struct ReadTransaction {
    session: Session,
}

impl ReadTransaction {
    /// Open an existing database read-only; missing databases are never created.
    /// One connection per database may be open within this invocation. Other read
    /// invocations can independently connect to the same database.
    pub fn connect(&self, name: &DatabaseName) -> Result<Connection<'_>> {
        self.session.connect(name)
    }

    /// Finish this invocation and release shared ownership. Connections must
    /// close first. Dropping the invocation also releases its lock.
    pub fn finish(self) -> Result<()> {
        self.session
            .state
            .lock()
            .map_err(|_| Error::Aborted)?
            .check_closed()
    }
}

struct Session {
    id: uuid::Uuid,
    state: Arc<Mutex<State>>,
}

impl Session {
    fn connect(&self, name: &DatabaseName) -> Result<Connection<'_>> {
        let access = self.state.lock().map_err(|_| Error::Aborted)?._guard.access;
        let flags = match access {
            Access::ReadOnly => rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            Access::ReadWrite => {
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            }
        };
        let path = format!("/{}/{}", self.id, name.as_str());
        let connection = rusqlite::Connection::open_with_flags_and_vfs(path, flags, vfs::VFS_NAME)?;
        match access {
            Access::ReadOnly => connection.execute_batch("PRAGMA temp_store=MEMORY;")?,
            Access::ReadWrite => connection.execute_batch(
                "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY;",
            )?,
        }
        Ok(Connection {
            inner: connection,
            _session: self,
        })
    }
}

impl Transaction {
    /// Open a database through the overlay VFS. SQLite commits remain private
    /// until outer publication. New databases are created only in the overlay.
    /// The returned connection borrows this invocation and must close first.
    pub fn connect(&self, name: &DatabaseName) -> Result<Connection<'_>> {
        self.session.connect(name)
    }

    /// Number of bytes occupied by overlay blocks, including SQLite journals.
    /// Unmodified database bytes are read from disk and do not count here.
    pub fn memory_bytes(&self) -> Result<usize> {
        Ok(self
            .session
            .state
            .lock()
            .map_err(|_| Error::Aborted)?
            .memory_bytes)
    }

    /// Stage deletion of a closed database. Returns false if it is absent.
    /// Deletion publishes with the other files and is discarded on outer rollback.
    pub fn delete_database(&self, name: &DatabaseName) -> Result<bool> {
        let mut state = self.session.state.lock().map_err(|_| Error::Aborted)?;
        state.check_active()?;
        state.require_write()?;
        if state.open_databases.contains(name) {
            return Err(Error::ConnectionsOpen);
        }
        let key = FileKey::Database(name.clone());
        if state.deleted.contains(name) {
            return Ok(false);
        }
        if !state.files.contains_key(&key) {
            if state.files.len() + state.deleted.len() >= overlay::MAX_FILES {
                return Err(Error::Limit("file count"));
            }
            match state.fs.metadata(&state.root.join(name.as_str())) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "managed database must be a regular file",
                    )
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.into()),
            }
        }
        if let Some(file) = state.files.remove(&key) {
            state.memory_bytes -= file.memory_bytes();
        }
        state.deleted.insert(name.clone());
        Ok(true)
    }

    /// Durably publish all changed databases together. Connections must already
    /// be closed; dropped SQLite transactions have rolled back before this call.
    /// Failure before the journal's commit decision leaves base files unchanged.
    /// [`Error::RecoveryRequired`] means publication must finish via recovery,
    /// rather than retrying the invocation. This consumes the invocation.
    pub fn commit(self) -> Result<()> {
        let state = self.session.state.lock().map_err(|_| Error::Aborted)?;
        state.check_closed()?;
        state.require_write()?;
        journal::publish(&state)
    }

    /// Abort this invocation, discarding every unpublished change.
    pub fn rollback(self) {}
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.status = InvocationStatus::Aborted;
        }
        vfs::remove_session(self.id);
    }
}

/// A SQLite connection whose lifetime cannot outlive its overlay invocation.
/// Dereferences to `rusqlite::Connection` for ordinary SQL APIs.
pub struct Connection<'a> {
    inner: rusqlite::Connection,
    _session: &'a Session,
}

impl Deref for Connection<'_> {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum FileKey {
    Database(DatabaseName),
    Temporary(String),
}

struct State {
    root: PathBuf,
    fs: Arc<dyn FileSystem>,
    // Shared ownership is required by independently owned SQLite VFS handles.
    // The lock outlives forgotten connections as well as the invocation itself.
    _guard: RootGuard,
    files: BTreeMap<FileKey, OverlayFile>,
    deleted: BTreeSet<DatabaseName>,
    open_databases: BTreeSet<DatabaseName>,
    memory_bytes: usize,
    status: InvocationStatus,
}

enum InvocationStatus {
    Active,
    Poisoned,
    Aborted,
}

#[derive(Clone, Copy)]
enum Access {
    ReadOnly,
    ReadWrite,
}

// Constructed only after the matching OS lock has been acquired. Keeping the
// access mode with its owned file prevents a session from losing that invariant.
struct RootGuard {
    _guard: Box<dyn LockGuard>,
    access: Access,
}

impl State {
    fn require_write(&self) -> Result<()> {
        match self._guard.access {
            Access::ReadOnly => Err(Error::ReadOnly),
            Access::ReadWrite => Ok(()),
        }
    }

    fn check_closed(&self) -> Result<()> {
        if !self.open_databases.is_empty() {
            return Err(Error::ConnectionsOpen);
        }
        self.check_active()
    }

    fn check_active(&self) -> Result<()> {
        match self.status {
            InvocationStatus::Active => Ok(()),
            InvocationStatus::Poisoned | InvocationStatus::Aborted => Err(Error::Aborted),
        }
    }
}

/// Builder selecting a backing filesystem. Defaults to the host filesystem.
pub struct StoreBuilder {
    fs: Arc<dyn FileSystem>,
}
impl StoreBuilder {
    /// Use an injected filesystem. Cloned backends may share persistent state.
    pub fn with_fs(mut self, fs: impl FileSystem + 'static) -> Self {
        self.fs = Arc::new(fs);
        self
    }
    /// Establish the root and recover interrupted publication before returning.
    pub fn open(self, path: impl AsRef<Path>) -> Result<Store> {
        let root = self.fs.prepare_root(path.as_ref())?;
        let store = Store { root, fs: self.fs };
        let _guard = store.clean_read_lock()?;
        store.fs.sync_dir(&store.root)?;
        Ok(store)
    }
    /// Open with an injected filesystem and apply the supplied migrations.
    pub fn open_with_migrations(
        self,
        path: impl AsRef<Path>,
        service: &state_migrations::Service,
    ) -> Result<Store> {
        let store = self.open(path)?;
        store.migrate(service)?;
        Ok(store)
    }
}

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod sqlite_tests;

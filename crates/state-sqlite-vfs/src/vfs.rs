//! Safe Rust adapter for sqlite-plugin; no C callbacks are implemented here.
use crate::{
    DatabaseName, Error, FileKey, InvocationStatus, Result, State,
    overlay::{MAX_FILES, MAX_MEMORY_BYTES, OverlayFile},
};
use sqlite_plugin::{
    flags::{AccessFlags, CreateMode, LockLevel, OpenKind, OpenMode, OpenOpts},
    logger::SqliteLogger,
    vars,
    vfs::{RegisterOpts, Vfs, VfsHandle, VfsResult, register_static},
};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    io,
    sync::{Arc, LazyLock, Mutex, OnceLock, Weak},
};
use uuid::Uuid;

pub(crate) const VFS_NAME: &str = "state-sqlite-overlay-v1";
type SharedState = Arc<Mutex<State>>;
type Registry = Mutex<BTreeMap<Uuid, Weak<Mutex<State>>>>;
static SESSIONS: LazyLock<Registry> = LazyLock::new(|| Mutex::new(BTreeMap::new()));
static REGISTERED: OnceLock<VfsResult<SqliteLogger>> = OnceLock::new();

pub(crate) fn register_session(state: &SharedState) -> Result<Uuid> {
    // sqlite-plugin retains registered VFS state for the process lifetime. Register
    // once and route through weak invocation entries, avoiding a leak per invocation.
    let registered = REGISTERED.get_or_init(|| {
        register_static(
            c"state-sqlite-overlay-v1".to_owned(),
            OverlayVfs,
            RegisterOpts {
                make_default: false,
            },
        )
    });
    if let Err(code) = registered {
        return Err(Error::Registration(rusqlite::ffi::Error::new(*code)));
    }
    let id = Uuid::new_v4();
    SESSIONS
        .lock()
        .map_err(|_| Error::Aborted)?
        .insert(id, Arc::downgrade(state));
    Ok(id)
}

pub(crate) fn remove_session(id: Uuid) {
    if let Ok(mut sessions) = SESSIONS.lock() {
        sessions.remove(&id);
    }
}

fn io_error(error: Error) -> io::Error {
    match error {
        Error::ReadOnly => io::ErrorKind::PermissionDenied.into(),
        Error::Io(error) => error,
        error @ Error::Limit(_) => io::Error::new(io::ErrorKind::WriteZero, error),
        error => io::Error::other(error),
    }
}

fn locate(path: &str) -> io::Result<(SharedState, FileKey)> {
    let (id, name) = path
        .strip_prefix('/')
        .and_then(|value| value.split_once('/'))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid overlay path"))?;
    let id =
        Uuid::parse_str(id).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if name.is_empty()
        || name.len() > 160
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid overlay filename",
        ));
    }
    let key = if name.ends_with(".sqlite") {
        FileKey::Database(DatabaseName::parse(name).map_err(io_error)?)
    } else {
        // SQLite journal names are not persistent database filenames. They are
        // scoped to this invocation and never translated to host paths.
        FileKey::Temporary(name.to_owned())
    };
    let state = SESSIONS
        .lock()
        .map_err(|_| io_error(Error::Aborted))?
        .get(&id)
        .and_then(Weak::upgrade)
        .ok_or_else(|| io_error(Error::Aborted))?;
    Ok((state, key))
}

struct OverlayVfs;

impl OverlayVfs {
    fn open(&self, path: &str, opts: OpenOpts) -> io::Result<Handle> {
        if opts.kind() == OpenKind::Wal {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "overlay uses rollback journals, not WAL",
            ));
        }
        if opts.kind() == OpenKind::MainDb && opts.delete_on_close() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "persistent databases cannot delete on close",
            ));
        }
        if path.starts_with("/temporary/")
            && matches!(
                opts.kind(),
                OpenKind::TempDb
                    | OpenKind::TempJournal
                    | OpenKind::TransientDb
                    | OpenKind::SubJournal
            )
        {
            return Ok(Handle {
                storage: Storage::Temporary(Mutex::new(MemoryFile {
                    file: OverlayFile::empty(),
                    bytes: 0,
                    poisoned: false,
                })),
                read_only: opts.mode().is_readonly(),
                delete_on_close: opts.delete_on_close(),
                lock: LockLevel::Unlocked,
            });
        }
        let (shared, key) = locate(path)?;
        let mut state = shared.lock().map_err(|_| io_error(Error::Aborted))?;
        state.check_active().map_err(io_error)?;
        match opts.kind() {
            OpenKind::MainDb if !opts.mode().is_readonly() => {
                state.require_write().map_err(io_error)?
            }
            OpenKind::MainJournal | OpenKind::SuperJournal => {
                state.require_write().map_err(io_error)?
            }
            _ => {}
        }
        match (&key, opts.kind()) {
            (FileKey::Database(_), OpenKind::MainDb) => {}
            (
                FileKey::Temporary(_),
                OpenKind::MainJournal
                | OpenKind::SuperJournal
                | OpenKind::SubJournal
                | OpenKind::TempDb
                | OpenKind::TempJournal
                | OpenKind::TransientDb,
            ) => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file kind does not match overlay path",
                ));
            }
        }
        if let FileKey::Database(name) = &key
            && state.open_databases.contains(name)
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "database already has an open connection",
            ));
        }
        let exists = exists(&state, &key).map_err(io_error)?;
        match opts.mode() {
            OpenMode::ReadOnly
            | OpenMode::ReadWrite {
                create: CreateMode::None,
            } if !exists => {
                return Err(io::ErrorKind::NotFound.into());
            }
            OpenMode::ReadWrite {
                create: CreateMode::MustCreate,
            } if exists => return Err(io::ErrorKind::AlreadyExists.into()),
            _ => {}
        }
        if !state.files.contains_key(&key) {
            if state.files.len() + state.deleted.len() >= MAX_FILES
                && !matches!(&key, FileKey::Database(name) if state.deleted.contains(name))
            {
                return Err(io_error(Error::Limit("file count")));
            }
            let file = match &key {
                FileKey::Database(name) if exists => {
                    // Copying only the main file of a WAL database misses committed
                    // content. This root accepts clean, closed databases only.
                    for suffix in ["-wal", "-shm", "-journal"] {
                        if match state
                            .fs
                            .metadata(&state.root.join(format!("{}{suffix}", name.as_str())))
                        {
                            Ok(_) => true,
                            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                            Err(error) => return Err(error),
                        } {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "database has an external SQLite sidecar",
                            ));
                        }
                    }
                    OverlayFile::load(state.fs.as_ref(), &state.root.join(name.as_str()))
                        .map_err(io_error)?
                }
                _ => OverlayFile::empty(),
            };
            state.files.insert(key.clone(), file);
            if let FileKey::Database(name) = &key {
                state.deleted.remove(name);
            }
        }
        if let FileKey::Database(name) = &key {
            state.open_databases.insert(name.clone());
        }
        drop(state);
        Ok(Handle {
            storage: Storage::Root { shared, key },
            read_only: opts.mode().is_readonly(),
            delete_on_close: opts.delete_on_close(),
            lock: LockLevel::Unlocked,
        })
    }

    fn delete(&self, path: &str) -> io::Result<()> {
        if path.starts_with("/temporary/") {
            return Ok(());
        }
        let (shared, key) = locate(path)?;
        let mut state = shared.lock().map_err(|_| io_error(Error::Aborted))?;
        state.check_active().map_err(io_error)?;
        if matches!(key, FileKey::Database(_)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "database deletion requires an outer storage operation",
            ));
        }
        if let Some(file) = state.files.remove(&key) {
            state.memory_bytes -= file.memory_bytes();
        }
        Ok(())
    }

    fn exists(&self, path: &str) -> io::Result<bool> {
        if path.starts_with("/temporary/") {
            return Ok(false);
        }
        let (shared, key) = locate(path)?;
        let state = shared.lock().map_err(|_| io_error(Error::Aborted))?;
        state.check_active().map_err(io_error)?;
        exists(&state, &key).map_err(io_error)
    }
}

fn exists(state: &State, key: &FileKey) -> Result<bool> {
    if let FileKey::Database(name) = key
        && state.deleted.contains(name)
    {
        return Ok(false);
    }
    if state.files.contains_key(key) {
        return Ok(true);
    }
    match key {
        FileKey::Database(name) => match state.fs.metadata(&state.root.join(name.as_str())) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        },
        FileKey::Temporary(_) => Ok(false),
    }
}

enum Storage {
    Root { shared: SharedState, key: FileKey },
    // Anonymous SQLite scratch files need no durable root. Their memory is
    // bounded independently; normal SQL temporary tables use temp_store=MEMORY.
    Temporary(Mutex<MemoryFile>),
}
struct MemoryFile {
    file: OverlayFile,
    bytes: usize,
    poisoned: bool,
}
struct Handle {
    storage: Storage,
    read_only: bool,
    delete_on_close: bool,
    lock: LockLevel,
}

impl Handle {
    fn with_file<T>(
        &self,
        operation: impl FnOnce(&mut OverlayFile, &mut usize) -> Result<T>,
    ) -> io::Result<T> {
        match &self.storage {
            Storage::Root { shared, key } => {
                let mut state = shared.lock().map_err(|_| io_error(Error::Aborted))?;
                state.check_active().map_err(io_error)?;
                let State {
                    files,
                    memory_bytes,
                    status,
                    ..
                } = &mut *state;
                let file = files.get_mut(key).ok_or_else(|| io_error(Error::Aborted))?;
                let result = operation(file, memory_bytes);
                if result.is_err() {
                    *status = InvocationStatus::Poisoned;
                }
                result.map_err(io_error)
            }
            Storage::Temporary(memory) => {
                let mut memory = memory.lock().map_err(|_| io_error(Error::Aborted))?;
                if memory.poisoned {
                    return Err(io_error(Error::Aborted));
                }
                let MemoryFile {
                    file,
                    bytes,
                    poisoned,
                } = &mut *memory;
                let result = operation(file, bytes);
                if result.is_err() {
                    *poisoned = true;
                }
                result.map_err(io_error)
            }
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Storage::Root { shared, key } = &self.storage
            && let Ok(mut state) = shared.lock()
        {
            match key {
                FileKey::Database(name) => {
                    state.open_databases.remove(name);
                }
                FileKey::Temporary(_) if self.delete_on_close => {
                    if let Some(file) = state.files.remove(key) {
                        state.memory_bytes -= file.memory_bytes();
                    }
                }
                FileKey::Temporary(_) => {}
            }
        }
    }
}

impl Handle {
    fn size(&self) -> io::Result<u64> {
        self.with_file(|file, _| Ok(file.len))
    }

    fn read_exact_at(&mut self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let complete = self.with_file(|file, _| file.read(offset, buf))?;
        if complete {
            Ok(())
        } else {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    }

    fn write_all_at(&mut self, bytes: &[u8], offset: u64) -> io::Result<()> {
        if self.read_only {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        self.with_file(|file, memory| {
            let allocated = file.write(offset, bytes, MAX_MEMORY_BYTES.saturating_sub(*memory))?;
            *memory += allocated;
            Ok(())
        })
    }

    fn set_len(&mut self, size: u64) -> io::Result<()> {
        if self.read_only {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        self.with_file(|file, memory| {
            *memory -= file.truncate(size)?;
            Ok(())
        })
    }

    // SQLite's sync is an inner transaction boundary. The outer redo journal
    // supplies durability only at Transaction::commit; no host writes occur here.
    fn sync(&mut self, _data_only: bool) -> io::Result<()> {
        self.with_file(|_, _| Ok(()))
    }

    fn lock(&mut self, lock: LockLevel) -> io::Result<()> {
        self.with_file(|_, _| Ok(()))?;
        // The exclusive root guard and single-main-handle rule eliminate other
        // writers/readers. SQLite still needs its per-handle lock bookkeeping.
        self.lock = lock;
        Ok(())
    }
    fn reserved(&mut self) -> io::Result<bool> {
        Ok(matches!(
            self.lock,
            LockLevel::Reserved | LockLevel::Pending | LockLevel::Exclusive
        ))
    }
}

impl VfsHandle for Handle {
    fn readonly(&self) -> bool {
        self.read_only
    }
    fn in_memory(&self) -> bool {
        true
    }
}

fn sqlite_error(error: io::Error) -> i32 {
    match error.kind() {
        io::ErrorKind::UnexpectedEof => vars::SQLITE_IOERR_SHORT_READ,
        io::ErrorKind::WriteZero => vars::SQLITE_FULL,
        io::ErrorKind::WouldBlock => vars::SQLITE_BUSY,
        io::ErrorKind::PermissionDenied => vars::SQLITE_READONLY,
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput | io::ErrorKind::AlreadyExists => {
            vars::SQLITE_CANTOPEN
        }
        _ => vars::SQLITE_IOERR,
    }
}

impl Vfs for OverlayVfs {
    type Handle = Handle;
    fn canonical_path<'a>(&self, path: Cow<'a, str>) -> VfsResult<Cow<'a, str>> {
        locate(&path).map_err(sqlite_error)?;
        Ok(path)
    }
    fn open(&self, path: Option<&str>, opts: OpenOpts) -> VfsResult<Handle> {
        self.open(path.unwrap_or("/temporary/scratch"), opts)
            .map_err(sqlite_error)
    }
    fn delete(&self, path: &str) -> VfsResult<()> {
        self.delete(path).map_err(sqlite_error)
    }
    fn access(&self, path: &str, _flags: AccessFlags) -> VfsResult<bool> {
        self.exists(path).map_err(sqlite_error)
    }
    fn file_size(&self, handle: &mut Handle) -> VfsResult<usize> {
        let size = handle.size().map_err(sqlite_error)?;
        usize::try_from(size).map_err(|_| vars::SQLITE_TOOBIG)
    }
    fn truncate(&self, handle: &mut Handle, size: usize) -> VfsResult<()> {
        handle.set_len(size as u64).map_err(sqlite_error)
    }
    fn write(&self, handle: &mut Handle, offset: usize, bytes: &[u8]) -> VfsResult<usize> {
        handle
            .write_all_at(bytes, offset as u64)
            .map_err(sqlite_error)?;
        Ok(bytes.len())
    }
    fn read(&self, handle: &mut Handle, offset: usize, bytes: &mut [u8]) -> VfsResult<usize> {
        handle
            .read_exact_at(bytes, offset as u64)
            .map_err(sqlite_error)?;
        Ok(bytes.len())
    }
    fn lock(&self, handle: &mut Handle, level: LockLevel) -> VfsResult<()> {
        handle.lock(level).map_err(sqlite_error)
    }
    fn unlock(&self, handle: &mut Handle, level: LockLevel) -> VfsResult<()> {
        Vfs::lock(self, handle, level)
    }
    fn check_reserved_lock(&self, handle: &mut Handle) -> VfsResult<bool> {
        handle.reserved().map_err(sqlite_error)
    }
    fn sync(&self, handle: &mut Handle) -> VfsResult<()> {
        handle.sync(false).map_err(sqlite_error)
    }
    fn close(&self, handle: Handle) -> VfsResult<()> {
        drop(handle);
        Ok(())
    }
    // The overlay has exact byte-range writes but promises no physical atomic
    // writes. The durable outer journal, rather than this flag, protects bases.
    fn device_characteristics(&self, _handle: &mut Handle) -> VfsResult<i32> {
        Ok(0)
    }
}

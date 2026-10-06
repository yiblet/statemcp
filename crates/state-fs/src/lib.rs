//! Blocking filesystem boundary for storage and deterministic unit tests.
use std::{
    collections::BTreeMap,
    fmt::Debug,
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Kind and length observed without following a symlink.
#[derive(Debug, Clone, Copy)]
pub enum Metadata {
    /// Ordinary file and its byte length.
    File(u64),
    /// Directory.
    Directory,
    /// Symlink or another unsupported entry.
    Other,
}
impl Metadata {
    /// Whether this entry is an ordinary file.
    pub fn is_file(self) -> bool {
        matches!(self, Self::File(_))
    }
    /// Byte length for a file, or zero for other entries.
    pub fn len(self) -> u64 {
        match self {
            Self::File(len) => len,
            _ => 0,
        }
    }
    /// Whether this entry has zero file bytes.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}
/// File access and creation behavior; creation never truncates an existing file.
#[derive(Debug, Clone, Copy)]
pub enum OpenMode {
    /// Open an existing file for reading.
    Read,
    /// Open or create for reading and writing.
    Update,
    /// Atomically create a new file; fail if any entry already exists.
    CreateNew,
}
/// Lock access. Acquisition is nonblocking.
#[derive(Debug, Clone, Copy)]
pub enum LockMode {
    Shared,
    Exclusive,
}
/// Owned lock token. Dropping it releases the lock.
pub trait LockGuard: Send {}
/// Seekable file used by overlays and redo journals.
pub trait File: Read + Write + Seek + Send {
    /// Return current byte length.
    fn len(&self) -> io::Result<u64>;
    /// Return whether the file is empty.
    fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }
    /// Shrink or extend, zero filling any extension.
    fn set_len(&mut self, len: u64) -> io::Result<()>;
    /// Flush file contents and metadata to the backend's durability boundary.
    fn sync_all(&self) -> io::Result<()>;
}
/// Minimal filesystem contract for the overlay store.
///
/// Implementations must share files and locks across handles to the same root.
/// Rename must atomically replace its destination. Locks return `WouldBlock` on
/// contention. Metadata must not follow symlinks. Memory implementations may make
/// sync a no-op; persistent implementations must durably flush files/directories.
pub trait FileSystem: Debug + Send + Sync {
    /// Create and normalize a root; durably establish its directory ancestry.
    fn prepare_root(&self, path: &Path) -> io::Result<PathBuf>;
    /// Inspect an entry, returning `NotFound` when absent.
    fn metadata(&self, path: &Path) -> io::Result<Metadata>;
    /// Open according to explicit access/creation semantics.
    fn open(&self, path: &Path, mode: OpenMode) -> io::Result<Box<dyn File>>;
    /// Atomically rename an entry within the same filesystem.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Remove a file entry, returning `NotFound` when absent.
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Durably flush directory entries.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;
    /// Create/open a lock file and acquire shared or exclusive ownership.
    fn lock(&self, path: &Path, mode: LockMode) -> io::Result<Box<dyn LockGuard>>;
}
/// Host filesystem adapter.
#[derive(Debug, Default)]
pub struct PhysicalFs;
impl File for fs::File {
    fn len(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        fs::File::set_len(self, len)
    }
    fn sync_all(&self) -> io::Result<()> {
        fs::File::sync_all(self)
    }
}
impl LockGuard for fs::File {}
impl FileSystem for PhysicalFs {
    fn prepare_root(&self, path: &Path) -> io::Result<PathBuf> {
        let mut ancestor = path.to_path_buf();
        while !ancestor.try_exists()? {
            ancestor.pop();
            if ancestor.as_os_str().is_empty() {
                ancestor = PathBuf::from(".");
                break;
            }
        }
        let ancestor = fs::canonicalize(ancestor)?;
        fs::create_dir_all(path)?;
        let root = fs::canonicalize(path)?;
        let mut directory = root.as_path();
        loop {
            self.sync_dir(directory)?;
            if directory == ancestor {
                break;
            }
            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent;
        }
        Ok(root)
    }
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        let m = fs::symlink_metadata(path)?;
        Ok(if m.is_file() {
            Metadata::File(m.len())
        } else if m.is_dir() {
            Metadata::Directory
        } else {
            Metadata::Other
        })
    }
    fn open(&self, path: &Path, mode: OpenMode) -> io::Result<Box<dyn File>> {
        let mut options = fs::File::options();
        options.read(true);
        match mode {
            OpenMode::Read => {}
            OpenMode::Update => {
                options.write(true).create(true);
            }
            OpenMode::CreateNew => {
                options.write(true).create_new(true);
            }
        }
        Ok(Box::new(options.open(path)?))
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        fs::File::open(path)?.sync_all()
    }
    fn lock(&self, path: &Path, mode: LockMode) -> io::Result<Box<dyn LockGuard>> {
        let file = fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let result = match mode {
            LockMode::Shared => file.try_lock_shared(),
            LockMode::Exclusive => file.try_lock(),
        };
        result.map_err(|e| match e {
            fs::TryLockError::WouldBlock => io::ErrorKind::WouldBlock.into(),
            fs::TryLockError::Error(e) => e,
        })?;
        Ok(Box::new(file))
    }
}

/// In-memory filesystem for tests. Clones share files and locks; independent
/// instances are isolated. No operation accesses the host filesystem. Sync is a
/// no-op, so this backend does not simulate power-loss durability.
#[derive(Debug, Clone)]
pub struct MemoryFs {
    state: Arc<Mutex<MemoryState>>,
}
#[derive(Debug, Default)]
struct MemoryState {
    entries: BTreeMap<PathBuf, Entry>,
    locks: BTreeMap<PathBuf, Arc<Mutex<Locks>>>,
}
#[derive(Debug)]
enum Entry {
    Directory,
    File(Arc<Mutex<Vec<u8>>>),
}
#[derive(Debug, Default)]
struct Locks {
    readers: usize,
    writer: bool,
}
impl Default for MemoryFs {
    fn default() -> Self {
        let mut state = MemoryState::default();
        state.entries.insert(PathBuf::from("/"), Entry::Directory);
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }
}
impl MemoryFs {
    /// Create an empty filesystem with a root directory.
    pub fn new() -> Self {
        Self::default()
    }
}
fn locked<T>(mutex: &Mutex<T>) -> io::Result<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| io::Error::other("memory filesystem mutex poisoned"))
}
fn normalize(path: &Path) -> io::Result<PathBuf> {
    let mut result = PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(value) => result.push(value),
            Component::ParentDir | Component::Prefix(_) => {
                return Err(io::ErrorKind::InvalidInput.into());
            }
        }
    }
    Ok(result)
}
fn parent_exists(state: &MemoryState, path: &Path) -> io::Result<()> {
    match path.parent().and_then(|p| state.entries.get(p)) {
        Some(Entry::Directory) => Ok(()),
        _ => Err(io::ErrorKind::NotFound.into()),
    }
}
impl FileSystem for MemoryFs {
    fn prepare_root(&self, path: &Path) -> io::Result<PathBuf> {
        let path = normalize(path)?;
        let mut state = locked(&self.state)?;
        let mut current = PathBuf::from("/");
        for part in path.components() {
            if let Component::Normal(value) = part {
                current.push(value);
                match state.entries.get(&current) {
                    Some(Entry::File(_)) => return Err(io::ErrorKind::NotADirectory.into()),
                    Some(Entry::Directory) => {}
                    None => {
                        state.entries.insert(current.clone(), Entry::Directory);
                    }
                }
            }
        }
        Ok(path)
    }
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        let state = locked(&self.state)?;
        match state.entries.get(&normalize(path)?) {
            Some(Entry::Directory) => Ok(Metadata::Directory),
            Some(Entry::File(bytes)) => Ok(Metadata::File(locked(bytes)?.len() as u64)),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }
    fn open(&self, path: &Path, mode: OpenMode) -> io::Result<Box<dyn File>> {
        let path = normalize(path)?;
        let mut state = locked(&self.state)?;
        parent_exists(&state, &path)?;
        if matches!(mode, OpenMode::CreateNew) && state.entries.contains_key(&path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let bytes = match state.entries.get(&path) {
            Some(Entry::File(bytes)) => Arc::clone(bytes),
            Some(Entry::Directory) => return Err(io::ErrorKind::IsADirectory.into()),
            None if matches!(mode, OpenMode::Read) => return Err(io::ErrorKind::NotFound.into()),
            None => {
                let bytes = Arc::new(Mutex::new(Vec::new()));
                state.entries.insert(path, Entry::File(Arc::clone(&bytes)));
                bytes
            }
        };
        Ok(Box::new(MemoryFile {
            bytes,
            position: 0,
            writable: !matches!(mode, OpenMode::Read),
        }))
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let from = normalize(from)?;
        let to = normalize(to)?;
        let mut state = locked(&self.state)?;
        parent_exists(&state, &to)?;
        if !matches!(state.entries.get(&from), Some(Entry::File(_))) {
            return Err(io::ErrorKind::NotFound.into());
        }
        if matches!(state.entries.get(&to), Some(Entry::Directory)) {
            return Err(io::ErrorKind::IsADirectory.into());
        }
        let entry = state.entries.remove(&from).ok_or(io::ErrorKind::NotFound)?;
        state.entries.insert(to, entry);
        Ok(())
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let path = normalize(path)?;
        let mut state = locked(&self.state)?;
        match state.entries.get(&path) {
            Some(Entry::File(_)) => {}
            Some(Entry::Directory) => return Err(io::ErrorKind::IsADirectory.into()),
            None => return Err(io::ErrorKind::NotFound.into()),
        }
        state.entries.remove(&path);
        Ok(())
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        match self.metadata(path)? {
            Metadata::Directory => Ok(()),
            _ => Err(io::ErrorKind::NotADirectory.into()),
        }
    }
    fn lock(&self, path: &Path, mode: LockMode) -> io::Result<Box<dyn LockGuard>> {
        let path = normalize(path)?;
        self.open(&path, OpenMode::Update)?;
        let locks = {
            let mut state = locked(&self.state)?;
            Arc::clone(state.locks.entry(path).or_default())
        };
        {
            let mut current = locked(&locks)?;
            match mode {
                LockMode::Shared if !current.writer => {
                    current.readers = current
                        .readers
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("reader count overflow"))?;
                }
                LockMode::Exclusive if !current.writer && current.readers == 0 => {
                    current.writer = true
                }
                _ => return Err(io::ErrorKind::WouldBlock.into()),
            }
        }
        Ok(Box::new(MemoryGuard { locks, mode }))
    }
}
struct MemoryGuard {
    locks: Arc<Mutex<Locks>>,
    mode: LockMode,
}
impl LockGuard for MemoryGuard {}
impl Drop for MemoryGuard {
    fn drop(&mut self) {
        // A poisoned lock remains unavailable; dropping never grants ownership.
        if let Ok(mut locks) = self.locks.lock() {
            match self.mode {
                LockMode::Shared => locks.readers = locks.readers.saturating_sub(1),
                LockMode::Exclusive => locks.writer = false,
            }
        }
    }
}
struct MemoryFile {
    bytes: Arc<Mutex<Vec<u8>>>,
    position: u64,
    writable: bool,
}
impl Read for MemoryFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let bytes = locked(&self.bytes)?;
        let start = usize::try_from(self.position).map_err(|_| io::ErrorKind::InvalidInput)?;
        let source = bytes.get(start..).unwrap_or(&[]);
        let count = source.len().min(buffer.len());
        buffer
            .get_mut(..count)
            .ok_or(io::ErrorKind::InvalidInput)?
            .copy_from_slice(source.get(..count).ok_or(io::ErrorKind::InvalidInput)?);
        self.position += count as u64;
        Ok(count)
    }
}
impl Write for MemoryFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if !self.writable {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let start = usize::try_from(self.position).map_err(|_| io::ErrorKind::InvalidInput)?;
        let end = start
            .checked_add(buffer.len())
            .ok_or(io::ErrorKind::InvalidInput)?;
        let mut bytes = locked(&self.bytes)?;
        if end > bytes.len() {
            let additional = end - bytes.len();
            bytes.try_reserve(additional).map_err(io::Error::other)?;
            bytes.resize(end, 0);
        }
        bytes
            .get_mut(start..end)
            .ok_or(io::ErrorKind::InvalidInput)?
            .copy_from_slice(buffer);
        self.position = end as u64;
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Seek for MemoryFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => locked(&self.bytes)?.len() as i128 + i128::from(offset),
        };
        self.position = u64::try_from(position).map_err(|_| io::ErrorKind::InvalidInput)?;
        Ok(self.position)
    }
}
impl File for MemoryFile {
    fn len(&self) -> io::Result<u64> {
        Ok(locked(&self.bytes)?.len() as u64)
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        if !self.writable {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let len = usize::try_from(len).map_err(|_| io::ErrorKind::InvalidInput)?;
        let mut bytes = locked(&self.bytes)?;
        if len > bytes.len() {
            let additional = len - bytes.len();
            bytes.try_reserve(additional).map_err(io::Error::other)?;
        }
        bytes.resize(len, 0);
        Ok(())
    }
    fn sync_all(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_survive_rename_and_unlink_and_observe_truncation() -> io::Result<()> {
        let fs = MemoryFs::new();
        let root = fs.prepare_root(Path::new("/state"))?;
        let source = root.join("source");
        let target = root.join("target");
        let mut writer = fs.open(&source, OpenMode::CreateNew)?;
        writer.write_all(b"abcdef")?;
        let mut reader = fs.open(&source, OpenMode::Read)?;
        writer.set_len(2)?;
        writer.set_len(5)?;
        fs.rename(&source, &target)?;
        assert_eq!(
            fs.metadata(&source).err().map(|e| e.kind()),
            Some(io::ErrorKind::NotFound)
        );
        fs.remove_file(&target)?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"ab\0\0\0");
        assert_eq!(
            reader.write(b"x").err().map(|e| e.kind()),
            Some(io::ErrorKind::PermissionDenied)
        );
        assert_eq!(
            reader.set_len(0).err().map(|e| e.kind()),
            Some(io::ErrorKind::PermissionDenied)
        );
        reader.seek(SeekFrom::Start(50))?;
        assert_eq!(reader.read(&mut [0; 1])?, 0);
        assert!(reader.seek(SeekFrom::Current(-51)).is_err());
        Ok(())
    }

    #[test]
    fn cloned_filesystems_share_locks_and_atomic_creation() -> io::Result<()> {
        let fs = MemoryFs::new();
        let other = fs.clone();
        let root = fs.prepare_root(Path::new("/state"))?;
        let lock = root.join("lock");
        let first = fs.lock(&lock, LockMode::Shared)?;
        let second = other.lock(&lock, LockMode::Shared)?;
        assert!(
            matches!(fs.lock(&lock, LockMode::Exclusive), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
        );
        drop(first);
        assert!(
            matches!(fs.lock(&lock, LockMode::Exclusive), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
        );
        drop(second);
        let writer = fs.lock(&lock, LockMode::Exclusive)?;
        assert!(
            matches!(other.lock(&lock, LockMode::Shared), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
        );
        drop(writer);
        other.lock(&lock, LockMode::Shared)?;
        fs.open(&root.join("file"), OpenMode::CreateNew)?;
        assert!(
            matches!(other.open(&root.join("file"), OpenMode::CreateNew), Err(e) if e.kind() == io::ErrorKind::AlreadyExists)
        );
        assert!(fs.prepare_root(&root.join("file/subdirectory")).is_err());
        assert!(fs.prepare_root(Path::new("/state/../other")).is_err());
        assert!(
            matches!(MemoryFs::new().metadata(&root), Err(e) if e.kind() == io::ErrorKind::NotFound)
        );
        Ok(())
    }

    #[test]
    fn files_reopen_sync_rename_truncate_and_keep_old_handles() -> io::Result<()> {
        let fs = MemoryFs::new();
        let root = fs.prepare_root(Path::new("/nested/root"))?;
        let source = root.join("source");
        let target = root.join("target");
        let mut file = fs.open(&source, OpenMode::CreateNew)?;
        file.write_all(b"abcdef")?;
        file.set_len(2)?;
        file.set_len(5)?;
        file.sync_all()?;
        assert!(
            matches!(fs.open(&source, OpenMode::CreateNew), Err(e) if e.kind() == io::ErrorKind::AlreadyExists)
        );
        fs.rename(&source, &target)?;
        fs.sync_dir(&root)?;
        let mut reader = fs.open(&target, OpenMode::Read)?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"ab\0\0\0");
        assert!(reader.write_all(b"x").is_err());
        assert!(reader.set_len(0).is_err());
        drop(reader);
        fs.remove_file(&target)?;
        fs.sync_dir(&root)?;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"ab\0\0\0");
        assert!(matches!(fs.metadata(&target), Err(e) if e.kind() == io::ErrorKind::NotFound));
        Ok(())
    }
}

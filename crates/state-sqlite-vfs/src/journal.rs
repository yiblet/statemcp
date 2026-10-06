//! Durable outer commit: publish a checksummed redo journal before mutating bases.
use crate::fs::{File, FileSystem, OpenMode};
use crate::{
    DatabaseName, Error, FileKey, Result, State,
    overlay::{BLOCK_SIZE, MAX_FILE_BYTES, MAX_FILES, MAX_MEMORY_BYTES},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

const MAGIC: &[u8; 8] = b"STOVLY01";
pub(crate) const PENDING: &str = "overlay.pending";
pub(crate) const COMMITTED: &str = "overlay.redo";
const MAX_JOURNAL_BYTES: u64 = (MAX_MEMORY_BYTES + MAX_FILES * 256 + 1024 * 1024) as u64;

// Called under shared root ownership: a writer cannot create a journal until
// that guard is released. Even a dangling symlink must go through validation.
pub(crate) fn needs_recovery(fs: &dyn FileSystem, root: &Path) -> Result<bool> {
    for name in [COMMITTED, PENDING] {
        match fs.metadata(&root.join(name)) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

struct JournalWriter {
    file: Box<dyn File>,
    hash: Sha256,
}

impl JournalWriter {
    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes)?;
        self.hash.update(bytes);
        Ok(())
    }
    fn u32(&mut self, value: usize) -> Result<()> {
        let value = u32::try_from(value).map_err(|_| Error::Limit("journal count"))?;
        self.bytes(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }
    fn finish(mut self) -> Result<()> {
        self.file.write_all(&self.hash.finalize())?;
        self.file.sync_all()?;
        Ok(())
    }
}

pub(crate) fn publish(state: &State) -> Result<()> {
    let count = state
        .files
        .iter()
        .filter(|(key, file)| matches!(key, FileKey::Database(_)) && file.dirty)
        .count()
        + state.deleted.len();
    if count == 0 {
        return Ok(());
    }
    if count > MAX_FILES {
        return Err(Error::Limit("journal file count"));
    }
    let pending = state.root.join(PENDING);
    // create_new prevents following a stray symlink or overwriting another journal.
    let mut writer = JournalWriter {
        file: state.fs.open(&pending, OpenMode::CreateNew)?,
        hash: Sha256::new(),
    };
    writer.bytes(MAGIC)?;
    writer.u32(count)?;
    for (key, file) in &state.files {
        let FileKey::Database(name) = key else {
            continue;
        };
        if !file.dirty {
            continue;
        }
        writer.u32(name.as_str().len())?;
        writer.bytes(name.as_str().as_bytes())?;
        writer.bytes(b"W")?;
        writer.u64(file.base_visible_len)?;
        writer.u64(file.len)?;
        writer.u32(file.blocks.len())?;
        for (offset, block) in &file.blocks {
            writer.u64(*offset)?;
            writer.bytes(block.as_ref())?;
        }
    }
    for name in &state.deleted {
        writer.u32(name.as_str().len())?;
        writer.bytes(name.as_str().as_bytes())?;
        writer.bytes(b"D")?;
    }
    writer.finish()?;
    state.fs.rename(&pending, &state.root.join(COMMITTED))?;
    // After rename, the decision might be durable even if directory sync fails.
    // Never report this as an aborted invocation or remove its recovery record.
    let finish = || -> Result<()> {
        state.fs.sync_dir(&state.root)?;
        recover(state.fs.as_ref(), &state.root)
    };
    finish().map_err(|error| Error::RecoveryRequired(Box::new(error)))
}

pub(crate) fn recover(fs: &dyn FileSystem, root: &Path) -> Result<()> {
    let committed = root.join(COMMITTED);
    match fs.metadata(&committed) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
                return Err(Error::CorruptJournal(
                    "journal is not a bounded regular file",
                ));
            }
            let mut bytes = Vec::new();
            fs.open(&committed, OpenMode::Read)?
                .take(MAX_JOURNAL_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_JOURNAL_BYTES {
                return Err(Error::CorruptJournal("journal exceeds limit"));
            }
            let plan = decode(&bytes)?;
            // Check every destination before modifying any of them.
            for change in &plan {
                check_destination(fs, &root.join(change.name().as_str()))?;
            }
            for change in plan {
                apply(fs, root, change)?;
            }
            fs.sync_dir(root)?;
            fs.remove_file(&committed)?;
            fs.sync_dir(root)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // An unpublished pending file is never replayed. No base was changed before
    // the rename, so recovery can discard an interrupted preparation safely.
    match fs.remove_file(&root.join(PENDING)) {
        Ok(()) => fs.sync_dir(root)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn check_destination(fs: &dyn FileSystem, path: &Path) -> Result<()> {
    match fs.metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(Error::CorruptJournal("destination is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

enum Change<'a> {
    Write(FileWrite<'a>),
    Delete(DatabaseName),
}
impl Change<'_> {
    fn name(&self) -> &DatabaseName {
        match self {
            Self::Write(write) => &write.name,
            Self::Delete(name) => name,
        }
    }
}

struct FileWrite<'a> {
    name: DatabaseName,
    visible: u64,
    len: u64,
    blocks: BTreeMap<u64, &'a [u8]>,
}

fn apply(fs: &dyn FileSystem, root: &Path, change: Change<'_>) -> Result<()> {
    match change {
        Change::Write(write) => apply_write(fs, root, write),
        Change::Delete(name) => match fs.remove_file(&root.join(name.as_str())) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

fn apply_write(fs: &dyn FileSystem, root: &Path, change: FileWrite<'_>) -> Result<()> {
    let mut file = fs.open(&root.join(change.name.as_str()), OpenMode::Update)?;
    file.set_len(change.visible)?;
    file.set_len(change.len)?;
    for (offset, block) in change.blocks {
        let count = (change.len - offset).min(BLOCK_SIZE as u64) as usize;
        let bytes = block
            .get(..count)
            .ok_or(Error::CorruptJournal("invalid block length"))?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
    }
    file.sync_all()?;
    Ok(())
}

struct Parser<'a> {
    bytes: &'a [u8],
}
impl<'a> Parser<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let value = self
            .bytes
            .get(..count)
            .ok_or(Error::CorruptJournal("truncated record"))?;
        self.bytes = self
            .bytes
            .get(count..)
            .ok_or(Error::CorruptJournal("truncated record"))?;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32> {
        let bytes = self
            .take(4)?
            .try_into()
            .map_err(|_| Error::CorruptJournal("invalid integer"))?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn u64(&mut self) -> Result<u64> {
        let bytes = self
            .take(8)?
            .try_into()
            .map_err(|_| Error::CorruptJournal("invalid integer"))?;
        Ok(u64::from_le_bytes(bytes))
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<Change<'_>>> {
    let payload_len = bytes
        .len()
        .checked_sub(32)
        .ok_or(Error::CorruptJournal("missing checksum"))?;
    let payload = bytes
        .get(..payload_len)
        .ok_or(Error::CorruptJournal("missing payload"))?;
    let expected = bytes
        .get(payload_len..)
        .ok_or(Error::CorruptJournal("missing checksum"))?;
    if Sha256::digest(payload).as_slice() != expected {
        return Err(Error::CorruptJournal("checksum mismatch"));
    }
    let mut parser = Parser { bytes: payload };
    if parser.take(MAGIC.len())? != MAGIC {
        return Err(Error::CorruptJournal("unsupported format"));
    }
    let count = parser.u32()? as usize;
    if count == 0 || count > MAX_FILES {
        return Err(Error::CorruptJournal("invalid file count"));
    }
    let mut changes = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    let mut total_blocks = 0_usize;
    for _ in 0..count {
        let name_len = parser.u32()? as usize;
        if name_len > 128 {
            return Err(Error::CorruptJournal("invalid filename length"));
        }
        let name = std::str::from_utf8(parser.take(name_len)?)
            .map_err(|_| Error::CorruptJournal("invalid filename encoding"))?;
        let name =
            DatabaseName::parse(name).map_err(|_| Error::CorruptJournal("invalid filename"))?;
        if !names.insert(name.clone()) {
            return Err(Error::CorruptJournal("duplicate filename"));
        }
        match parser.take(1)? {
            b"D" => {
                changes.push(Change::Delete(name));
                continue;
            }
            b"W" => {}
            _ => return Err(Error::CorruptJournal("unknown file operation")),
        }
        let visible = parser.u64()?;
        let len = parser.u64()?;
        if visible > len || len > MAX_FILE_BYTES {
            return Err(Error::CorruptJournal("invalid file length"));
        }
        let count = parser.u32()? as usize;
        total_blocks = total_blocks
            .checked_add(count)
            .ok_or(Error::CorruptJournal("block count overflow"))?;
        if total_blocks > MAX_MEMORY_BYTES / BLOCK_SIZE {
            return Err(Error::CorruptJournal("too many blocks"));
        }
        let mut blocks = BTreeMap::new();
        for _ in 0..count {
            let offset = parser.u64()?;
            if offset % BLOCK_SIZE as u64 != 0 || offset >= len {
                return Err(Error::CorruptJournal("invalid block offset"));
            }
            if blocks.insert(offset, parser.take(BLOCK_SIZE)?).is_some() {
                return Err(Error::CorruptJournal("duplicate block"));
            }
        }
        changes.push(Change::Write(FileWrite {
            name,
            visible,
            len,
            blocks,
        }));
    }
    if !parser.bytes.is_empty() {
        return Err(Error::CorruptJournal("trailing data"));
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemoryFs;

    fn fixture(fs: &MemoryFs, root: &Path) -> Result<()> {
        let mut writer = JournalWriter {
            file: fs.open(&root.join(COMMITTED), OpenMode::CreateNew)?,
            hash: Sha256::new(),
        };
        writer.bytes(MAGIC)?;
        writer.u32(2)?;
        for (name, value) in [("a.sqlite", 11), ("b.sqlite", 22)] {
            writer.u32(name.len())?;
            writer.bytes(name.as_bytes())?;
            writer.bytes(b"W")?;
            writer.u64(0)?;
            writer.u64(BLOCK_SIZE as u64)?;
            writer.u32(1)?;
            writer.u64(0)?;
            writer.bytes(&[value; BLOCK_SIZE])?;
        }
        writer.finish()
    }
    fn read(fs: &MemoryFs, path: &Path) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        fs.open(path, OpenMode::Read)?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    #[test]
    fn recovery_replays_partially_applied_commit_and_is_idempotent() -> Result<()> {
        let fs = MemoryFs::new();
        let root = fs.prepare_root(Path::new("/state"))?;
        fixture(&fs, &root)?;
        fs.open(&root.join("a.sqlite"), OpenMode::CreateNew)?
            .write_all(&[11; BLOCK_SIZE])?;
        fs.open(&root.join("b.sqlite"), OpenMode::CreateNew)?
            .write_all(&[0; BLOCK_SIZE])?;
        recover(&fs, &root)?;
        recover(&fs, &root)?;
        assert_eq!(read(&fs, &root.join("a.sqlite"))?, [11; BLOCK_SIZE]);
        assert_eq!(read(&fs, &root.join("b.sqlite"))?, [22; BLOCK_SIZE]);
        assert!(
            matches!(fs.metadata(&root.join(COMMITTED)), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        );
        Ok(())
    }
    #[test]
    fn corrupt_journal_is_retained_without_modifying_any_base() -> Result<()> {
        let fs = MemoryFs::new();
        let root = fs.prepare_root(Path::new("/state"))?;
        fixture(&fs, &root)?;
        fs.open(&root.join("a.sqlite"), OpenMode::CreateNew)?
            .write_all(b"original")?;
        fs.open(&root.join(COMMITTED), OpenMode::Update)?
            .write_all(b"corrupt!")?;
        assert!(matches!(recover(&fs, &root), Err(Error::CorruptJournal(_))));
        assert_eq!(read(&fs, &root.join("a.sqlite"))?, b"original");
        assert!(fs.metadata(&root.join(COMMITTED)).is_ok());
        Ok(())
    }
    #[test]
    fn incomplete_preparation_is_discarded() -> Result<()> {
        let fs = MemoryFs::new();
        let root = fs.prepare_root(Path::new("/state"))?;
        fs.open(&root.join(PENDING), OpenMode::CreateNew)?
            .write_all(b"partial")?;
        recover(&fs, &root)?;
        assert!(
            matches!(fs.metadata(&root.join(PENDING)), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        );
        Ok(())
    }
}

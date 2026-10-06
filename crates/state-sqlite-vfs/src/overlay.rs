//! Byte-range I/O over lazily materialized 4 KiB blocks.
use crate::{
    Error, Result,
    fs::{File, FileSystem, OpenMode},
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(crate) const BLOCK_SIZE: usize = 4096;
pub(crate) const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_FILES: usize = 1024;
pub(crate) type Block = Box<[u8; BLOCK_SIZE]>;

pub(crate) struct OverlayFile {
    base: Option<Box<dyn File>>,
    // Truncation permanently hides the old suffix, even if later writes extend
    // this overlay. Replay must shrink the base to this boundary before growing.
    pub(crate) base_visible_len: u64,
    pub(crate) len: u64,
    pub(crate) blocks: BTreeMap<u64, Block>,
    pub(crate) dirty: bool,
}

impl OverlayFile {
    pub(crate) fn empty() -> Self {
        Self {
            base: None,
            base_visible_len: 0,
            len: 0,
            blocks: BTreeMap::new(),
            dirty: true,
        }
    }

    pub(crate) fn load(fs: &dyn FileSystem, path: &Path) -> Result<Self> {
        let metadata = fs.metadata(path)?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "managed database must be a regular file",
            )
            .into());
        }
        let base = fs.open(path, OpenMode::Read)?;
        let len = base.len()?;
        if len > MAX_FILE_BYTES {
            return Err(Error::Limit("database file size"));
        }
        Ok(Self {
            base: Some(base),
            base_visible_len: len,
            len,
            blocks: BTreeMap::new(),
            dirty: false,
        })
    }

    fn read_base(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        buf.fill(0);
        let available = self
            .base_visible_len
            .saturating_sub(offset)
            .min(buf.len() as u64) as usize;
        if available != 0 {
            let base = self.base.as_mut().ok_or(Error::Aborted)?;
            base.seek(SeekFrom::Start(offset))?;
            let destination = buf.get_mut(..available).ok_or(Error::Aborted)?;
            base.read_exact(destination)?;
        }
        Ok(())
    }

    /// Zero fills the entire buffer, then reads only bytes inside the logical EOF.
    /// Returns whether the requested range was complete (SQLite short-read rule).
    pub(crate) fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<bool> {
        buf.fill(0);
        let count = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
        let mut copied = 0;
        while copied < count {
            let position = offset + copied as u64;
            let block_offset = position / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
            let within = (position - block_offset) as usize;
            let amount = (BLOCK_SIZE - within).min(count - copied);
            let destination = buf.get_mut(copied..copied + amount).ok_or(Error::Aborted)?;
            if let Some(block) = self.blocks.get(&block_offset) {
                let source = block.get(within..within + amount).ok_or(Error::Aborted)?;
                destination.copy_from_slice(source);
            } else {
                self.read_base(position, destination)?;
            }
            copied += amount;
        }
        Ok(count == buf.len())
    }

    /// Preflight the entire allocation before mutating any block.
    pub(crate) fn write(&mut self, offset: u64, bytes: &[u8], budget: usize) -> Result<usize> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Limit("file offset"))?;
        if end > MAX_FILE_BYTES {
            return Err(Error::Limit("database file size"));
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        let first = offset / BLOCK_SIZE as u64;
        let last = (end - 1) / BLOCK_SIZE as u64;
        let required = (first..=last)
            .filter(|index| !self.blocks.contains_key(&(index * BLOCK_SIZE as u64)))
            .count()
            * BLOCK_SIZE;
        if required > budget {
            return Err(Error::Limit("dirty block memory"));
        }
        let mut copied = 0;
        while copied < bytes.len() {
            let position = offset + copied as u64;
            let block_offset = position / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
            if !self.blocks.contains_key(&block_offset) {
                let mut block = Box::new([0; BLOCK_SIZE]);
                self.read_base(block_offset, block.as_mut())?;
                self.blocks.insert(block_offset, block);
            }
            let within = (position - block_offset) as usize;
            let amount = (BLOCK_SIZE - within).min(bytes.len() - copied);
            let destination = self
                .blocks
                .get_mut(&block_offset)
                .and_then(|block| block.get_mut(within..within + amount))
                .ok_or(Error::Aborted)?;
            let source = bytes.get(copied..copied + amount).ok_or(Error::Aborted)?;
            destination.copy_from_slice(source);
            copied += amount;
        }
        self.len = self.len.max(end);
        self.dirty = true;
        Ok(required)
    }

    pub(crate) fn truncate(&mut self, len: u64) -> Result<usize> {
        if len > MAX_FILE_BYTES {
            return Err(Error::Limit("database file size"));
        }
        let previous = self.blocks.len();
        if len < self.len {
            self.base_visible_len = self.base_visible_len.min(len);
            self.blocks.retain(|offset, _| *offset < len);
            let block_offset = len / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
            if let Some(block) = self.blocks.get_mut(&block_offset) {
                let tail = block
                    .get_mut((len - block_offset) as usize..)
                    .ok_or(Error::Aborted)?;
                tail.fill(0);
            }
        }
        self.len = len;
        self.dirty = true;
        Ok((previous - self.blocks.len()) * BLOCK_SIZE)
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        self.blocks.len() * BLOCK_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_blocks_short_reads_and_truncate_then_extend() -> Result<()> {
        use std::io::Write;
        let fs = crate::fs::MemoryFs::new();
        let root = fs.prepare_root(Path::new("/state"))?;
        let path = root.join("base");
        fs.open(&path, OpenMode::CreateNew)?
            .write_all(&vec![7; BLOCK_SIZE * 3])?;
        let mut file = OverlayFile::load(&fs, &path)?;
        assert_eq!(
            file.write(BLOCK_SIZE as u64 - 2, &[1, 2, 3, 4], MAX_MEMORY_BYTES)?,
            BLOCK_SIZE * 2
        );
        let mut read = [0; 6];
        assert!(file.read(BLOCK_SIZE as u64 - 3, &mut read)?);
        assert_eq!(read, [7, 1, 2, 3, 4, 7]);
        file.truncate(BLOCK_SIZE as u64 + 1)?;
        file.truncate((BLOCK_SIZE * 3) as u64)?;
        let mut read = [9; 4];
        assert!(file.read(BLOCK_SIZE as u64, &mut read)?);
        assert_eq!(read, [3, 0, 0, 0]);
        let mut suffix = [9; 4];
        assert!(!file.read(file.len - 1, &mut suffix)?);
        assert_eq!(suffix, [0; 4]);
        let mut original = Vec::new();
        fs.open(&path, OpenMode::Read)?.read_to_end(&mut original)?;
        assert_eq!(original, vec![7; BLOCK_SIZE * 3]);
        Ok(())
    }

    #[test]
    fn memory_limit_rejects_entire_write_before_changes() -> Result<()> {
        let mut file = OverlayFile::empty();
        assert!(matches!(
            file.write(0, &[1; BLOCK_SIZE + 1], BLOCK_SIZE),
            Err(Error::Limit(_))
        ));
        assert_eq!(file.len, 0);
        assert!(file.blocks.is_empty());
        Ok(())
    }
}

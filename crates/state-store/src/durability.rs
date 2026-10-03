//! Filesystem durability shared by catalog initialization and snapshot publication.
use crate::Result;
use std::{fs::File, path::Path};

pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

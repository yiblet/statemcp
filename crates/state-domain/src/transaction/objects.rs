//! Content-addressed virtual file bytes shared with pinned function sources.
use super::Transaction;
use crate::{Error, Result, content::ContentHash};
impl Transaction {
    pub(super) fn bytes(&self, hash: &ContentHash) -> Result<Vec<u8>> {
        if let Some(bytes) = self.objects.get(hash) {
            return Ok(bytes.clone());
        }
        let catalog = self.backing.connect(&crate::store::catalog::name()?)?;
        let bytes = self
            .loader
            .load(&crate::content::CatalogContent(&catalog), hash)?;
        Ok(bytes.to_vec())
    }
    pub(super) fn put_bytes(&mut self, bytes: Vec<u8>) -> Result<ContentHash> {
        if bytes.len() > state_contract::transfer::MAX_UPLOAD_BYTES {
            return Err(Error::limit("virtual file exceeds 10 MiB"));
        }
        let key = ContentHash::of(&bytes);
        self.objects.insert(key.clone(), bytes);
        Ok(key)
    }
}

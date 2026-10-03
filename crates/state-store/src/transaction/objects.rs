//! Content-addressed virtual file bytes shared with pinned function sources.
use super::Transaction;
use crate::{Error, Result, identity::hash};
impl Transaction {
    pub(super) fn bytes(&self, hash: &str) -> Result<Vec<u8>> {
        if let Some(bytes) = self.objects.get(hash) {
            return Ok(bytes.clone());
        }
        self.store.object(hash)
    }
    pub(super) fn put_bytes(&mut self, bytes: Vec<u8>) -> Result<String> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(Error::limit("virtual file exceeds 8 MiB"));
        }
        let key = hash(&bytes);
        self.objects.insert(key.clone(), bytes);
        Ok(key)
    }
}

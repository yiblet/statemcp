//! Virtual POSIX file operations over immutable content-addressed objects.
use super::Transaction;
use crate::{Error, Result, validation::virtual_path};
use std::collections::BTreeMap;

impl Transaction {
    /// Read pinned Python source without constructing a JSON file response.
    pub fn file_text(&self, namespace: &str, path: &str) -> Result<String> {
        let namespace = self.namespace_identity(namespace)?;
        let path = virtual_path(path)?;
        let hash = self
            .staged_namespace(namespace)?
            .manifest
            .files
            .get(&path)
            .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "file does not exist"))?;
        String::from_utf8(self.bytes(hash)?)
            .map_err(|_| Error::invalid("function source must be UTF-8"))
    }
    /// Snapshot Python files for namespace-local imports with a cumulative byte cap.
    pub fn python_sources(
        &self,
        namespace: &str,
        max_bytes: usize,
    ) -> Result<BTreeMap<String, String>> {
        let namespace = self.namespace_identity(namespace)?;
        let mut sources = BTreeMap::new();
        let mut total = 0usize;
        for (path, hash) in &self.staged_namespace(namespace)?.manifest.files {
            if !path.ends_with(".py") {
                continue;
            }
            let bytes = self.bytes(hash)?;
            total = total
                .checked_add(path.len())
                .and_then(|n| n.checked_add(bytes.len()))
                .ok_or_else(|| {
                    Error::new(
                        crate::ErrorCode::LimitExceeded,
                        "module source byte limit exceeded",
                    )
                })?;
            if total > max_bytes {
                return Err(Error::new(
                    crate::ErrorCode::LimitExceeded,
                    "module source byte limit exceeded",
                ));
            }
            let source = String::from_utf8(bytes)
                .map_err(|_| Error::invalid("Python module source must be UTF-8"))?;
            sources.insert(path.clone(), source);
        }
        Ok(sources)
    }
}

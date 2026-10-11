//! Virtual POSIX file operations over immutable content-addressed objects.
use super::Transaction;
use crate::{Error, Result, validation::virtual_path};
use std::collections::BTreeMap;

impl Transaction {
    /// Read virtual file bytes without constructing a text/base64 protocol response.
    /// Invalid paths and absent files return recoverable errors.
    pub fn file_bytes(&self, namespace: &str, path: &str) -> Result<Vec<u8>> {
        let namespace = self.namespace_identity(namespace)?;
        let path = virtual_path(path)?;
        let hash = self
            .staged_namespace(namespace)?
            .manifest
            .files
            .get(&path)
            .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "file does not exist"))?;
        self.bytes(hash)
    }
    /// Read pinned Python source without constructing a JSON file response.
    pub fn file_text(&self, namespace: &str, path: &str) -> Result<String> {
        String::from_utf8(self.file_bytes(namespace, path)?)
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

impl Transaction {
    /// Resolve every file path in the current namespace snapshot without reading contents.
    pub fn file_paths(
        &self,
        namespace: &str,
    ) -> Result<Vec<state_contract::transfer::TransferPath>> {
        let id = self.namespace_identity(namespace)?;
        self.staged_namespace(id)?
            .manifest
            .files
            .keys()
            .map(|path| state_contract::transfer::TransferPath::parse(path).map_err(Into::into))
            .collect()
    }
    /// Stage raw upload bytes without a base64 round trip. Errors poison publication.
    pub fn write_file_bytes(
        &mut self,
        namespace: &str,
        path: &state_contract::transfer::TransferPath,
        bytes: Vec<u8>,
    ) -> Result<state_contract::file::FileWritten> {
        self.operation(|tx| {
            let id = tx.namespace_for_change(namespace, None)?;
            let files = &tx.staged_namespace(&id)?.manifest.files;
            if files.keys().any(|other| {
                other
                    .strip_prefix(path.as_str())
                    .is_some_and(|tail| tail.starts_with('/'))
                    || path
                        .as_str()
                        .strip_prefix(other)
                        .is_some_and(|tail| tail.starts_with('/'))
            }) {
                return Err(Error::invalid("file collides with a directory"));
            }
            let size = bytes.len();
            let hash = tx.put_bytes(bytes)?;
            let ns = tx.staged_namespace_mut(&id)?;
            ns.manifest.files.insert(path.as_str().to_owned(), hash);
            ns.dirty = true;
            Ok(state_contract::file::FileWritten {
                path: path.as_str().to_owned(),
                size,
            })
        })
    }
}

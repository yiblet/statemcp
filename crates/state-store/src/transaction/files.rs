//! Virtual POSIX file operations over immutable content-addressed objects.
use super::Transaction;
use crate::{
    Error, Result,
    validation::{required, virtual_path},
};
use crate::{FileAction, FileRequest};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::collections::BTreeMap;

impl Transaction {
    /// Read pinned Python source without constructing a JSON file response.
    pub fn file_text(&self, namespace: &str, path: &str) -> Result<String> {
        let namespace = self.namespace_identity(namespace)?;
        let path = virtual_path(path)?;
        let hash = self.namespaces[namespace]
            .manifest
            .files
            .get(&path)
            .ok_or_else(|| Error::new("NOT_FOUND", "file does not exist"))?;
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
        for (path, hash) in &self.namespaces[namespace].manifest.files {
            if !path.ends_with(".py") {
                continue;
            }
            let bytes = self.bytes(hash)?;
            total = total
                .checked_add(path.len())
                .and_then(|n| n.checked_add(bytes.len()))
                .ok_or_else(|| Error::new("LIMIT_EXCEEDED", "module source byte limit exceeded"))?;
            if total > max_bytes {
                return Err(Error::new(
                    "LIMIT_EXCEEDED",
                    "module source byte limit exceeded",
                ));
            }
            let source = String::from_utf8(bytes)
                .map_err(|_| Error::invalid("Python module source must be UTF-8"))?;
            sources.insert(path.clone(), source);
        }
        Ok(sources)
    }
    pub(super) fn file(&mut self, args: &FileRequest) -> Result<Value> {
        let action = args.action;
        let key = self.selected(&args.namespace)?;
        self.check_revision(&key, args.expected_revision.as_deref())?;
        let path = virtual_path(&args.path)?;
        let files = &self.namespaces[&key].manifest.files;
        match action {
            FileAction::List => {
                let prefix = if path == "/" {
                    "/".into()
                } else {
                    format!("{path}/")
                };
                let mut entries = BTreeMap::new();
                for (p, hash) in files {
                    if let Some(tail) = p.strip_prefix(&prefix) {
                        let first = tail.split('/').next().expect("split");
                        let full = format!("{prefix}{first}");
                        entries.entry(full.clone()).or_insert_with(||json!({"path":full,"kind":if tail.contains('/') {"directory"} else {"file"},"hash":if tail.contains('/') {Value::Null} else {json!(hash)}}));
                    }
                }
                if path != "/" && entries.is_empty() {
                    return Err(Error::new("NOT_FOUND", "directory does not exist"));
                }
                Ok(json!({"entries":entries.into_values().collect::<Vec<_>>()}))
            }
            FileAction::Read | FileAction::Stat => {
                if let Some(h) = files.get(&path) {
                    let bytes = self.bytes(h)?;
                    let mut result = json!({"path":path,"kind":"file","hash":h,"size":bytes.len()});
                    if action == FileAction::Read {
                        match String::from_utf8(bytes.clone()) {
                            Ok(text) => result["text"] = json!(text),
                            Err(_) => result["base64"] = json!(STANDARD.encode(bytes)),
                        }
                    }
                    Ok(result)
                } else if action == FileAction::Stat
                    && (path == "/" || files.keys().any(|p| p.starts_with(&format!("{path}/"))))
                {
                    Ok(json!({"path":path,"kind":"directory"}))
                } else {
                    Err(Error::new("NOT_FOUND", "file does not exist"))
                }
            }
            FileAction::Write | FileAction::Append => {
                if path == "/"
                    || files.keys().any(|p| {
                        p.starts_with(&format!("{path}/")) || path.starts_with(&format!("{p}/"))
                    })
                {
                    return Err(Error::invalid("file collides with a directory"));
                }
                if args.text.is_some() && args.base64.is_some() {
                    return Err(Error::invalid("provide text or base64, not both"));
                }
                let mut bytes = if action == FileAction::Append {
                    if let Some(h) = files.get(&path) {
                        self.bytes(h)?
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                if let Some(text) = &args.text {
                    bytes.extend_from_slice(text.as_bytes());
                } else {
                    bytes.extend(
                        STANDARD
                            .decode(required(args.base64.as_deref(), "base64")?)
                            .map_err(|_| Error::invalid("invalid base64"))?,
                    );
                }
                let size = bytes.len();
                let hash = self.put_bytes(bytes)?;
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.files.insert(path.clone(), hash.clone());
                ns.dirty = true;
                Ok(json!({"path":path,"hash":hash,"size":size}))
            }
            FileAction::Delete => {
                let mut removed = Vec::new();
                if files.contains_key(&path) {
                    removed.push(path.clone());
                } else if args.recursive {
                    let prefix = if path == "/" {
                        "/".into()
                    } else {
                        format!("{path}/")
                    };
                    removed.extend(files.keys().filter(|p| p.starts_with(&prefix)).cloned());
                }
                if removed.is_empty() {
                    return Err(Error::new(
                        "NOT_FOUND",
                        "file does not exist (directory deletion requires recursive=true)",
                    ));
                }
                let ns = self.namespaces.get_mut(&key).expect("selected");
                for p in &removed {
                    ns.manifest.files.remove(p);
                }
                ns.dirty = true;
                Ok(json!({"deleted":removed.len()}))
            }
            FileAction::Move | FileAction::Copy => {
                let destination =
                    virtual_path(required(args.destination.as_deref(), "destination")?)?;
                let hash = files
                    .get(&path)
                    .cloned()
                    .ok_or_else(|| Error::new("NOT_FOUND", "file does not exist"))?;
                if destination == "/"
                    || files.contains_key(&destination)
                    || files.keys().any(|p| {
                        p.starts_with(&format!("{destination}/"))
                            || destination.starts_with(&format!("{p}/"))
                    })
                {
                    return Err(Error::new(
                        "ALREADY_EXISTS",
                        "destination exists or collides with directory",
                    ));
                }
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.files.insert(destination.clone(), hash.clone());
                if action == FileAction::Move {
                    ns.manifest.files.remove(&path);
                }
                ns.dirty = true;
                Ok(json!({"path":destination,"hash":hash}))
            }
        }
    }
}

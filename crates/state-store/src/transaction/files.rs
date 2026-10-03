//! Virtual POSIX file operations over immutable content-addressed objects.
use super::Transaction;
use crate::{
    Error, Result,
    validation::{required, virtual_path},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::collections::BTreeMap;

impl Transaction {
    pub(super) fn file(&mut self, args: &Value) -> Result<Value> {
        let key = self.selected(args)?;
        self.check_revision(&key, args)?;
        let action = required(args, "action")?;
        let path = virtual_path(args.get("path").and_then(Value::as_str).unwrap_or("/"))?;
        let files = &self.namespaces[&key].manifest.files;
        match action {
            "list" => {
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
            "read" | "stat" => {
                if let Some(h) = files.get(&path) {
                    let bytes = self.bytes(h)?;
                    let mut result = json!({"path":path,"kind":"file","hash":h,"size":bytes.len()});
                    if action == "read" {
                        match String::from_utf8(bytes.clone()) {
                            Ok(text) => result["text"] = json!(text),
                            Err(_) => result["base64"] = json!(STANDARD.encode(bytes)),
                        }
                    }
                    Ok(result)
                } else if action == "stat"
                    && (path == "/" || files.keys().any(|p| p.starts_with(&format!("{path}/"))))
                {
                    Ok(json!({"path":path,"kind":"directory"}))
                } else {
                    Err(Error::new("NOT_FOUND", "file does not exist"))
                }
            }
            "write" | "append" => {
                if path == "/"
                    || files.keys().any(|p| {
                        p.starts_with(&format!("{path}/")) || path.starts_with(&format!("{p}/"))
                    })
                {
                    return Err(Error::invalid("file collides with a directory"));
                }
                if args.get("text").is_some() && args.get("base64").is_some() {
                    return Err(Error::invalid("provide text or base64, not both"));
                }
                let mut bytes = if action == "append" {
                    if let Some(h) = files.get(&path) {
                        self.bytes(h)?
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                if let Some(text) = args.get("text") {
                    bytes.extend_from_slice(
                        text.as_str()
                            .ok_or_else(|| Error::invalid("text must be a string"))?
                            .as_bytes(),
                    );
                } else {
                    bytes.extend(
                        STANDARD
                            .decode(required(args, "base64")?)
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
            "delete" => {
                let mut removed = Vec::new();
                if files.contains_key(&path) {
                    removed.push(path.clone());
                } else if args.get("recursive").and_then(Value::as_bool) == Some(true) {
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
            "move" | "copy" => {
                let destination = virtual_path(required(args, "destination")?)?;
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
                if action == "move" {
                    ns.manifest.files.remove(&path);
                }
                ns.dirty = true;
                Ok(json!({"path":destination,"hash":hash}))
            }
            _ => Err(Error::invalid("unknown file action")),
        }
    }
}

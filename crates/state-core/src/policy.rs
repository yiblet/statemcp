use crate::{Error, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub(crate) enum Access {
    Owner,
    Endpoint(EndpointAccess),
}
#[derive(Clone)]
pub(crate) struct EndpointAccess {
    pub namespace: String,
    pub databases: BTreeMap<String, (String, String)>,
    pub files: BTreeMap<String, String>,
    pub calls: BTreeSet<(String, String)>,
}
impl EndpointAccess {
    pub fn from_declaration(namespace: String, declaration: &Value) -> Result<Self> {
        let mut databases = BTreeMap::new();
        if let Some(grants) = declaration["databases"].as_object() {
            for (name, mode) in grants {
                let id = declaration["database_ids"][name].as_str().ok_or_else(|| {
                    Error::new("CORRUPT_STORE", "missing pinned database identity")
                })?;
                databases.insert(
                    name.clone(),
                    (mode.as_str().unwrap_or("").into(), id.into()),
                );
            }
        }
        let files = declaration["files"]
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(path, mode)| (path.clone(), mode.as_str().unwrap_or("").into()))
                    .collect()
            })
            .unwrap_or_default();
        let calls = declaration["calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| {
                        let target = call["namespace"].as_str().unwrap_or("");
                        (
                            if target == "self" {
                                namespace.clone()
                            } else {
                                target.into()
                            },
                            call["function"].as_str().unwrap_or("").into(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            namespace,
            databases,
            files,
            calls,
        })
    }
    pub fn file(&self, path: &str, write: bool) -> Result<()> {
        let path = normalize_path(path)?;
        if self.files.iter().any(|(prefix, mode)| {
            (!write || mode == "write")
                && (prefix == "/" || path == *prefix || path.starts_with(&format!("{prefix}/")))
        }) {
            Ok(())
        } else {
            Err(Error::denied())
        }
    }
}
pub(crate) fn normalize_path(path: &str) -> Result<String> {
    if !path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        return Err(Error::invalid("expected absolute virtual POSIX path"));
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(Error::invalid("parent traversal is forbidden")),
            _ => parts.push(part),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

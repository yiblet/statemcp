use crate::{Error, Result};
use serde_json::Value;
use state_store::{DatabaseAccess, FileAccess, Grants};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub(crate) enum Access {
    Owner,
    Endpoint(EndpointAccess),
}
#[derive(Clone)]
pub(crate) struct EndpointAccess {
    pub namespace: String,
    pub databases: BTreeMap<String, (DatabaseAccess, String)>,
    pub files: BTreeMap<String, FileAccess>,
    pub calls: BTreeSet<(String, String)>,
}
impl EndpointAccess {
    pub fn from_declaration(namespace: String, declaration: &Value) -> Result<Self> {
        let grants = Grants::from_metadata(declaration)?;
        let mut databases = BTreeMap::new();
        for grant in grants.databases {
            let id = declaration["database_ids"][&grant.database]
                .as_str()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "missing pinned database identity"))?;
            databases.insert(grant.database, (grant.access, id.into()));
        }
        let files = grants
            .files
            .into_iter()
            .map(|grant| (grant.path, grant.access))
            .collect();
        let calls = grants
            .calls
            .into_iter()
            .map(|grant| {
                (
                    if grant.namespace == "self" {
                        namespace.clone()
                    } else {
                        grant.namespace
                    },
                    grant.function,
                )
            })
            .collect();
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
            (!write || *mode == FileAccess::Write)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_persisted_grants_keep_their_permissions_and_database_pins() {
        let access = EndpointAccess::from_declaration(
            "ns".into(),
            &json!({
                "databases":{"app":"read"},"database_ids":{"app":"db-id"},
                "files":{"/notes":"read"},"calls":[{"namespace":"self","function":"notify"}]
            }),
        )
        .unwrap();
        assert_eq!(
            access.databases["app"],
            (DatabaseAccess::Read, "db-id".into())
        );
        assert!(access.file("/notes/file", false).is_ok());
        assert!(access.file("/notes/file", true).is_err());
        assert!(access.file("/notes-other/file", false).is_err());
        assert!(access.calls.contains(&("ns".into(), "notify".into())));
        assert!(
            EndpointAccess::from_declaration("ns".into(), &json!({"databases":{"app":"admin"}}))
                .is_err()
        );
    }
}

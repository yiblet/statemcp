//! Stored endpoint records become policy facts at the persistence boundary.
use crate::{Error, Result, policy::EndpointAccess};
use state_store::{FunctionDeclaration, VirtualPath};
use std::collections::BTreeMap;

impl EndpointAccess {
    pub fn from_declaration(namespace: String, declaration: &FunctionDeclaration) -> Result<Self> {
        let grants = declaration.grants.clone();
        let mut databases = BTreeMap::new();
        for grant in grants.databases {
            let id = declaration
                .database_ids
                .get(&grant.database)
                .ok_or_else(|| Error::new("CORRUPT_STORE", "missing pinned database identity"))?;
            databases.insert(grant.database, (grant.access, id.into()));
        }
        let files = grants
            .files
            .into_iter()
            .map(|grant| VirtualPath::parse(&grant.path).map(|path| (path, grant.access)))
            .collect::<state_store::Result<_>>()?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn legacy_record() -> serde_json::Value {
        json!({"name":"f", "file":"/api.py", "symbol":"f", "source":"def f(): pass", "source_hash":"hash", "abi_version":1, "version":"old-version",
            "databases":{"app":"read"}, "database_ids":{"app":"db-id"}, "files":{"/notes":"read"}, "calls":[{"namespace":"self","function":"notify"}]})
    }
    use state_store::{DatabaseAccess, FileAccess};
    #[test]
    fn legacy_persisted_grants_keep_their_permissions_and_database_pins() {
        let declaration: FunctionDeclaration = serde_json::from_value(legacy_record()).unwrap();
        let access = EndpointAccess::from_declaration("ns".into(), &declaration).unwrap();
        assert_eq!(
            access.databases["app"],
            (DatabaseAccess::Read, "db-id".into())
        );
        assert_eq!(
            access.files[&VirtualPath::parse("/notes").unwrap()],
            FileAccess::Read
        );
        assert!(access.calls.contains(&("ns".into(), "notify".into())));
        let mut invalid = legacy_record();
        invalid["databases"] = json!({"app":"admin"});
        assert!(serde_json::from_value::<FunctionDeclaration>(invalid).is_err());
    }
}

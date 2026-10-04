//! Typed endpoint permissions shared by publication and runtime enforcement.
use crate::{
    Error, Result,
    validation::{name, virtual_path},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseAccess {
    Read,
    Write,
    Migrate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAccess {
    Read,
    Write,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseGrant {
    pub database: String,
    pub access: DatabaseAccess,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileGrant {
    pub path: String,
    pub access: FileAccess,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallGrant {
    pub namespace: String,
    pub function: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    #[serde(default)]
    pub databases: Vec<DatabaseGrant>,
    #[serde(default)]
    pub files: Vec<FileGrant>,
    #[serde(default)]
    pub calls: Vec<CallGrant>,
}
impl Grants {
    pub fn normalize(&mut self) -> Result<()> {
        let mut databases = BTreeSet::new();
        for grant in &self.databases {
            name(&grant.database)?;
            if !databases.insert(&grant.database) {
                return Err(Error::invalid("duplicate database grant"));
            }
        }
        let mut paths = BTreeSet::new();
        for grant in &mut self.files {
            grant.path = virtual_path(&grant.path)?;
            if !paths.insert(grant.path.clone()) {
                return Err(Error::invalid("duplicate file grant"));
            }
        }
        let mut calls = BTreeSet::new();
        for grant in &self.calls {
            name(&grant.namespace)?;
            name(&grant.function)?;
            if !calls.insert((&grant.namespace, &grant.function)) {
                return Err(Error::invalid("duplicate call grant"));
            }
        }
        self.databases.sort_by(|a, b| a.database.cmp(&b.database));
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        self.calls
            .sort_by(|a, b| (&a.namespace, &a.function).cmp(&(&b.namespace, &b.function)));
        Ok(())
    }
}

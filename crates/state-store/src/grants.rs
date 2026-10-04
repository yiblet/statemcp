//! Typed endpoint permissions shared by publication and runtime enforcement.
use crate::{
    Error, Result,
    validation::{name, virtual_path},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseGrant {
    pub database: String,
    pub access: DatabaseAccess,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileGrant {
    pub path: String,
    pub access: FileAccess,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallGrant {
    pub namespace: String,
    pub function: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    pub fn from_arguments(args: &Value) -> Result<Self> {
        let mut grants: Self = serde_json::from_value(json!({
            "databases":args.get("databases").cloned().unwrap_or(json!([])),
            "files":args.get("files").cloned().unwrap_or(json!([])),
            "calls":args.get("calls").cloned().unwrap_or(json!([]))
        }))
        .map_err(|error| Error::invalid(format!("invalid grants: {error}")))?;
        grants.normalize()?;
        Ok(grants)
    }

    /// Read older persisted maps without changing pinned versions or requiring a migration.
    pub fn from_metadata(metadata: &Value) -> Result<Self> {
        let mut value = metadata.clone();
        for (field, resource) in [("databases", "database"), ("files", "path")] {
            if let Some(map) = value[field].as_object() {
                value[field] = Value::Array(
                    map.iter()
                        .map(|(key, access)| {
                            let mut entry = json!({"access":access});
                            entry[resource] = json!(key);
                            entry
                        })
                        .collect(),
                );
            }
        }
        Self::from_arguments(&value).map_err(|error| Error::new("CORRUPT_STORE", error.message))
    }

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

    pub fn write_to(&self, value: &mut Value) {
        value["databases"] = json!(self.databases);
        value["files"] = json!(self.files);
        value["calls"] = json!(self.calls);
    }
}

pub(crate) fn inspected_metadata(mut value: Value) -> Result<Value> {
    Grants::from_metadata(&value)?.write_to(&mut value);
    Ok(value)
}

//! Endpoint declarations pin source bytes, database identities, and versioned metadata.
use super::Transaction;
use crate::identity::hash;
use crate::{
    Error, Result,
    validation::{name, required, virtual_path},
};
use crate::{Grants, grants::inspected_metadata};
use serde_json::{Value, json};

impl Transaction {
    pub(super) fn function(&mut self, args: &Value) -> Result<Value> {
        let key = self.selected(args)?;
        self.check_revision(&key, args)?;
        let action = required(args, "action")?;
        if action == "list" {
            return Ok(
                json!({"functions":self.namespaces[&key].manifest.functions.values().cloned().map(inspected_metadata).collect::<Result<Vec<_>>>()?}),
            );
        }
        let function_name = name(required(args, "name")?)?;
        let existing = self.namespaces[&key].manifest.functions.get(&function_name);
        if let Some(expected) = args.get("expected_version")
            && existing.and_then(|f| f.get("version")) != Some(expected)
        {
            return Err(Error::new("CONFLICT", "function version does not match"));
        }
        match action {
            "get" => existing
                .cloned()
                .ok_or_else(|| Error::new("NOT_FOUND", "function does not exist"))
                .and_then(inspected_metadata),
            "remove" => {
                if existing.is_none() {
                    return Err(Error::new("NOT_FOUND", "function does not exist"));
                }
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.functions.remove(&function_name);
                ns.dirty = true;
                Ok(json!({"removed":true}))
            }
            "declare" | "update" => {
                let file = virtual_path(required(args, "file")?)?;
                let source_hash = self.namespaces[&key]
                    .manifest
                    .files
                    .get(&file)
                    .cloned()
                    .ok_or_else(|| Error::new("NOT_FOUND", "source file does not exist"))?;
                let source = String::from_utf8(self.bytes(&source_hash)?)
                    .map_err(|_| Error::invalid("function source must be UTF-8"))?;
                required(args, "symbol")?;
                let mut metadata = args
                    .as_object()
                    .cloned()
                    .ok_or_else(|| Error::invalid("declaration must be an object"))?;
                for field in [
                    "action",
                    "namespace",
                    "expected_version",
                    "expected_revision",
                    "version",
                ] {
                    metadata.remove(field);
                }
                let mut database_ids = serde_json::Map::new();
                let grants = Grants::from_arguments(args)?;
                for grant in &grants.databases {
                    let name = &grant.database;
                    let db = self.namespaces[&key]
                        .manifest
                        .databases
                        .get(name)
                        .ok_or_else(|| {
                            Error::new("NOT_FOUND", format!("database grant {name} does not exist"))
                        })?;
                    database_ids.insert(name.clone(), json!(db.id));
                }
                for (field, value) in serde_json::to_value(&grants)?.as_object().expect("grants") {
                    metadata.insert(field.clone(), value.clone());
                }
                metadata.insert("database_ids".into(), json!(database_ids));
                metadata.insert("file".into(), json!(file));
                metadata.insert("source".into(), json!(source));
                metadata.insert("source_hash".into(), json!(source_hash));
                metadata.insert("abi_version".into(), json!(1));
                let version = hash(&serde_json::to_vec(&metadata)?);
                metadata.insert("version".into(), json!(version));
                let metadata = Value::Object(metadata);
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest
                    .functions
                    .insert(function_name, metadata.clone());
                ns.dirty = true;
                Ok(metadata)
            }
            _ => Err(Error::invalid("unknown function action")),
        }
    }
}

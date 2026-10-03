//! Namespace identity, lookup, revision guards, rename, and cheap copy.
use super::Transaction;
use crate::{
    Error, Result,
    identity::id,
    model::{Manifest, Namespace},
    validation::{name, required},
};
use serde_json::{Value, json};

impl Transaction {
    fn namespace_id(&self, selector: &str) -> Result<String> {
        self.namespaces
            .get(selector)
            .filter(|n| !n.deleted)
            .or_else(|| {
                self.namespaces
                    .values()
                    .find(|n| !n.deleted && n.name == selector)
            })
            .map(|n| n.id.clone())
            .ok_or_else(|| Error::new("NOT_FOUND", format!("namespace {selector} does not exist")))
    }
    pub(super) fn selected(&self, args: &Value) -> Result<String> {
        self.namespace_id(required(args, "namespace")?)
    }
    pub(super) fn check_revision(&self, ns: &str, args: &Value) -> Result<()> {
        if let Some(expected) = args.get("expected_revision")
            && expected.as_str() != Some(&self.namespaces[ns].revision)
        {
            return Err(Error::new("CONFLICT", "namespace revision does not match"));
        }
        Ok(())
    }
    fn namespace_info(ns: &Namespace) -> Value {
        json!({"id":ns.id,"name":ns.name,"revision":ns.revision,"files":ns.manifest.files.len(),"databases":ns.manifest.databases.len(),"functions":ns.manifest.functions.len()})
    }
    pub(super) fn namespace(&mut self, args: &Value) -> Result<Value> {
        match required(args, "action")? {
            "list" => Ok(
                json!({"namespaces":self.namespaces.values().filter(|n|!n.deleted).map(Self::namespace_info).collect::<Vec<_>>()}),
            ),
            "create" => {
                let name = name(required(args, "name")?)?;
                self.available(&name)?;
                let ns = Namespace {
                    id: id(),
                    name,
                    revision: String::new(),
                    deleted: false,
                    manifest: Manifest::default(),
                    dirty: true,
                };
                let result = Self::namespace_info(&ns);
                self.namespaces.insert(ns.id.clone(), ns);
                Ok(result)
            }
            action => {
                let key = self.selected(args)?;
                self.check_revision(&key, args)?;
                match action {
                    "get" => Ok(Self::namespace_info(&self.namespaces[&key])),
                    "update" | "rename" => {
                        let new_name = name(required(args, "name")?)?;
                        if self.namespaces[&key].name != new_name {
                            self.available(&new_name)?;
                        }
                        let ns = self.namespaces.get_mut(&key).expect("selected");
                        ns.name = new_name;
                        ns.dirty = true;
                        Ok(Self::namespace_info(ns))
                    }
                    "copy" => {
                        let new_name = name(required(args, "name")?)?;
                        self.available(&new_name)?;
                        let mut ns = self.namespaces[&key].clone();
                        self.freeze_databases(&key, &mut ns.manifest)?;
                        ns.id = id();
                        ns.name = new_name;
                        // An unchanged committed copy keeps the exact root reference.
                        let result = Self::namespace_info(&ns);
                        ns.dirty = true;
                        self.namespaces.insert(ns.id.clone(), ns);
                        Ok(result)
                    }
                    "delete" => {
                        let ns = self.namespaces.get_mut(&key).expect("selected");
                        ns.deleted = true;
                        ns.dirty = true;
                        Ok(json!({"deleted":true,"id":key}))
                    }
                    _ => Err(Error::invalid("unknown namespace action")),
                }
            }
        }
    }
    fn available(&self, name: &str) -> Result<()> {
        if self
            .namespaces
            .values()
            .any(|n| n.name == name || n.id == name)
        {
            Err(Error::new(
                "ALREADY_EXISTS",
                format!("namespace name {name} is reserved"),
            ))
        } else {
            Ok(())
        }
    }
}

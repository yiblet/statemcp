//! Namespace identity, lookup, revision guards, rename, and cheap copy.
use super::Transaction;
use crate::{
    Error, Result,
    identity::id,
    model::{Manifest, Namespace},
    validation::{name, required},
};
use crate::{NamespaceAction, NamespaceRequest};
use serde_json::{Value, json};

impl Transaction {
    /// Resolve a namespace selector against this transaction's current facts.
    pub fn namespace_identity(&self, selector: &str) -> Result<&str> {
        self.namespaces
            .get(selector)
            .filter(|n| !n.deleted)
            .or_else(|| {
                self.namespaces
                    .values()
                    .find(|n| !n.deleted && n.name == selector)
            })
            .map(|n| n.id.as_str())
            .ok_or_else(|| Error::new("NOT_FOUND", format!("namespace {selector} does not exist")))
    }
    pub(super) fn selected(&self, namespace: &str) -> Result<String> {
        self.namespace_identity(namespace).map(str::to_owned)
    }
    pub(super) fn check_revision(&self, ns: &str, expected: Option<&str>) -> Result<()> {
        if let Some(expected) = expected
            && expected != self.namespaces[ns].revision
        {
            return Err(Error::new("CONFLICT", "namespace revision does not match"));
        }
        Ok(())
    }
    fn namespace_info(ns: &Namespace) -> Value {
        json!({"id":ns.id,"name":ns.name,"revision":ns.revision,"files":ns.manifest.files.len(),"databases":ns.manifest.databases.len(),"functions":ns.manifest.functions.len()})
    }
    pub(super) fn namespace(&mut self, args: &NamespaceRequest) -> Result<Value> {
        match args.action {
            NamespaceAction::List => Ok(
                json!({"namespaces":self.namespaces.values().filter(|n|!n.deleted).map(Self::namespace_info).collect::<Vec<_>>()}),
            ),
            NamespaceAction::Create => {
                let name = name(required(args.name.as_deref(), "name")?)?;
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
                let key = self.selected(required(args.namespace.as_deref(), "namespace")?)?;
                self.check_revision(&key, args.expected_revision.as_deref())?;
                match action {
                    NamespaceAction::Get => Ok(Self::namespace_info(&self.namespaces[&key])),
                    NamespaceAction::Update => {
                        let new_name = name(required(args.name.as_deref(), "name")?)?;
                        if self.namespaces[&key].name != new_name {
                            self.available(&new_name)?;
                        }
                        let ns = self.namespaces.get_mut(&key).expect("selected");
                        ns.name = new_name;
                        ns.dirty = true;
                        Ok(Self::namespace_info(ns))
                    }
                    NamespaceAction::Copy => {
                        let new_name = name(required(args.name.as_deref(), "name")?)?;
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
                    NamespaceAction::Delete => {
                        let ns = self.namespaces.get_mut(&key).expect("selected");
                        ns.deleted = true;
                        ns.dirty = true;
                        Ok(json!({"deleted":true,"id":key}))
                    }
                    NamespaceAction::Create | NamespaceAction::List => {
                        unreachable!("handled above")
                    }
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

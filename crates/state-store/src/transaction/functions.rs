//! Endpoint declarations pin source bytes, database identities, and versioned metadata.
use super::Transaction;
use crate::FunctionDeclaration;
use crate::identity::hash;
use crate::{
    Error, Result,
    validation::{name, required, virtual_path},
};
use crate::{FunctionAction, FunctionRequest};
use serde_json::{Value, json};
use std::collections::BTreeMap;

impl Transaction {
    pub fn function_declaration(
        &self,
        namespace: &str,
        function_name: &str,
        expected_version: Option<&str>,
    ) -> Result<&FunctionDeclaration> {
        let namespace = self.namespace_identity(namespace)?;
        let function_name = name(function_name)?;
        let declaration = self.namespaces[namespace]
            .manifest
            .functions
            .get(&function_name);
        if let Some(expected) = expected_version
            && declaration.map(|record| record.version.as_str()) != Some(expected)
        {
            return Err(Error::new("CONFLICT", "function version does not match"));
        }
        declaration.ok_or_else(|| Error::new("NOT_FOUND", "function does not exist"))
    }
    pub fn function_declarations(
        &self,
        namespace: &str,
    ) -> Result<impl Iterator<Item = &FunctionDeclaration>> {
        let namespace = self.namespace_identity(namespace)?;
        Ok(self.namespaces[namespace].manifest.functions.values())
    }
    pub(super) fn function(&mut self, args: &FunctionRequest) -> Result<Value> {
        let action = args.action;
        let key = self.selected(&args.namespace)?;
        self.check_revision(&key, args.expected_revision.as_deref())?;
        if action == FunctionAction::List {
            return Ok(
                json!({"functions":self.namespaces[&key].manifest.functions.values().collect::<Vec<_>>()}),
            );
        }
        let function_name = name(required(args.name.as_deref(), "name")?)?;
        let existing = self.namespaces[&key].manifest.functions.get(&function_name);
        if let Some(expected) = args.expected_version.as_deref()
            && existing.map(|f| f.version.as_str()) != Some(expected)
        {
            return Err(Error::new("CONFLICT", "function version does not match"));
        }
        match action {
            FunctionAction::Get => existing
                .ok_or_else(|| Error::new("NOT_FOUND", "function does not exist"))
                .and_then(|function| serde_json::to_value(function).map_err(Into::into)),
            FunctionAction::Remove => {
                if existing.is_none() {
                    return Err(Error::new("NOT_FOUND", "function does not exist"));
                }
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.functions.remove(&function_name);
                ns.dirty = true;
                Ok(json!({"removed":true}))
            }
            FunctionAction::Declare | FunctionAction::Update => {
                let file = virtual_path(required(args.file.as_deref(), "file")?)?;
                let source_hash = self.namespaces[&key]
                    .manifest
                    .files
                    .get(&file)
                    .cloned()
                    .ok_or_else(|| Error::new("NOT_FOUND", "source file does not exist"))?;
                let source = String::from_utf8(self.bytes(&source_hash)?)
                    .map_err(|_| Error::invalid("function source must be UTF-8"))?;
                required(args.symbol.as_deref(), "symbol")?;
                let mut grants = args.grants.clone();
                grants.normalize()?;
                let mut database_ids = BTreeMap::new();
                for grant in &grants.databases {
                    let db = self.namespaces[&key]
                        .manifest
                        .databases
                        .get(&grant.database)
                        .ok_or_else(|| {
                            Error::new(
                                "NOT_FOUND",
                                format!("database grant {} does not exist", grant.database),
                            )
                        })?;
                    database_ids.insert(grant.database.clone(), db.id.clone());
                }
                let mut metadata = FunctionDeclaration {
                    name: function_name.clone(),
                    file,
                    symbol: args.symbol.clone().expect("required symbol"),
                    description: args.description.clone(),
                    input_schema: args.input_schema.clone(),
                    output_schema: args.output_schema.clone(),
                    grants,
                    database_ids,
                    source,
                    source_hash,
                    abi_version: 1,
                    version: String::new(),
                };
                // Hash the canonical wire record to preserve existing function versions.
                metadata.version = hash(&serde_json::to_vec(&serde_json::to_value(&metadata)?)?);
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest
                    .functions
                    .insert(function_name, metadata.clone());
                ns.dirty = true;
                serde_json::to_value(metadata).map_err(Into::into)
            }
            FunctionAction::List => unreachable!("handled above"),
        }
    }
}

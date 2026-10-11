//! Endpoint declarations pin source bytes and versioned metadata.
use super::Transaction;
use crate::FunctionDeclaration;
use crate::{Error, Result, validation::name};

impl Transaction {
    pub fn function_declaration(
        &self,
        namespace: &str,
        function_name: &str,
        expected_version: Option<&str>,
    ) -> Result<FunctionDeclaration> {
        let namespace = self.namespace_identity(namespace)?;
        let function_name = name(function_name)?;
        let declaration = self
            .staged_namespace(namespace)?
            .manifest
            .functions
            .get(&function_name);
        if let Some(expected) = expected_version
            && declaration.map(|record| record.version()) != Some(expected)
        {
            return Err(Error::new(
                crate::ErrorCode::Conflict,
                "function version does not match",
            ));
        }
        let declaration = declaration
            .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "function does not exist"))?;
        let catalog = self.backing.connect(&crate::store::catalog::name()?)?;
        declaration.resolve(&mut crate::store::records::SourceResolver::new(
            &catalog,
            self.loader.as_ref(),
        ))
    }
    pub fn function_declarations(
        &self,
        namespace: &str,
    ) -> Result<impl Iterator<Item = state_contract::function::FunctionSummary>> {
        let namespace = self.namespace_identity(namespace)?;
        Ok(self
            .staged_namespace(namespace)?
            .manifest
            .functions
            .values()
            .map(crate::model::FunctionRecord::summary))
    }
}

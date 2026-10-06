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
    ) -> Result<&FunctionDeclaration> {
        let namespace = self.namespace_identity(namespace)?;
        let function_name = name(function_name)?;
        let declaration = self
            .staged_namespace(namespace)?
            .manifest
            .functions
            .get(&function_name);
        if let Some(expected) = expected_version
            && declaration.map(|record| record.version.as_str()) != Some(expected)
        {
            return Err(Error::new(
                crate::ErrorCode::Conflict,
                "function version does not match",
            ));
        }
        declaration.ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "function does not exist"))
    }
    pub fn function_declarations(
        &self,
        namespace: &str,
    ) -> Result<impl Iterator<Item = &FunctionDeclaration>> {
        let namespace = self.namespace_identity(namespace)?;
        Ok(self
            .staged_namespace(namespace)?
            .manifest
            .functions
            .values())
    }
}

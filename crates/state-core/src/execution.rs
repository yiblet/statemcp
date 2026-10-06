use crate::{
    CoreLimits, Error, Result, RuntimeBackend, bounded, policy::Access, remaining, schemas,
};
use serde_json::Value;
use state_domain::Transaction;
use state_runtime::ModuleSources;
use std::{sync::Arc, time::Instant};

pub(crate) struct Root {
    pub tx: Transaction,
    pub backend: Arc<dyn RuntimeBackend>,
    pub limits: CoreLimits,
    pub start: Instant,
    pub failure: Option<Error>,
    calls: usize,
    depth: usize,
    stdout_bytes: usize,
}
impl Root {
    pub fn new(tx: Transaction, backend: Arc<dyn RuntimeBackend>, limits: CoreLimits) -> Self {
        Self {
            tx,
            backend,
            limits,
            start: Instant::now(),
            failure: None,
            calls: 0,
            depth: 0,
            stdout_bytes: 0,
        }
    }
    pub fn check(&self) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        remaining(self.start, &self.limits.runtime).map(|_| ())
    }
    pub(crate) fn start_operation(&mut self) -> Result<()> {
        self.check()?;
        self.calls += 1;
        if self.calls > self.limits.runtime.max_calls {
            return Err(Error::limit("root operation budget exceeded"));
        }
        Ok(())
    }
    pub(crate) fn check_file_readable(&mut self, namespace: &str, path: &str) -> Result<()> {
        let readable = self.tx.read_file(&crate::inputs::FsReadInput {
            namespace: namespace.to_owned(),
            path: path.to_owned(),
        })?;
        bounded(&readable, self.limits.max_result_bytes)
    }
    fn namespace_id(&self, selector: &str) -> Result<String> {
        self.tx
            .namespace_identity(selector)
            .map(str::to_owned)
            .map_err(Into::into)
    }
    pub(crate) fn validate_function(
        &mut self,
        namespace: &str,
        file: &str,
        symbol: &str,
        input: &mut Option<crate::inputs::EndpointSchema>,
        output: &mut Option<crate::inputs::EndpointSchema>,
    ) -> Result<()> {
        let input = input.get_or_insert_with(|| {
            crate::inputs::EndpointSchema::Object(serde_json::Map::from_iter([(
                "type".into(),
                Value::String("object".into()),
            )]))
        });
        schemas::compile(input)?;
        let output = output.get_or_insert(crate::inputs::EndpointSchema::Boolean(true));
        schemas::compile(output)?;
        let source = self.tx.file_text(namespace, file)?;
        let limits = remaining(self.start, &self.limits.runtime)?;
        let modules = ModuleSources {
            entry_path: file.to_owned(),
            files: self.tx.python_sources(namespace, limits.max_source_bytes)?,
        };
        self.backend
            .validate_module(&source, &modules, symbol, &limits)?;
        self.check()
    }
    fn enter(&mut self) -> Result<()> {
        if self.depth >= self.limits.max_depth {
            return Err(Error::limit("root endpoint/script depth exceeded"));
        }
        self.depth += 1;
        Ok(())
    }
    fn account_stdout(&mut self, stdout: &str) -> Result<()> {
        self.stdout_bytes = self.stdout_bytes.saturating_add(stdout.len());
        if self.stdout_bytes > self.limits.runtime.max_output_bytes {
            return Err(Error::limit("root diagnostic output budget exceeded"));
        }
        Ok(())
    }
    pub(crate) fn call_function(
        &mut self,
        selector: &str,
        function: &str,
        arguments: serde_json::Map<String, Value>,
        expected_version: Option<&str>,
    ) -> Result<Value> {
        let namespace = self.namespace_id(selector)?;
        // The interpreter may mutate this transaction during host calls. Its running declaration
        // must retain the pinned source and modules independently of those edits.
        let declaration = self
            .tx
            .function_declaration(&namespace, function, expected_version)?
            .clone();
        if declaration.abi_version != 1 {
            return Err(Error::new(
                crate::ErrorCode::AbiMismatch,
                format!(
                    "runtime ABI mismatch: expected 1, actual {}; redeclare the function for this runtime",
                    declaration.abi_version
                ),
            ));
        }
        let arguments = Value::Object(arguments);
        schemas::compile(
            declaration.input_schema.as_ref().ok_or_else(|| {
                Error::new(crate::ErrorCode::CorruptStore, "missing input schema")
            })?,
        )?
        .validate(&arguments)
        .map_err(|e| {
            Error::new(
                crate::ErrorCode::SchemaValidation,
                format!("input schema: {e}"),
            )
        })?;
        let Value::Object(arguments) = arguments else {
            return Err(Error::invalid("endpoint arguments must be an object"));
        };
        let access = Access::Namespace(namespace.clone());
        self.enter()?;
        let limits = remaining(self.start, &self.limits.runtime)?;
        let backend = self.backend.clone();
        let modules = ModuleSources {
            entry_path: declaration.file.clone(),
            files: declaration.modules,
        };
        let result = backend.invoke(
            &declaration.source,
            &modules,
            &declaration.symbol,
            arguments,
            &limits,
            &mut |name, positional, keywords| {
                self.host(name, positional, keywords, &access, Some(&namespace))
                    .map_err(Into::into)
            },
        );
        self.depth -= 1;
        let result = result?;
        self.check()?;
        self.account_stdout(&result.stdout)?;
        schemas::compile(
            declaration.output_schema.as_ref().ok_or_else(|| {
                Error::new(crate::ErrorCode::CorruptStore, "missing output schema")
            })?,
        )?
        .validate(&result.value)
        .map_err(|e| {
            Error::new(
                crate::ErrorCode::SchemaValidation,
                format!("output schema: {e}"),
            )
        })?;
        Ok(result.value)
    }
    pub(crate) fn execute_script(
        &mut self,
        script: &str,
        selector: Option<&str>,
        inputs: Value,
        access: &Access,
    ) -> Result<state_runtime::RunResult> {
        let namespace = selector.map(|name| self.namespace_id(name)).transpose()?;
        self.enter()?;
        let limits = remaining(self.start, &self.limits.runtime)?;
        let backend = self.backend.clone();
        let modules = ModuleSources {
            entry_path: "/state.py".into(),
            files: namespace
                .as_deref()
                .map(|ns| self.tx.python_sources(ns, limits.max_source_bytes))
                .transpose()?
                .unwrap_or_default(),
        };
        let result = backend.execute(
            script,
            &modules,
            inputs,
            &limits,
            &mut |name, positional, keywords| {
                self.host(name, positional, keywords, access, namespace.as_deref())
                    .map_err(Into::into)
            },
        );
        self.depth -= 1;
        let result = result?;
        self.check()?;
        self.account_stdout(&result.stdout)?;
        Ok(result)
    }
}

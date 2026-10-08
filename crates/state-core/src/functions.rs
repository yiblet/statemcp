//! Publication and lookup of pinned function definitions.
use crate::{Context, Result, Runtime, Sources, schemas};
use serde_json::Value;
use state_contract::function::*;
use state_runtime::ModuleSources;

/// Function declarations with source and interpreter dependencies supplied explicitly.
#[derive(Clone)]
pub struct Functions {
    sources: Sources,
    runtime: Runtime,
}
impl Functions {
    /// Construct publication and lookup services using the configured interpreter.
    pub fn new(sources: Sources, runtime: Runtime) -> Self {
        Self { sources, runtime }
    }
    pub(crate) fn validate_function(
        &self,
        context: &mut Context,
        namespace: &str,
        file: &str,
        symbol: &str,
        input: &mut Option<state_contract::function::EndpointSchema>,
        output: &mut Option<state_contract::function::EndpointSchema>,
    ) -> Result<FunctionSources> {
        let input = input.get_or_insert_with(|| {
            state_contract::function::EndpointSchema::Object(serde_json::Map::from_iter([(
                "type".into(),
                Value::String("object".into()),
            )]))
        });
        schemas::compile(input)?;
        let output = output.get_or_insert(state_contract::function::EndpointSchema::Boolean(true));
        schemas::compile(output)?;
        let limits = context.runtime_limits()?;
        let snapshot = self
            .sources
            .snapshot(namespace, file, limits.max_source_bytes, context)?;
        let modules = ModuleSources {
            entry_path: file.to_owned(),
            files: snapshot.modules,
        };
        self.runtime
            .validate_module(&snapshot.source, &modules, symbol, &limits)?;
        context.check()?;
        Ok(FunctionSources {
            source: snapshot.source,
            modules: modules.files,
        })
    }
    /// List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts, without source code. Use function.get with namespace and name for a full declaration.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn list_functions(
        &self,
        mut input: FunctionListInput,
        context: &mut Context,
    ) -> Result<state_contract::function::Functions> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().list_functions(&input)?)
        })
    }
    /// Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn get_function(
        &self,
        mut input: FunctionGetInput,
        context: &mut Context,
    ) -> Result<FunctionDeclaration> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().get_function(&input)?)
        })
    }
    /// Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn remove_function(
        &self,
        mut input: FunctionRemoveInput,
        context: &mut Context,
    ) -> Result<FunctionRemoved> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().remove_function(&input)?)
        })
    }
    /// Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions can access all files, databases, and functions in their own namespace only. Source bytes are pinned at publication. Returns name, version, and published status, with commit revisions supplied by the controller.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn declare_function(
        &self,
        mut input: FunctionDeclareInput,
        context: &mut Context,
    ) -> Result<FunctionPublished> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            let sources = self.validate_function(
                context,
                &input.namespace,
                &input.file,
                &input.symbol,
                &mut input.input_schema,
                &mut input.output_schema,
            )?;
            Ok(context.transaction().declare_function(&input, sources)?)
        })
    }
    /// Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete declaration. Functions retain full access to their own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn update_function(
        &self,
        mut input: FunctionUpdateInput,
        context: &mut Context,
    ) -> Result<FunctionPublished> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            let sources = self.validate_function(
                context,
                &input.namespace,
                &input.file,
                &input.symbol,
                &mut input.input_schema,
                &mut input.output_schema,
            )?;
            Ok(context.transaction().update_function(&input, sources)?)
        })
    }
    pub(crate) fn contracts(
        &self,
        namespace: &str,
        context: &mut Context,
    ) -> Result<state_contract::function::Functions> {
        let id = context.namespace(namespace)?;
        Ok(context.transaction().list_functions(&FunctionListInput {
            namespace: id.as_str().to_owned(),
        })?)
    }
    pub(crate) fn pinned(
        &self,
        namespace: &str,
        function: &str,
        expected_version: Option<&str>,
        context: &mut Context,
    ) -> Result<FunctionDeclaration> {
        let id = context.namespace(namespace)?;
        // Running code must own a snapshot: callbacks may replace its declaration and sources.
        Ok(context
            .transaction()
            .function_declaration(id.as_str(), function, expected_version)?
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreLimits, Error, Files, Namespaces, test_support};
    use state_contract::{file::*, namespace::*};
    use std::sync::Arc;

    fn declare_input(namespace: &str) -> FunctionDeclareInput {
        FunctionDeclareInput {
            namespace: namespace.into(),
            name: "answer".into(),
            file: "/entry.py".into(),
            symbol: "answer".into(),
            description: None,
            expected_revision: None,
            expected_version: None,
            input_schema: None,
            output_schema: None,
        }
    }
    fn write(namespace: &str, path: &str, text: &str) -> FsWriteInput {
        FsWriteInput::Text(FileTextInput {
            namespace: namespace.into(),
            path: path.into(),
            text: text.into(),
            expected_revision: None,
        })
    }
    fn create(namespace: &str, context: &mut Context) -> Result<()> {
        Namespaces.create_namespace(
            NamespaceCreateInput {
                name: namespace.into(),
                description: DescriptionChange::Unchanged,
            },
            context,
        )?;
        Ok(())
    }

    #[test]
    fn function_service_keeps_pinned_modules() -> Result<()> {
        let fs = state_fs::MemoryFs::new();
        let store = test_support::memory_store(&fs)?;
        let files = Files;
        let functions = Functions::new(Sources::new(Arc::new(files)), Runtime::embedded());
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        create("notes", &mut context)?;
        files.write_file(
            write("notes", "/util.py", "def value():\n    return 7\n"),
            &mut context,
        )?;
        files.write_file(
            write(
                "notes",
                "/entry.py",
                "from util import value\ndef answer():\n    return value()\n",
            ),
            &mut context,
        )?;
        let published = functions.declare_function(declare_input("notes"), &mut context)?;
        files.write_file(
            write("notes", "/util.py", "def value():\n    return 9\n"),
            &mut context,
        )?;
        let declaration = functions.get_function(
            FunctionGetInput {
                namespace: "notes".into(),
                name: "answer".into(),
                expected_version: Some(published.version),
            },
            &mut context,
        )?;
        assert_eq!(
            declaration.modules.get("/util.py").map(String::as_str),
            Some("def value():\n    return 7\n")
        );
        context.finish()?;
        Ok(())
    }

    #[test]
    fn invalid_publication_poisoning_prevents_commit() -> Result<()> {
        let fs = state_fs::MemoryFs::new();
        let store = test_support::memory_store(&fs)?;
        let functions = Functions::new(Sources::new(Arc::new(Files)), Runtime::embedded());
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        create("notes", &mut context)?;
        Files.write_file(
            write("notes", "/entry.py", "def other():\n    return 1\n"),
            &mut context,
        )?;
        assert!(
            functions
                .declare_function(declare_input("notes"), &mut context)
                .is_err()
        );
        assert!(context.finish().is_err());
        let mut tx = store.begin()?;
        assert!(tx.list_namespaces()?.namespaces.is_empty());
        Ok(())
    }

    #[test]
    fn resource_services_enforce_namespace_scope_without_host_adapter() -> Result<()> {
        let fs = state_fs::MemoryFs::new();
        let store = test_support::memory_store(&fs)?;
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        create("own", &mut context)?;
        create("other", &mut context)?;
        context.finish()?;
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        let own = context.namespace("own")?;
        let denied = context.scoped(own, |context| {
            Files.write_file(write("other", "/escape", "bad"), context)
        });
        assert!(matches!(
            denied,
            Err(Error {
                code: crate::ErrorCode::PermissionDenied,
                ..
            })
        ));
        assert!(matches!(
            context.finish(),
            Err(Error {
                code: crate::ErrorCode::PermissionDenied,
                ..
            })
        ));
        Ok(())
    }
    struct FailingSources;
    impl crate::SourceProvider for FailingSources {
        fn snapshot(&self, _: &str, _: &str, _: usize, _: &mut Context) -> Result<FunctionSources> {
            Err(Error::new(
                crate::ErrorCode::NotFound,
                "controlled source failure",
            ))
        }
    }

    #[test]
    fn injected_source_failure_aborts_publication() -> Result<()> {
        let fs = state_fs::MemoryFs::new();
        let store = test_support::memory_store(&fs)?;
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        create("notes", &mut context)?;
        let functions = Functions::new(Sources::new(Arc::new(FailingSources)), Runtime::embedded());
        let failed = functions.declare_function(declare_input("notes"), &mut context);
        assert!(matches!(
            failed,
            Err(Error {
                code: crate::ErrorCode::NotFound,
                ..
            })
        ));
        assert!(context.finish().is_err());
        let mut tx = store.begin()?;
        assert!(tx.list_namespaces()?.namespaces.is_empty());
        Ok(())
    }

    struct FailingValidation;
    impl crate::RuntimeBackend for FailingValidation {
        fn execute(
            &self,
            _: &str,
            _: &state_runtime::ModuleSources,
            _: Value,
            _: &state_runtime::Limits,
            _: &mut state_runtime::HostCallback<'_>,
        ) -> std::result::Result<state_runtime::RunResult, state_runtime::RuntimeError> {
            Err(state_runtime::RuntimeError::new(
                crate::ErrorCode::PythonError,
                "execution is not configured",
            ))
        }
        fn invoke(
            &self,
            _: &str,
            _: &state_runtime::ModuleSources,
            _: &str,
            _: serde_json::Map<String, Value>,
            _: &state_runtime::Limits,
            _: &mut state_runtime::HostCallback<'_>,
        ) -> std::result::Result<state_runtime::RunResult, state_runtime::RuntimeError> {
            Err(state_runtime::RuntimeError::new(
                crate::ErrorCode::PythonError,
                "invocation is not configured",
            ))
        }
        fn validate_module(
            &self,
            _: &str,
            _: &state_runtime::ModuleSources,
            _: &str,
            _: &state_runtime::Limits,
        ) -> std::result::Result<(), state_runtime::RuntimeError> {
            Err(state_runtime::RuntimeError::new(
                crate::ErrorCode::PythonError,
                "controlled validation failure",
            ))
        }
    }

    #[test]
    fn injected_runtime_validation_failure_aborts_publication() -> Result<()> {
        let fs = state_fs::MemoryFs::new();
        let store = test_support::memory_store(&fs)?;
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        create("notes", &mut context)?;
        Files.write_file(
            write("notes", "/entry.py", "def answer():\n    return 7\n"),
            &mut context,
        )?;
        let functions = Functions::new(
            Sources::new(Arc::new(Files)),
            Runtime::new(Arc::new(FailingValidation)),
        );
        let failed = functions.declare_function(declare_input("notes"), &mut context);
        assert!(matches!(
            failed,
            Err(Error {
                code: crate::ErrorCode::PythonError,
                ..
            })
        ));
        assert!(context.finish().is_err());
        let mut tx = store.begin()?;
        assert!(tx.list_namespaces()?.namespaces.is_empty());
        Ok(())
    }
    struct ForeignModuleSources;
    impl crate::SourceProvider for ForeignModuleSources {
        fn snapshot(
            &self,
            namespace: &str,
            file: &str,
            max: usize,
            context: &mut Context,
        ) -> Result<FunctionSources> {
            let mut sources =
                crate::SourceProvider::snapshot(&Files, namespace, file, max, context)?;
            sources.modules.insert(
                "/foreign.py".into(),
                "def surprise():\n    return 99\n".into(),
            );
            Ok(sources)
        }
    }

    #[test]
    fn injected_snapshot_cannot_publish_modules_outside_transaction() -> Result<()> {
        let fs = state_fs::MemoryFs::new();
        let store = test_support::memory_store(&fs)?;
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        create("notes", &mut context)?;
        Files.write_file(
            write("notes", "/entry.py", "def answer():\n    return 7\n"),
            &mut context,
        )?;
        context.finish()?;
        let mut context = Context::new(store.begin()?, CoreLimits::default());
        let functions = Functions::new(
            Sources::new(Arc::new(ForeignModuleSources)),
            Runtime::embedded(),
        );
        let failed = functions.declare_function(declare_input("notes"), &mut context);
        assert!(matches!(
            failed,
            Err(Error {
                code: crate::ErrorCode::InvalidArgument,
                ..
            })
        ));
        assert!(context.finish().is_err());
        let mut tx = store.begin()?;
        assert!(
            tx.list_functions(&FunctionListInput {
                namespace: "notes".into()
            })?
            .functions
            .is_empty()
        );
        Ok(())
    }
}

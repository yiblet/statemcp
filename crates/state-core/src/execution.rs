//! Python execution with host bindings to the same typed primitive services.
use crate::{
    Context, Databases, DiscoveryService, Error, Files, Functions, Namespaces, Result, Runtime,
    schemas,
};
use serde_json::Value;
use state_contract::execution::{CallInput, ExecuteInput, ExecutionResult};
use state_runtime::ModuleSources;

/// Executes code with explicitly supplied feature services and runtime.
#[derive(Clone)]
pub struct Execution {
    api: crate::Api,
    pub(crate) functions: Functions,
    pub(crate) files: Files,
    pub(crate) databases: Databases,
    pub(crate) namespaces: Namespaces,
    pub(crate) discovery: DiscoveryService,
    runtime: Runtime,
}
impl Execution {
    /// Construct an executor. All nested runs use this runtime and the caller's context.
    pub fn new(
        api: crate::Api,
        functions: Functions,
        files: Files,
        databases: Databases,
        namespaces: Namespaces,
        discovery: DiscoveryService,
        runtime: Runtime,
    ) -> Self {
        Self {
            api,
            functions,
            files,
            databases,
            namespaces,
            discovery,
            runtime,
        }
    }
    fn host<'a>(
        &'a self,
        context: &'a mut Context,
        namespace: Option<&'a str>,
    ) -> crate::hosts::Host<'a> {
        crate::hosts::Host {
            namespace,
            api: &self.api,
            executor: self,
            files: &self.files,
            databases: &self.databases,
            namespaces: &self.namespaces,
            functions: &self.functions,
            discovery: &self.discovery,
            context,
        }
    }
    pub(crate) fn invoke_http_handler(
        &self,
        namespace: crate::policy::NamespaceId,
        handler: state_contract::http::Handler,
        request: state_contract::http::HttpRequest,
        context: &mut Context,
    ) -> Result<state_contract::http::HttpResponse> {
        context.scoped(namespace.clone(), |context| {
            context.execution(|context| {
                let limits = context.runtime_limits()?;
                let modules = ModuleSources {
                    entry_path: handler.file,
                    files: handler.sources.modules,
                };
                let result = self
                    .runtime
                    .invoke_http(
                        &handler.sources.source,
                        &modules,
                        &handler.symbol,
                        request,
                        &limits,
                        &mut self.host(context, Some(namespace.as_str())),
                    )
                    .map_err(|error| {
                        if error.code == crate::ErrorCode::InvalidArgument {
                            Error::new(crate::ErrorCode::PythonError, error.message)
                        } else {
                            error.into()
                        }
                    })?;
                context.check()?;
                context.account_stdout(&result.stdout)?;
                Ok(result.response)
            })
        })
    }
    /// Invoke a pinned function, narrowing authority to its established namespace identity.
    pub fn call_function(&self, input: CallInput, context: &mut Context) -> Result<Value> {
        context.operation(|context| {
            self.call_pinned(
                context,
                &input.namespace,
                &input.function,
                input.arguments.unwrap_or_default(),
                input.expected_version.as_deref(),
            )
        })
    }
    /// Run a script with the caller's authority and shared budgets/transaction.
    pub fn execute_script(
        &self,
        input: ExecuteInput,
        context: &mut Context,
    ) -> Result<ExecutionResult> {
        context.operation(|context| {
            let result = self.run_script(
                context,
                &input.script,
                input.namespace.as_deref(),
                input.inputs.unwrap_or(Value::Null),
            )?;
            Ok(ExecutionResult {
                value: result.value,
                stdout: result.stdout,
            })
        })
    }
    fn call_pinned(
        &self,
        context: &mut Context,
        selector: &str,
        function: &str,
        arguments: serde_json::Map<String, Value>,
        expected_version: Option<&str>,
    ) -> Result<Value> {
        let namespace = context.namespace(selector)?;
        // The interpreter may mutate this transaction during host calls. Its running declaration
        // must retain the pinned source and modules independently of those edits.
        let declaration =
            self.functions
                .pinned(namespace.as_str(), function, expected_version, context)?;
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
        context.scoped(namespace.clone(), |context| {
            context.execution(|context| {
                let limits = context.runtime_limits()?;
                let runtime = &self.runtime;
                let modules = ModuleSources {
                    entry_path: declaration.file.clone(),
                    files: declaration.modules,
                };
                let result = runtime.invoke(
                    &declaration.source,
                    &modules,
                    &declaration.symbol,
                    arguments,
                    &limits,
                    &mut self.host(context, Some(namespace.as_str())),
                );
                let result = result?;
                context.check()?;
                context.account_stdout(&result.stdout)?;
                schemas::compile(declaration.output_schema.as_ref().ok_or_else(|| {
                    Error::new(crate::ErrorCode::CorruptStore, "missing output schema")
                })?)?
                .validate(&result.value)
                .map_err(|e| {
                    Error::new(
                        crate::ErrorCode::SchemaValidation,
                        format!("output schema: {e}"),
                    )
                })?;
                Ok(result.value)
            })
        })
    }
    fn run_script(
        &self,
        context: &mut Context,
        script: &str,
        selector: Option<&str>,
        inputs: Value,
    ) -> Result<state_runtime::RunResult> {
        let namespace = selector.map(|name| context.namespace(name)).transpose()?;
        context.execution(|context| {
            let limits = context.runtime_limits()?;
            let runtime = &self.runtime;
            let modules = ModuleSources {
                entry_path: "/state.py".into(),
                files: namespace
                    .as_ref()
                    .map(|ns| self.files.modules(ns.as_str(), context))
                    .transpose()?
                    .unwrap_or_default(),
            };
            let result = runtime.execute(
                script,
                &modules,
                inputs,
                &limits,
                &mut self.host(context, namespace.as_ref().map(|id| id.as_str())),
            );
            let result = result?;
            context.check()?;
            context.account_stdout(&result.stdout)?;
            Ok(result)
        })
    }
}

impl crate::ports::CodeExecutor for Execution {
    fn call_function(&self, input: CallInput, context: &mut Context) -> Result<Value> {
        Execution::call_function(self, input, context)
    }
    fn execute_script(
        &self,
        input: ExecuteInput,
        context: &mut Context,
    ) -> Result<ExecutionResult> {
        Execution::execute_script(self, input, context)
    }
}

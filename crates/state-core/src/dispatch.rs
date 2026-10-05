use crate::{
    CoreLimits, Error, FileAction, FunctionAction, Result, RuntimeBackend, Tool, bounded,
    policy::{self, Access, Target},
    remaining,
    request::Request,
    schemas,
};
use serde_json::{Value, json};
use state_runtime::ModuleSources;
use state_store::{
    Arguments, CallRequest, DescribeRequest, DiscoveryMode, ExecuteRequest, FileRequest,
    FunctionRequest, Transaction,
};
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
    pub fn dispatch(
        &mut self,
        tool: &str,
        args: Value,
        access: &Access,
        top: bool,
    ) -> Result<Value> {
        let result = Request::parse(tool, args)
            .and_then(|request| self.dispatch_request(request, access, top));
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    pub(crate) fn dispatch_typed(
        &mut self,
        tool: Tool,
        args: Value,
        access: &Access,
        top: bool,
    ) -> Result<Value> {
        let result = Request::grouped(tool, args)
            .and_then(|request| self.dispatch_request(request, access, top));
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    pub(crate) fn dispatch_request(
        &mut self,
        request: Request,
        access: &Access,
        top: bool,
    ) -> Result<Value> {
        let result = self.dispatch_inner(request, access, top);
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    fn dispatch_inner(&mut self, request: Request, access: &Access, top: bool) -> Result<Value> {
        let mut args = request.into_arguments();
        let operation = args.operation();
        self.check()?;
        self.calls += 1;
        if self.calls > self.limits.runtime.max_calls {
            return Err(Error::limit("root operation budget exceeded"));
        }
        if !top && args.idempotency_key().is_some() {
            return Err(Error::invalid(
                "idempotency keys are only valid on root calls",
            ));
        }
        if let Access::Namespace(namespace) = access {
            args.resolve_self(namespace);
            if let Arguments::Execute(script) = &mut args
                && script.namespace.is_none()
            {
                script.namespace = Some(namespace.clone());
            }
            self.authorize(&args, namespace)?;
        }
        let result = match &args {
            Arguments::Function(function)
                if matches!(
                    function.action,
                    FunctionAction::Declare | FunctionAction::Update
                ) =>
            {
                self.declare(function.clone())?
            }
            Arguments::Call(call) => self.invoke(call)?,
            Arguments::Execute(script) => self.execute(script, access)?,
            Arguments::Describe(discovery) => self.describe(discovery)?,
            _ => self.tx.dispatch_request(&args)?,
        };
        let result = crate::presentation::tool_result(operation, result);
        // A file write must remain readable through the same JSON boundary, including
        // escaping/base64 overhead; reject and roll back before publication otherwise.
        if let Arguments::File(file) = &args
            && matches!(file.action, FileAction::Write | FileAction::Append)
        {
            let readable = self
                .tx
                .dispatch_request(&Arguments::File(FileRequest::read(
                    file.namespace.clone(),
                    file.path.clone(),
                )))?;
            bounded(
                &readable,
                if top {
                    self.limits.max_result_bytes
                } else {
                    self.limits.runtime.max_output_bytes
                },
            )?;
        }
        self.check()?;
        bounded(
            &result,
            if top {
                self.limits.max_result_bytes
            } else {
                self.limits.runtime.max_output_bytes
            },
        )?;
        Ok(result)
    }
    fn namespace_id(&self, selector: &str) -> Result<String> {
        self.tx
            .namespace_identity(selector)
            .map(str::to_owned)
            .map_err(Into::into)
    }
    fn authorize(&self, args: &Arguments, namespace: &str) -> Result<()> {
        match policy::target(args) {
            Target::ApiDiscovery => Ok(()),
            Target::GlobalState => Err(Error::denied()),
            Target::Namespace(selector) => {
                policy::check_namespace(namespace, self.tx.namespace_identity(selector)?)
            }
        }
    }
    fn declare(&mut self, mut args: FunctionRequest) -> Result<Value> {
        let input_schema = args
            .input_schema
            .get_or_insert_with(|| json!({"type":"object"}));
        schemas::compile(input_schema)?;
        let output_schema = args.output_schema.get_or_insert(Value::Bool(true));
        schemas::compile(output_schema)?;
        let file = args.file.as_ref().expect("parsed declaration file");
        let source = self.tx.file_text(&args.namespace, file)?;
        let limits = remaining(self.start, &self.limits.runtime)?;
        let modules = ModuleSources {
            entry_path: file.clone(),
            files: self
                .tx
                .python_sources(&args.namespace, limits.max_source_bytes)?,
        };
        self.backend.validate_module(
            &source,
            &modules,
            args.symbol.as_deref().expect("parsed symbol"),
            &limits,
        )?;
        self.check()?;
        self.tx
            .dispatch_request(&Arguments::Function(args))
            .map_err(Into::into)
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
    fn invoke(&mut self, args: &CallRequest) -> Result<Value> {
        let namespace = self.namespace_id(&args.namespace)?;
        let declaration = self
            .tx
            .function_declaration(&namespace, &args.function, args.expected_version.as_deref())?
            .clone();
        if declaration.abi_version != 1 {
            return Err(Error::new(
                "ABI_MISMATCH",
                format!(
                    "runtime ABI mismatch: expected 1, actual {}; redeclare the function for this runtime",
                    declaration.abi_version
                ),
            ));
        }
        let arguments = args.arguments.clone();
        schemas::compile(
            declaration
                .input_schema
                .as_ref()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "missing input schema"))?,
        )?
        .validate(&arguments)
        .map_err(|e| Error::new("SCHEMA_VALIDATION", format!("input schema: {e}")))?;
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
            declaration
                .output_schema
                .as_ref()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "missing output schema"))?,
        )?
        .validate(&result.value)
        .map_err(|e| Error::new("SCHEMA_VALIDATION", format!("output schema: {e}")))?;
        Ok(result.value)
    }
    fn execute(&mut self, args: &ExecuteRequest, access: &Access) -> Result<Value> {
        let namespace = args
            .namespace
            .as_deref()
            .map(|name| self.namespace_id(name))
            .transpose()?;
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
            &args.script,
            &modules,
            args.inputs.clone(),
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
        Ok(json!({"value":result.value,"stdout":result.stdout}))
    }
    fn describe(&mut self, args: &DescribeRequest) -> Result<Value> {
        let Some(selector) = args.namespace.as_deref() else {
            let mut tools = schemas::tool_definitions();
            if let Some(name) = args.tool.as_deref() {
                let tool = tools
                    .into_iter()
                    .find(|tool| tool["name"] == name)
                    .ok_or_else(|| Error::invalid("unknown discovery tool"))?;
                return Ok(json!({"tool":tool}));
            }
            let mode = args.mode;
            if mode == DiscoveryMode::Readme {
                return Ok(
                    json!({"title":"StateMCP README","format":"markdown","text":include_str!("../../../README.md")}),
                );
            }
            if mode == DiscoveryMode::Runtime {
                return Ok(schemas::runtime_guide());
            }
            if mode == DiscoveryMode::Overview {
                for tool in &mut tools {
                    tool.as_object_mut().expect("tool").remove("inputSchema");
                }
            }
            let mut result = json!({"tools":tools,"model":"Namespaces contain virtual files, named SQLite databases, and published Python endpoints.","runtime":"Pydantic Monty","abi_version":1,"discovery":{"readme":{"mode":"readme"},"runtime":{"mode":"runtime"},"schemas":{"mode":"full"},"operation":{"tool":"db.create"}},"identity":"local owner; dispatch_as scopes receipts only","retention":"history and receipts retained until explicit maintenance; no automatic GC","limits":{"root_calls":self.limits.runtime.max_calls,"root_depth":self.limits.max_depth,"root_milliseconds":self.limits.runtime.max_duration.as_millis(),"runtime_json_bytes":self.limits.runtime.max_output_bytes,"direct_result_bytes":self.limits.max_result_bytes}});
            if mode == DiscoveryMode::Full {
                result["runtime_guide"] = schemas::runtime_guide();
            }
            return Ok(result);
        };
        let namespace = self.namespace_id(selector)?;
        let mut functions = Vec::new();
        for declaration in self.tx.function_declarations(&namespace)? {
            let name = &declaration.name;
            if args
                .function
                .as_deref()
                .is_some_and(|requested| requested != name)
            {
                continue;
            }
            functions.push(json!({"name":name,"version":declaration.version,"description":declaration.description,"input_schema":declaration.input_schema,"output_schema":declaration.output_schema}));
        }
        if args.function.is_some() && functions.is_empty() {
            return Err(Error::new("NOT_FOUND", "endpoint contract is unavailable"));
        }
        Ok(json!({"namespace":namespace,"functions":functions}))
    }
}

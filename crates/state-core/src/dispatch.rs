use crate::{
    CoreLimits, Error, Result, RuntimeBackend, bounded,
    policy::{Access, EndpointAccess},
    remaining, schemas,
};
use serde_json::{Value, json};
use state_store::{DatabaseAccess, Grants, Transaction};
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
        let result = schemas::normalize_call(tool, args)
            .and_then(|(tool, args)| self.dispatch_inner(&tool, args, access, top));
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    fn dispatch_inner(
        &mut self,
        tool: &str,
        mut args: Value,
        access: &Access,
        top: bool,
    ) -> Result<Value> {
        self.check()?;
        self.calls += 1;
        if self.calls > self.limits.runtime.max_calls {
            return Err(Error::limit("root operation budget exceeded"));
        }
        schemas::validate_operation(tool, &args)?;
        if !top && args.get("idempotency_key").is_some() {
            return Err(Error::invalid(
                "idempotency keys are only valid on root calls",
            ));
        }
        if let Access::Endpoint(endpoint) = access {
            if args["namespace"] == "self" {
                args["namespace"] = json!(endpoint.namespace);
            }
            self.authorize(tool, &args, endpoint)?;
        }
        let mut result = match tool {
            "state_function" if matches!(args["action"].as_str(), Some("declare" | "update")) => {
                self.declare(args.clone())?
            }
            "state_call" => self.invoke(&args)?,
            "state_execute" => self.execute(&args, access)?,
            "state_describe" => self.describe(&args, access)?,
            _ => self.tx.dispatch(tool, args.clone())?,
        };
        // Keep storage metadata available internally and through explicit inspection.
        if tool == "state_function" {
            match args["action"].as_str() {
                Some("declare" | "update") => {
                    result =
                        json!({"name":result["name"],"version":result["version"],"published":true});
                }
                Some("list") => {
                    for function in result["functions"].as_array_mut().expect("function list") {
                        for field in ["source", "source_hash", "database_ids", "abi_version"] {
                            function
                                .as_object_mut()
                                .expect("function metadata")
                                .remove(field);
                        }
                    }
                }
                _ => {}
            }
        }
        if tool == "state_db" && args["action"] == "list" {
            for database in result["databases"].as_array_mut().expect("database list") {
                database
                    .as_object_mut()
                    .expect("database metadata")
                    .remove("snapshot");
            }
        }
        if tool == "state_db" && args["action"] == "create" {
            result = json!({"name":result["name"],"created":true});
        }
        if tool == "state_fs" && args["action"] != "stat" {
            if let Some(object) = result.as_object_mut() {
                object.remove("hash");
            }
            if let Some(entries) = result["entries"].as_array_mut() {
                for entry in entries {
                    entry.as_object_mut().expect("file entry").remove("hash");
                }
            }
        }
        // A staged namespace has no published revision yet. The root API fills this after commit.
        if tool == "state_namespace"
            && matches!(args["action"].as_str(), Some("create" | "copy" | "update"))
        {
            result
                .as_object_mut()
                .expect("namespace metadata")
                .remove("revision");
        }
        // A file write must remain readable through the same JSON boundary, including
        // escaping/base64 overhead; reject and roll back before publication otherwise.
        if tool == "state_fs" && matches!(args["action"].as_str(), Some("write" | "append")) {
            let readable = self.tx.dispatch(
                "state_fs",
                json!({"action":"read","namespace":args["namespace"],"path":args["path"]}),
            )?;
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
    fn namespace_id(&mut self, selector: &str) -> Result<String> {
        let namespace = self.tx.dispatch(
            "state_namespace",
            json!({"action":"get","namespace":selector}),
        )?;
        Ok(namespace["id"].as_str().expect("store namespace id").into())
    }
    fn authorize(&mut self, tool: &str, args: &Value, endpoint: &EndpointAccess) -> Result<()> {
        if tool == "state_describe" {
            return Ok(());
        }
        if !matches!(tool, "state_call" | "state_db" | "state_fs") {
            return Err(Error::denied());
        }
        let namespace =
            self.namespace_id(args["namespace"].as_str().expect("validated namespace"))?;
        if tool == "state_call" {
            return if endpoint.calls.contains(&(
                namespace,
                args["function"]
                    .as_str()
                    .expect("validated function")
                    .into(),
            )) {
                Ok(())
            } else {
                Err(Error::denied())
            };
        }
        if namespace != endpoint.namespace {
            return Err(Error::denied());
        }
        let action = args["action"].as_str().expect("validated action");
        if tool == "state_fs" {
            let write = !matches!(action, "read" | "stat" | "list" | "copy");
            endpoint.file(args["path"].as_str().unwrap_or("/"), write)?;
            if let Some(destination) = args["destination"].as_str() {
                endpoint.file(destination, true)?;
            }
            return Ok(());
        }
        let name = args["database"].as_str().ok_or_else(Error::denied)?;
        let (mode, id) = endpoint.databases.get(name).ok_or_else(Error::denied)?;
        let allowed = match action {
            "query" | "inspect" | "migrations" => true,
            "execute" => matches!(mode, DatabaseAccess::Write | DatabaseAccess::Migrate),
            "migrate" => *mode == DatabaseAccess::Migrate,
            _ => false,
        };
        if !allowed {
            return Err(Error::denied());
        }
        let databases = self
            .tx
            .dispatch("state_db", json!({"action":"list","namespace":namespace}))?;
        if !databases["databases"]
            .as_array()
            .expect("store database list")
            .iter()
            .any(|db| db["name"] == name && db["id"] == *id)
        {
            return Err(Error::denied());
        }
        Ok(())
    }
    fn declare(&mut self, mut args: Value) -> Result<Value> {
        for (key, default) in [
            ("input_schema", json!({"type":"object"})),
            ("output_schema", json!(true)),
            ("databases", json!([])),
            ("files", json!([])),
            ("calls", json!([])),
        ] {
            if args.get(key).is_none() {
                args[key] = default;
            }
        }
        schemas::compile(&args["input_schema"])?;
        schemas::compile(&args["output_schema"])?;
        let mut grants = Grants::from_arguments(&args)?;
        for call in &mut grants.calls {
            if call.namespace != "self" {
                call.namespace = self.namespace_id(&call.namespace)?;
            }
        }
        // Namespace names and UUID aliases must not grant the same call twice.
        grants.normalize()?;
        grants.write_to(&mut args);
        let source = self.tx.dispatch(
            "state_fs",
            json!({"action":"read","namespace":args["namespace"],"path":args["file"]}),
        )?;
        let source = source["text"]
            .as_str()
            .ok_or_else(|| Error::invalid("function source must be UTF-8"))?;
        let limits = remaining(self.start, &self.limits.runtime)?;
        self.backend.validate_module(
            source,
            args["symbol"].as_str().expect("validated symbol"),
            &limits,
        )?;
        self.check()?;
        self.tx.dispatch("state_function", args).map_err(Into::into)
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
    fn invoke(&mut self, args: &Value) -> Result<Value> {
        let namespace =
            self.namespace_id(args["namespace"].as_str().expect("validated namespace"))?;
        let mut lookup = json!({"action":"get","namespace":namespace,"name":args["function"]});
        if let Some(version) = args.get("expected_version") {
            lookup["expected_version"] = version.clone();
        }
        let declaration = self.tx.dispatch("state_function", lookup)?;
        if declaration["abi_version"] != 1 {
            return Err(Error::new(
                "ABI_MISMATCH",
                format!(
                    "runtime ABI mismatch: expected 1, actual {}; redeclare the function for this runtime",
                    declaration["abi_version"]
                ),
            ));
        }
        let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
        schemas::compile(&declaration["input_schema"])?
            .validate(&arguments)
            .map_err(|e| Error::new("SCHEMA_VALIDATION", format!("input schema: {e}")))?;
        let access = Access::Endpoint(EndpointAccess::from_declaration(
            namespace.clone(),
            &declaration,
        )?);
        self.enter()?;
        let limits = remaining(self.start, &self.limits.runtime)?;
        let backend = self.backend.clone();
        let result = backend.invoke(
            declaration["source"].as_str().expect("pinned source"),
            declaration["symbol"].as_str().expect("pinned symbol"),
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
        schemas::compile(&declaration["output_schema"])?
            .validate(&result.value)
            .map_err(|e| Error::new("SCHEMA_VALIDATION", format!("output schema: {e}")))?;
        Ok(result.value)
    }
    fn execute(&mut self, args: &Value, access: &Access) -> Result<Value> {
        let namespace = args["namespace"]
            .as_str()
            .map(|name| self.namespace_id(name))
            .transpose()?;
        self.enter()?;
        let limits = remaining(self.start, &self.limits.runtime)?;
        let backend = self.backend.clone();
        let result = backend.execute(
            args["script"].as_str().expect("validated script"),
            args.get("inputs").cloned().unwrap_or(Value::Null),
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
    fn describe(&mut self, args: &Value, access: &Access) -> Result<Value> {
        let Some(selector) = args["namespace"].as_str() else {
            let mut tools = schemas::tool_definitions();
            if let Some(name) = args["tool"].as_str() {
                let tool = tools
                    .into_iter()
                    .find(|tool| tool["name"] == name)
                    .ok_or_else(|| Error::invalid("unknown discovery tool"))?;
                return Ok(json!({"tool":tool}));
            }
            let mode = args["mode"].as_str().unwrap_or("overview");
            if mode == "readme" {
                return Ok(
                    json!({"title":"StateMCP README","format":"markdown","text":include_str!("../../../README.md")}),
                );
            }
            if mode == "runtime" {
                return Ok(schemas::runtime_guide());
            }
            if mode == "overview" {
                for tool in &mut tools {
                    tool.as_object_mut().expect("tool").remove("inputSchema");
                }
            }
            let mut result = json!({"tools":tools,"model":"Namespaces contain virtual files, named SQLite databases, and published Python endpoints.","runtime":"Pydantic Monty","abi_version":1,"discovery":{"readme":{"mode":"readme"},"runtime":{"mode":"runtime"},"schemas":{"mode":"full"},"operation":{"tool":"db.create"}},"identity":"local owner; dispatch_as scopes receipts only","retention":"history and receipts retained until explicit maintenance; no automatic GC","limits":{"root_calls":self.limits.runtime.max_calls,"root_depth":self.limits.max_depth,"root_milliseconds":self.limits.runtime.max_duration.as_millis(),"runtime_json_bytes":self.limits.runtime.max_output_bytes,"direct_result_bytes":self.limits.max_result_bytes}});
            if mode == "full" {
                result["runtime_guide"] = schemas::runtime_guide();
            }
            return Ok(result);
        };
        let namespace = self.namespace_id(selector)?;
        let result = self.tx.dispatch(
            "state_function",
            json!({"action":"list","namespace":namespace}),
        )?;
        let mut functions = Vec::new();
        for declaration in result["functions"].as_array().expect("store function list") {
            let name = declaration["name"].as_str().expect("function name");
            if args["function"]
                .as_str()
                .is_some_and(|requested| requested != name)
            {
                continue;
            }
            if let Access::Endpoint(endpoint) = access
                && !endpoint.calls.contains(&(namespace.clone(), name.into()))
            {
                continue;
            }
            functions.push(json!({"name":name,"version":declaration["version"],"description":declaration["description"],"input_schema":declaration["input_schema"],"output_schema":declaration["output_schema"]}));
        }
        if args.get("function").is_some() && functions.is_empty() {
            return Err(Error::new("NOT_FOUND", "endpoint contract is unavailable"));
        }
        Ok(json!({"namespace":namespace,"functions":functions}))
    }
}

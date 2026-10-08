//! Decode Python values at the host boundary and pass concrete inputs to domain operations.
use crate::ports::CodeExecutor;
use crate::{
    Context, Databases, DiscoveryService, Error, Files, Functions, Namespaces, Result, bounded,
    selectors::HostFunction,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use state_contract::database::SqlCell;
use state_contract::{database::*, file::*};

/// Consume external Python arguments in signature order, retaining typed field values.
struct HostArguments {
    positional: std::vec::IntoIter<Value>,
    keywords: Map<String, Value>,
}
impl HostArguments {
    fn new(positional: Vec<Value>, keywords: Map<String, Value>) -> Self {
        Self {
            positional: positional.into_iter(),
            keywords,
        }
    }
    fn optional<T: DeserializeOwned>(&mut self, name: &'static str) -> Result<Option<T>> {
        let positional = self.positional.next();
        let keyword = self.keywords.remove(name);
        let value = match (positional, keyword) {
            (Some(_), Some(_)) => {
                return Err(Error::invalid(format!(
                    "host argument {name} supplied more than once"
                )));
            }
            (Some(value), None) | (None, Some(value)) => value,
            (None, None) => return Ok(None),
        };
        serde_json::from_value(value)
            .map(Some)
            .map_err(|e| Error::invalid(format!("host argument {name}: {e}")))
    }
    fn required<T: DeserializeOwned>(&mut self, name: &'static str) -> Result<T> {
        self.optional(name)?
            .ok_or_else(|| Error::invalid(format!("missing host argument {name}")))
    }
    fn finish(mut self) -> Result<()> {
        if self.positional.next().is_some() {
            return Err(Error::invalid("too many host arguments"));
        }
        if let Some((name, _)) = self.keywords.into_iter().next() {
            return Err(Error::invalid(format!("unknown host argument {name}")));
        }
        Ok(())
    }
}
fn encode(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| Error::invalid(e.to_string()))
}
pub(crate) struct Host<'a> {
    pub(crate) executor: &'a dyn CodeExecutor,
    pub(crate) files: &'a Files,
    pub(crate) databases: &'a Databases,
    pub(crate) namespaces: &'a Namespaces,
    pub(crate) functions: &'a Functions,
    pub(crate) discovery: &'a DiscoveryService,
    pub(crate) context: &'a mut Context,
}
impl Host<'_> {
    pub(crate) fn host(
        &mut self,
        name: &str,
        positional: Vec<Value>,
        keywords: Map<String, Value>,
        namespace: Option<&str>,
    ) -> Result<Value> {
        let result = self.host_inner(name, positional, keywords, namespace);
        self.context.record(result)
    }
    fn host_inner(
        &mut self,
        name: &str,
        positional: Vec<Value>,
        keywords: Map<String, Value>,
        namespace: Option<&str>,
    ) -> Result<Value> {
        self.context.check()?;
        #[derive(Serialize)]
        struct WireArguments<'a> {
            args: &'a [Value],
            kwargs: &'a Map<String, Value>,
        }
        bounded(
            &WireArguments {
                args: &positional,
                kwargs: &keywords,
            },
            self.context.limits().runtime.max_output_bytes,
        )?;
        let function =
            HostFunction::parse(name).ok_or_else(|| Error::invalid("unknown host function"))?;
        let mut arguments = HostArguments::new(positional, keywords);
        if function == HostFunction::Mcp {
            let name: String = arguments.required("name")?;
            let fields: Map<String, Value> = arguments.optional("arguments")?.unwrap_or_default();
            arguments.finish()?;
            return self.host_tool(&name, fields);
        }
        let result = match function {
            HostFunction::Call => {
                let namespace: String = arguments.required("namespace")?;
                let function: String = arguments.required("function")?;
                let fields: Map<String, Value> =
                    arguments.optional("arguments")?.unwrap_or_default();
                arguments.finish()?;

                self.executor.call_function(
                    state_contract::execution::CallInput {
                        namespace,
                        function,
                        arguments: Some(fields),
                        expected_version: None,
                        idempotency_key: None,
                    },
                    self.context,
                )?
            }
            HostFunction::DbQuery => {
                let database = arguments.required("database")?;
                let sql = arguments.required("sql")?;
                let params: Vec<SqlCell> = arguments.optional("params")?.unwrap_or_default();
                arguments.finish()?;
                let namespace = namespace
                    .ok_or_else(|| Error::invalid("database helper requires a default namespace"))?
                    .to_owned();
                encode(self.databases.query_database(
                    DbQueryInput {
                        namespace,
                        database,
                        sql,
                        params: Some(params),
                    },
                    self.context,
                )?)?
            }
            HostFunction::DbExecute => {
                let database = arguments.required("database")?;
                let sql = arguments.required("sql")?;
                let params: Vec<SqlCell> = arguments.optional("params")?.unwrap_or_default();
                arguments.finish()?;
                let namespace = namespace
                    .ok_or_else(|| Error::invalid("database helper requires a default namespace"))?
                    .to_owned();
                encode(self.databases.execute_database(
                    DbExecuteInput {
                        namespace,
                        database,
                        sql,
                        params: Some(params),
                        expected_revision: None,
                    },
                    self.context,
                )?)?
            }
            HostFunction::DbInspect => {
                let database = arguments.required("database")?;
                arguments.finish()?;
                let namespace = namespace
                    .ok_or_else(|| Error::invalid("database helper requires a default namespace"))?
                    .to_owned();
                encode(self.databases.inspect_database(
                    DbInspectInput {
                        namespace,
                        database,
                    },
                    self.context,
                )?)?
            }
            HostFunction::ReadText => {
                let path = arguments.required("path")?;
                arguments.finish()?;
                let namespace = namespace
                    .ok_or_else(|| Error::invalid("file helper requires a default namespace"))?
                    .to_owned();
                Value::String(
                    self.files
                        .read_text(FsReadInput { namespace, path }, self.context)?,
                )
            }
            HostFunction::WriteText => {
                let path = arguments.required("path")?;
                let text = arguments.required("text")?;
                arguments.finish()?;
                let namespace = namespace
                    .ok_or_else(|| Error::invalid("file helper requires a default namespace"))?
                    .to_owned();
                let input = FsWriteInput::Text(FileTextInput {
                    namespace,
                    path,
                    text,
                    expected_revision: None,
                });
                let result = self.files.write_file(input, self.context)?;
                encode(result)?
            }
            HostFunction::Mcp => return Err(Error::invalid("mcp call must use its tool boundary")),
        };
        self.context.check()?;
        bounded(&result, self.context.limits().runtime.max_output_bytes)?;
        Ok(result)
    }
}

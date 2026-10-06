//! Decode Python values at the host boundary and pass concrete inputs to domain operations.
use crate::{
    Error, Result, bounded, execution::Root, inputs::*, policy::Access, selectors::HostFunction,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use state_domain::records::{FileContents, SqlCell};

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
impl Root {
    pub(crate) fn host(
        &mut self,
        name: &str,
        positional: Vec<Value>,
        keywords: Map<String, Value>,
        access: &Access,
        namespace: Option<&str>,
    ) -> Result<Value> {
        let result = self.host_inner(name, positional, keywords, access, namespace);
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    fn scoped_namespace(&self, selector: &str, access: &Access) -> Result<String> {
        let selector = match access {
            Access::Namespace(namespace) if selector == "self" => namespace,
            _ => selector,
        };
        let target = self.tx.namespace_identity(selector)?;
        if let Access::Namespace(scope) = access {
            crate::policy::check_namespace(scope, target)?;
        }
        Ok(target.to_owned())
    }
    fn host_inner(
        &mut self,
        name: &str,
        positional: Vec<Value>,
        keywords: Map<String, Value>,
        access: &Access,
        namespace: Option<&str>,
    ) -> Result<Value> {
        self.check()?;
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
            self.limits.runtime.max_output_bytes,
        )?;
        let function =
            HostFunction::parse(name).ok_or_else(|| Error::invalid("unknown host function"))?;
        let mut arguments = HostArguments::new(positional, keywords);
        if function == HostFunction::Mcp {
            let name: String = arguments.required("name")?;
            let fields: Map<String, Value> = arguments.optional("arguments")?.unwrap_or_default();
            arguments.finish()?;
            return self.host_tool(&name, fields, access);
        }
        self.start_operation()?;
        let result = match function {
            HostFunction::Call => {
                let namespace: String = arguments.required("namespace")?;
                let function: String = arguments.required("function")?;
                let fields: Map<String, Value> =
                    arguments.optional("arguments")?.unwrap_or_default();
                arguments.finish()?;
                let namespace = self.scoped_namespace(&namespace, access)?;
                self.call_function(&namespace, &function, fields, None)?
            }
            HostFunction::DbQuery => {
                let database = arguments.required("database")?;
                let sql = arguments.required("sql")?;
                let params: Vec<SqlCell> = arguments.optional("params")?.unwrap_or_default();
                arguments.finish()?;
                let namespace = self.scoped_namespace(
                    namespace.ok_or_else(|| {
                        Error::invalid("database helper requires a default namespace")
                    })?,
                    access,
                )?;
                encode(self.tx.query_database(&DbQueryInput {
                    namespace,
                    database,
                    sql,
                    params: Some(params),
                })?)?
            }
            HostFunction::DbExecute => {
                let database = arguments.required("database")?;
                let sql = arguments.required("sql")?;
                let params: Vec<SqlCell> = arguments.optional("params")?.unwrap_or_default();
                arguments.finish()?;
                let namespace = self.scoped_namespace(
                    namespace.ok_or_else(|| {
                        Error::invalid("database helper requires a default namespace")
                    })?,
                    access,
                )?;
                encode(self.tx.execute_database(&DbExecuteInput {
                    namespace,
                    database,
                    sql,
                    params: Some(params),
                    expected_revision: None,
                })?)?
            }
            HostFunction::DbInspect => {
                let database = arguments.required("database")?;
                arguments.finish()?;
                let namespace = self.scoped_namespace(
                    namespace.ok_or_else(|| {
                        Error::invalid("database helper requires a default namespace")
                    })?,
                    access,
                )?;
                encode(self.tx.inspect_database(&DbInspectInput {
                    namespace,
                    database,
                })?)?
            }
            HostFunction::ReadText => {
                let path = arguments.required("path")?;
                arguments.finish()?;
                let namespace = self.scoped_namespace(
                    namespace.ok_or_else(|| {
                        Error::invalid("file helper requires a default namespace")
                    })?,
                    access,
                )?;
                match self.tx.read_file(&FsReadInput { namespace, path })? {
                    FileContents::Text { text, .. } => Value::String(text),
                    FileContents::Binary { .. } => {
                        return Err(Error::invalid("file is not UTF-8 text"));
                    }
                }
            }
            HostFunction::WriteText => {
                let path = arguments.required("path")?;
                let text = arguments.required("text")?;
                arguments.finish()?;
                let namespace = self.scoped_namespace(
                    namespace.ok_or_else(|| {
                        Error::invalid("file helper requires a default namespace")
                    })?,
                    access,
                )?;
                let input = FsWriteInput::Text(FileTextInput {
                    namespace,
                    path,
                    text,
                    expected_revision: None,
                });
                let result = self.tx.write_file(&input)?;
                let (namespace, path) = match &input {
                    FsWriteInput::Text(file) => (&file.namespace, &file.path),
                    FsWriteInput::Binary(file) => (&file.namespace, &file.path),
                };
                self.check_file_readable(namespace, path)?;
                encode(result)?
            }
            HostFunction::Mcp => return Err(Error::invalid("mcp call must use its tool boundary")),
        };
        self.check()?;
        bounded(&result, self.limits.runtime.max_output_bytes)?;
        Ok(result)
    }
}

//! Decode the dynamic Python mcp tool name at its boundary; execute using concrete inputs and results.
use crate::{
    Error, Result,
    execution::Root,
    inputs::{self, *},
    policy::Access,
};
use serde::Serialize;
use serde_json::Value;

fn encode(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| Error::invalid(e.to_string()))
}
impl Root {
    pub(crate) fn host_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Map<String, Value>,
        access: &Access,
    ) -> Result<Value> {
        self.start_operation()?;
        let mut input = HostTool::parse(name, Value::Object(arguments))?;
        if input.idempotency_key().is_some() {
            return Err(Error::invalid(
                "idempotency keys are only valid on root calls",
            ));
        }
        if let Access::Namespace(namespace) = access {
            input.resolve_self(namespace);
            match input.target() {
                Target::Discovery => {}
                Target::Global => return Err(Error::denied()),
                Target::Namespace(selector) => crate::policy::check_namespace(
                    namespace,
                    self.tx.namespace_identity(selector)?,
                )?,
            }
        }
        let result = match input {
            HostTool::NamespaceCreate(input) => encode(self.tx.create_namespace(&input)?)?,
            HostTool::NamespaceList(_) => encode(self.tx.list_namespaces()?)?,
            HostTool::NamespaceGet(input) => encode(self.tx.get_namespace(&input)?)?,
            HostTool::NamespaceUpdate(input) => encode(self.tx.update_namespace(&input)?)?,
            HostTool::NamespaceCopy(input) => encode(self.tx.copy_namespace(&input)?)?,
            HostTool::NamespaceDelete(input) => encode(self.tx.delete_namespace(&input)?)?,
            HostTool::FsRead(input) => encode(self.tx.read_file(&input)?)?,
            HostTool::FsStat(input) => encode(self.tx.stat_file(&input)?)?,
            HostTool::FsList(input) => encode(self.tx.list_files(&input)?)?,
            HostTool::FsWrite(input) => {
                let result = self.tx.write_file(&input)?;
                let (namespace, path) = match &input {
                    inputs::FsWriteInput::Text(file) => (&file.namespace, &file.path),
                    inputs::FsWriteInput::Binary(file) => (&file.namespace, &file.path),
                };
                self.check_file_readable(namespace, path)?;
                encode(result)?
            }
            HostTool::FsAppend(input) => {
                let result = self.tx.append_file(&input)?;
                let (namespace, path) = match &input.0 {
                    inputs::FsWriteInput::Text(file) => (&file.namespace, &file.path),
                    inputs::FsWriteInput::Binary(file) => (&file.namespace, &file.path),
                };
                self.check_file_readable(namespace, path)?;
                encode(result)?
            }
            HostTool::FsMove(input) => encode(self.tx.move_file(&input)?)?,
            HostTool::FsCopy(input) => encode(self.tx.copy_file(&input)?)?,
            HostTool::FsDelete(input) => encode(self.tx.delete_file(&input)?)?,
            HostTool::DbList(input) => encode(self.tx.list_databases(&input)?)?,
            HostTool::DbCreate(input) => encode(self.tx.create_database(&input)?)?,
            HostTool::DbDrop(input) => encode(self.tx.drop_database(&input)?)?,
            HostTool::DbQuery(input) => encode(self.tx.query_database(&input)?)?,
            HostTool::DbExecute(input) => encode(self.tx.execute_database(&input)?)?,
            HostTool::DbInspect(input) => encode(self.tx.inspect_database(&input)?)?,
            HostTool::DbMigrations(input) => encode(self.tx.database_migrations(&input)?)?,
            HostTool::DbMigrate(input) => encode(self.tx.migrate_database(&input)?)?,
            HostTool::FunctionList(input) => encode(self.tx.list_functions(&input)?)?,
            HostTool::FunctionGet(input) => encode(self.tx.get_function(&input)?)?,
            HostTool::FunctionDeclare(mut input) => {
                self.validate_function(
                    &input.namespace,
                    &input.file,
                    &input.symbol,
                    &mut input.input_schema,
                    &mut input.output_schema,
                )?;
                encode(self.tx.declare_function(&input)?)?
            }
            HostTool::FunctionUpdate(mut input) => {
                self.validate_function(
                    &input.namespace,
                    &input.file,
                    &input.symbol,
                    &mut input.input_schema,
                    &mut input.output_schema,
                )?;
                encode(self.tx.update_function(&input)?)?
            }
            HostTool::FunctionRemove(input) => encode(self.tx.remove_function(&input)?)?,
            HostTool::Call(input) => self.call_function(
                &input.namespace,
                &input.function,
                input.arguments.unwrap_or_default(),
                input.expected_version.as_deref(),
            )?,
            HostTool::Execute(input) => encode(self.execute_script(
                &input.script,
                input.namespace.as_deref(),
                input.inputs.unwrap_or(Value::Null),
                access,
            )?)?,
            HostTool::Describe(input) => encode(self.describe_typed(input)?)?,
        };
        self.check()?;
        crate::bounded(&result, self.limits.runtime.max_output_bytes)?;
        Ok(result)
    }
}
enum Target<'a> {
    Discovery,
    Global,
    Namespace(&'a str),
}
trait Scope {
    fn target(&self) -> Target<'_>;
    fn resolve_self(&mut self, namespace: &str);
    fn idempotency_key(&self) -> Option<&str>;
}
impl Scope for HostTool {
    fn target(&self) -> Target<'_> {
        match self {
            Self::NamespaceCreate(_) => Target::Global,
            Self::NamespaceList(_) => Target::Global,
            Self::NamespaceGet(input) => Target::Namespace(&input.namespace),
            Self::NamespaceUpdate(input) => Target::Namespace(&input.namespace),
            Self::NamespaceCopy(_) => Target::Global,
            Self::NamespaceDelete(input) => Target::Namespace(&input.namespace),
            Self::FsRead(input) => Target::Namespace(&input.namespace),
            Self::FsStat(input) => Target::Namespace(&input.namespace),
            Self::FsList(input) => Target::Namespace(&input.namespace),
            Self::FsWrite(input) => match input {
                inputs::FsWriteInput::Text(input) => Target::Namespace(&input.namespace),
                inputs::FsWriteInput::Binary(input) => Target::Namespace(&input.namespace),
            },
            Self::FsAppend(input) => match &input.0 {
                inputs::FsWriteInput::Text(input) => Target::Namespace(&input.namespace),
                inputs::FsWriteInput::Binary(input) => Target::Namespace(&input.namespace),
            },
            Self::FsMove(input) => Target::Namespace(&input.namespace),
            Self::FsCopy(input) => Target::Namespace(&input.namespace),
            Self::FsDelete(input) => Target::Namespace(&input.namespace),
            Self::DbList(input) => Target::Namespace(&input.namespace),
            Self::DbCreate(input) => Target::Namespace(&input.namespace),
            Self::DbDrop(input) => Target::Namespace(&input.namespace),
            Self::DbQuery(input) => Target::Namespace(&input.namespace),
            Self::DbExecute(input) => Target::Namespace(&input.namespace),
            Self::DbInspect(input) => Target::Namespace(&input.namespace),
            Self::DbMigrations(input) => Target::Namespace(&input.namespace),
            Self::DbMigrate(input) => Target::Namespace(&input.namespace),
            Self::FunctionList(input) => Target::Namespace(&input.namespace),
            Self::FunctionGet(input) => Target::Namespace(&input.namespace),
            Self::FunctionDeclare(input) => Target::Namespace(&input.namespace),
            Self::FunctionUpdate(input) => Target::Namespace(&input.namespace),
            Self::FunctionRemove(input) => Target::Namespace(&input.namespace),
            Self::Call(input) => Target::Namespace(&input.namespace),
            Self::Execute(input) => input
                .namespace
                .as_deref()
                .map_or(Target::Discovery, Target::Namespace),
            Self::Describe(input) => match input {
                inputs::DescribeInput::Namespace(input) => Target::Namespace(&input.namespace),
                _ => Target::Discovery,
            },
        }
    }
    fn resolve_self(&mut self, namespace: &str) {
        fn resolve(selector: &mut String, namespace: &str) {
            if selector == "self" {
                namespace.clone_into(selector);
            }
        }
        match self {
            Self::NamespaceCreate(_) => {}
            Self::NamespaceList(_) => {}
            Self::NamespaceGet(input) => resolve(&mut input.namespace, namespace),
            Self::NamespaceUpdate(input) => resolve(&mut input.namespace, namespace),
            Self::NamespaceCopy(input) => resolve(&mut input.namespace, namespace),
            Self::NamespaceDelete(input) => resolve(&mut input.namespace, namespace),
            Self::FsRead(input) => resolve(&mut input.namespace, namespace),
            Self::FsStat(input) => resolve(&mut input.namespace, namespace),
            Self::FsList(input) => resolve(&mut input.namespace, namespace),
            Self::FsWrite(input) => match input {
                inputs::FsWriteInput::Text(input) => resolve(&mut input.namespace, namespace),
                inputs::FsWriteInput::Binary(input) => resolve(&mut input.namespace, namespace),
            },
            Self::FsAppend(input) => match &mut input.0 {
                inputs::FsWriteInput::Text(input) => resolve(&mut input.namespace, namespace),
                inputs::FsWriteInput::Binary(input) => resolve(&mut input.namespace, namespace),
            },
            Self::FsMove(input) => resolve(&mut input.namespace, namespace),
            Self::FsCopy(input) => resolve(&mut input.namespace, namespace),
            Self::FsDelete(input) => resolve(&mut input.namespace, namespace),
            Self::DbList(input) => resolve(&mut input.namespace, namespace),
            Self::DbCreate(input) => resolve(&mut input.namespace, namespace),
            Self::DbDrop(input) => resolve(&mut input.namespace, namespace),
            Self::DbQuery(input) => resolve(&mut input.namespace, namespace),
            Self::DbExecute(input) => resolve(&mut input.namespace, namespace),
            Self::DbInspect(input) => resolve(&mut input.namespace, namespace),
            Self::DbMigrations(input) => resolve(&mut input.namespace, namespace),
            Self::DbMigrate(input) => resolve(&mut input.namespace, namespace),
            Self::FunctionList(input) => resolve(&mut input.namespace, namespace),
            Self::FunctionGet(input) => resolve(&mut input.namespace, namespace),
            Self::FunctionDeclare(input) => resolve(&mut input.namespace, namespace),
            Self::FunctionUpdate(input) => resolve(&mut input.namespace, namespace),
            Self::FunctionRemove(input) => resolve(&mut input.namespace, namespace),
            Self::Call(input) => resolve(&mut input.namespace, namespace),
            Self::Execute(input) => {
                if let Some(selector) = &mut input.namespace {
                    resolve(selector, namespace);
                } else {
                    input.namespace = Some(namespace.to_owned());
                }
            }
            Self::Describe(input) => {
                if let inputs::DescribeInput::Namespace(input) = input {
                    resolve(&mut input.namespace, namespace);
                }
            }
        }
    }
    fn idempotency_key(&self) -> Option<&str> {
        match self {
            Self::Call(input) => input.idempotency_key.as_deref(),
            Self::Execute(input) => input.idempotency_key.as_deref(),
            _ => None,
        }
    }
}

/// Tool names are parsed only at external JSON boundaries. Each variant owns its exact input type.
#[derive(Clone, Debug)]
enum HostTool {
    NamespaceCreate(NamespaceCreateInput),
    NamespaceList(NamespaceListInput),
    NamespaceGet(NamespaceGetInput),
    NamespaceUpdate(NamespaceUpdateInput),
    NamespaceCopy(NamespaceCopyInput),
    NamespaceDelete(NamespaceDeleteInput),
    FsRead(FsReadInput),
    FsStat(FsStatInput),
    FsList(FsListInput),
    FsWrite(FsWriteInput),
    FsAppend(FsAppendInput),
    FsMove(FsMoveInput),
    FsCopy(FsCopyInput),
    FsDelete(FsDeleteInput),
    DbList(DbListInput),
    DbCreate(DbCreateInput),
    DbDrop(DbDropInput),
    DbQuery(DbQueryInput),
    DbExecute(DbExecuteInput),
    DbInspect(DbInspectInput),
    DbMigrations(DbMigrationsInput),
    DbMigrate(DbMigrateInput),
    FunctionList(FunctionListInput),
    FunctionGet(FunctionGetInput),
    FunctionDeclare(FunctionDeclareInput),
    FunctionUpdate(FunctionUpdateInput),
    FunctionRemove(FunctionRemoveInput),
    Call(CallInput),
    Execute(ExecuteInput),
    Describe(DescribeInput),
}
impl HostTool {
    fn parse(name: &str, arguments: Value) -> Result<Self> {
        Ok(match state_store::Operation::from_public_name(name)? {
            state_store::Operation::Namespace(state_store::NamespaceAction::Create) => {
                Self::NamespaceCreate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Namespace(state_store::NamespaceAction::List) => {
                Self::NamespaceList(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Namespace(state_store::NamespaceAction::Get) => {
                Self::NamespaceGet(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Namespace(state_store::NamespaceAction::Update) => {
                Self::NamespaceUpdate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Namespace(state_store::NamespaceAction::Copy) => {
                Self::NamespaceCopy(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Namespace(state_store::NamespaceAction::Delete) => {
                Self::NamespaceDelete(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::File(state_store::FileAction::Read) => Self::FsRead(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::File(state_store::FileAction::Stat) => Self::FsStat(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::File(state_store::FileAction::List) => Self::FsList(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::File(state_store::FileAction::Write) => Self::FsWrite(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::File(state_store::FileAction::Append) => Self::FsAppend(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::File(state_store::FileAction::Move) => Self::FsMove(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::File(state_store::FileAction::Copy) => Self::FsCopy(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::File(state_store::FileAction::Delete) => Self::FsDelete(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::Database(state_store::DatabaseAction::List) => Self::DbList(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::Database(state_store::DatabaseAction::Create) => {
                Self::DbCreate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Database(state_store::DatabaseAction::Drop) => Self::DbDrop(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::Database(state_store::DatabaseAction::Query) => Self::DbQuery(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::Database(state_store::DatabaseAction::Execute) => {
                Self::DbExecute(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Database(state_store::DatabaseAction::Inspect) => {
                Self::DbInspect(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Database(state_store::DatabaseAction::Migrations) => {
                Self::DbMigrations(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Database(state_store::DatabaseAction::Migrate) => {
                Self::DbMigrate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Function(state_store::FunctionAction::List) => {
                Self::FunctionList(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Function(state_store::FunctionAction::Get) => {
                Self::FunctionGet(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Function(state_store::FunctionAction::Declare) => {
                Self::FunctionDeclare(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Function(state_store::FunctionAction::Update) => {
                Self::FunctionUpdate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Function(state_store::FunctionAction::Remove) => {
                Self::FunctionRemove(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                )
            }
            state_store::Operation::Call => Self::Call(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::Execute => Self::Execute(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
            state_store::Operation::Describe => Self::Describe(
                serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
            ),
        })
    }
}

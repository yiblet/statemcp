//! Decode the dynamic Python mcp tool name at its boundary; execute using concrete inputs and results.
use crate::{Error, Result, hosts::Host};
use serde::Serialize;
use serde_json::Value;
use state_contract::{database::*, discovery::*, execution::*, file::*, function::*, namespace::*};

fn encode(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| Error::invalid(e.to_string()))
}
impl Host<'_> {
    pub(crate) fn host_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let mut input = HostTool::parse(name, Value::Object(arguments))?;
        if input.idempotency_key().is_some() {
            return Err(Error::invalid(
                "idempotency keys are only valid on root calls",
            ));
        }
        if let Some(namespace) = self.context.default_namespace() {
            input.resolve_self(namespace);
        }
        let result = match input {
            HostTool::NamespaceCreate(input) => {
                encode(self.namespaces.create_namespace(input, self.context)?)?
            }
            HostTool::NamespaceList(input) => {
                encode(self.namespaces.list_namespaces(input, self.context)?)?
            }
            HostTool::NamespaceGet(input) => {
                encode(self.namespaces.get_namespace(input, self.context)?)?
            }
            HostTool::NamespaceUpdate(input) => {
                encode(self.namespaces.update_namespace(input, self.context)?)?
            }
            HostTool::NamespaceCopy(input) => {
                encode(self.namespaces.copy_namespace(input, self.context)?)?
            }
            HostTool::NamespaceDelete(input) => {
                encode(self.namespaces.delete_namespace(input, self.context)?)?
            }
            HostTool::FsRead(input) => encode(self.files.read_file(input, self.context)?)?,
            HostTool::FsStat(input) => encode(self.files.stat_file(input, self.context)?)?,
            HostTool::FsList(input) => encode(self.files.list_files(input, self.context)?)?,
            HostTool::FsWrite(input) => encode(self.files.write_file(input, self.context)?)?,
            HostTool::FsAppend(input) => encode(self.files.append_file(input, self.context)?)?,
            HostTool::FsMove(input) => encode(self.files.move_file(input, self.context)?)?,
            HostTool::FsCopy(input) => encode(self.files.copy_file(input, self.context)?)?,
            HostTool::FsDelete(input) => encode(self.files.delete_file(input, self.context)?)?,
            HostTool::DbList(input) => encode(self.databases.list_databases(input, self.context)?)?,
            HostTool::DbCreate(input) => {
                encode(self.databases.create_database(input, self.context)?)?
            }
            HostTool::DbDrop(input) => encode(self.databases.drop_database(input, self.context)?)?,
            HostTool::DbQuery(input) => {
                encode(self.databases.query_database(input, self.context)?)?
            }
            HostTool::DbExecute(input) => {
                encode(self.databases.execute_database(input, self.context)?)?
            }
            HostTool::DbInspect(input) => {
                encode(self.databases.inspect_database(input, self.context)?)?
            }
            HostTool::DbMigrations(input) => {
                encode(self.databases.database_migrations(input, self.context)?)?
            }
            HostTool::DbMigrate(input) => {
                encode(self.databases.migrate_database(input, self.context)?)?
            }
            HostTool::FunctionList(input) => {
                encode(self.functions.list_functions(input, self.context)?)?
            }
            HostTool::FunctionGet(input) => {
                encode(self.functions.get_function(input, self.context)?)?
            }
            HostTool::FunctionDeclare(input) => {
                encode(self.functions.declare_function(input, self.context)?)?
            }
            HostTool::FunctionUpdate(input) => {
                encode(self.functions.update_function(input, self.context)?)?
            }
            HostTool::FunctionRemove(input) => {
                encode(self.functions.remove_function(input, self.context)?)?
            }
            HostTool::Call(input) => self.executor.call_function(input, self.context)?,
            HostTool::Execute(input) => encode(self.executor.execute_script(input, self.context)?)?,
            HostTool::Describe(input) => encode(self.discovery.describe(input, self.context)?)?,
        };
        self.context.check()?;
        crate::bounded(&result, self.context.limits().runtime.max_output_bytes)?;
        Ok(result)
    }
}
trait Scope {
    fn resolve_self(&mut self, namespace: &str);
    fn idempotency_key(&self) -> Option<&str>;
}
impl Scope for HostTool {
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
                state_contract::file::FsWriteInput::Text(input) => {
                    resolve(&mut input.namespace, namespace)
                }
                state_contract::file::FsWriteInput::Binary(input) => {
                    resolve(&mut input.namespace, namespace)
                }
            },
            Self::FsAppend(input) => match &mut input.0 {
                state_contract::file::FsWriteInput::Text(input) => {
                    resolve(&mut input.namespace, namespace)
                }
                state_contract::file::FsWriteInput::Binary(input) => {
                    resolve(&mut input.namespace, namespace)
                }
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
                if let state_contract::discovery::DescribeInput::Namespace(input) = input {
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
        Ok(
            match state_contract::operations::Operation::from_public_name(name)? {
                state_contract::operations::Operation::Namespace(
                    state_contract::operations::NamespaceAction::Create,
                ) => Self::NamespaceCreate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Namespace(
                    state_contract::operations::NamespaceAction::List,
                ) => Self::NamespaceList(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Namespace(
                    state_contract::operations::NamespaceAction::Get,
                ) => Self::NamespaceGet(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Namespace(
                    state_contract::operations::NamespaceAction::Update,
                ) => Self::NamespaceUpdate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Namespace(
                    state_contract::operations::NamespaceAction::Copy,
                ) => Self::NamespaceCopy(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Namespace(
                    state_contract::operations::NamespaceAction::Delete,
                ) => Self::NamespaceDelete(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::Read,
                ) => Self::FsRead(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::Stat,
                ) => Self::FsStat(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::List,
                ) => Self::FsList(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::Write,
                ) => Self::FsWrite(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::Append,
                ) => Self::FsAppend(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::Move,
                ) => Self::FsMove(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::Copy,
                ) => Self::FsCopy(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::File(
                    state_contract::operations::FileAction::Delete,
                ) => Self::FsDelete(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::List,
                ) => Self::DbList(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::Create,
                ) => Self::DbCreate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::Drop,
                ) => Self::DbDrop(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::Query,
                ) => Self::DbQuery(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::Execute,
                ) => Self::DbExecute(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::Inspect,
                ) => Self::DbInspect(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::Migrations,
                ) => Self::DbMigrations(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Database(
                    state_contract::operations::DatabaseAction::Migrate,
                ) => Self::DbMigrate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Function(
                    state_contract::operations::FunctionAction::List,
                ) => Self::FunctionList(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Function(
                    state_contract::operations::FunctionAction::Get,
                ) => Self::FunctionGet(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Function(
                    state_contract::operations::FunctionAction::Declare,
                ) => Self::FunctionDeclare(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Function(
                    state_contract::operations::FunctionAction::Update,
                ) => Self::FunctionUpdate(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Function(
                    state_contract::operations::FunctionAction::Remove,
                ) => Self::FunctionRemove(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Call => Self::Call(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Execute => Self::Execute(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
                state_contract::operations::Operation::Describe => Self::Describe(
                    serde_json::from_value(arguments).map_err(|e| Error::invalid(e.to_string()))?,
                ),
            },
        )
    }
}

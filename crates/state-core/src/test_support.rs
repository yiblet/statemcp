#![allow(dead_code)]
use serde_json::Value;
fn encode(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}
pub trait ToolCalls {
    fn tool(&self, name: &str, arguments: Value) -> crate::Result<Value>;
    fn tool_as(&self, principal: &str, name: &str, arguments: Value) -> crate::Result<Value>;
}
impl ToolCalls for crate::State {
    fn tool_as(&self, principal: &str, name: &str, arguments: Value) -> crate::Result<Value> {
        self.clone()
            .with_principal(principal)?
            .tool(name, arguments)
    }
    fn tool(&self, name: &str, arguments: Value) -> crate::Result<Value> {
        fn parse<T: serde::de::DeserializeOwned>(arguments: Value) -> crate::Result<T> {
            serde_json::from_value(arguments)
                .map_err(|e| crate::Error::new(crate::ErrorCode::InvalidArgument, e.to_string()))
        }
        fn identity(
            arguments: &Value,
            name: &str,
        ) -> crate::Result<Option<crate::ReceiptIdentity>> {
            arguments
                .get("idempotency_key")
                .map(|key| {
                    crate::ReceiptIdentity::for_arguments(
                        key.as_str().ok_or_else(|| {
                            crate::Error::new(
                                crate::ErrorCode::InvalidArgument,
                                "idempotency_key must be a string",
                            )
                        })?,
                        name,
                        arguments,
                    )
                })
                .transpose()
        }
        Ok(match name {
            "namespace.create" => encode(self.create_namespace(parse::<
                state_contract::namespace::NamespaceCreateInput,
            >(arguments)?)?),
            "namespace.list" => {
                let _: state_contract::namespace::NamespaceListInput = parse(arguments)?;
                encode(self.list_namespaces()?)
            }
            "namespace.get" => encode(self.get_namespace(parse::<
                state_contract::namespace::NamespaceGetInput,
            >(arguments)?)?),
            "namespace.update" => encode(self.update_namespace(parse::<
                state_contract::namespace::NamespaceUpdateInput,
            >(arguments)?)?),
            "namespace.copy" => encode(self.copy_namespace(parse::<
                state_contract::namespace::NamespaceCopyInput,
            >(arguments)?)?),
            "namespace.delete" => encode(self.delete_namespace(parse::<
                state_contract::namespace::NamespaceDeleteInput,
            >(arguments)?)?),
            "fs.read" => {
                encode(self.read_file(parse::<state_contract::file::FsReadInput>(arguments)?)?)
            }
            "fs.stat" => {
                encode(self.stat_file(parse::<state_contract::file::FsStatInput>(arguments)?)?)
            }
            "fs.list" => {
                encode(self.list_files(parse::<state_contract::file::FsListInput>(arguments)?)?)
            }
            "fs.write" => {
                encode(self.write_file(parse::<state_contract::file::FsWriteInput>(arguments)?)?)
            }
            "fs.append" => {
                encode(self.append_file(parse::<state_contract::file::FsAppendInput>(arguments)?)?)
            }
            "fs.move" => {
                encode(self.move_file(parse::<state_contract::file::FsMoveInput>(arguments)?)?)
            }
            "fs.copy" => {
                encode(self.copy_file(parse::<state_contract::file::FsCopyInput>(arguments)?)?)
            }
            "fs.delete" => {
                encode(self.delete_file(parse::<state_contract::file::FsDeleteInput>(arguments)?)?)
            }
            "db.list" => encode(
                self.list_databases(parse::<state_contract::database::DbListInput>(arguments)?)?,
            ),
            "db.create" => encode(
                self.create_database(parse::<state_contract::database::DbCreateInput>(arguments)?)?,
            ),
            "db.drop" => encode(
                self.drop_database(parse::<state_contract::database::DbDropInput>(arguments)?)?,
            ),
            "db.query" => encode(
                self.query_database(parse::<state_contract::database::DbQueryInput>(arguments)?)?,
            ),
            "db.execute" => encode(self.execute_database(parse::<
                state_contract::database::DbExecuteInput,
            >(arguments)?)?),
            "db.inspect" => encode(self.inspect_database(parse::<
                state_contract::database::DbInspectInput,
            >(arguments)?)?),
            "db.migrations" => encode(self.database_migrations(parse::<
                state_contract::database::DbMigrationsInput,
            >(arguments)?)?),
            "db.migrate" => encode(self.migrate_database(parse::<
                state_contract::database::DbMigrateInput,
            >(arguments)?)?),
            "function.list" => encode(self.list_functions(parse::<
                state_contract::function::FunctionListInput,
            >(arguments)?)?),
            "function.get" => encode(self.get_function(parse::<
                state_contract::function::FunctionGetInput,
            >(arguments)?)?),
            "function.declare" => encode(self.declare_function(parse::<
                state_contract::function::FunctionDeclareInput,
            >(arguments)?)?),
            "function.update" => encode(self.update_function(parse::<
                state_contract::function::FunctionUpdateInput,
            >(arguments)?)?),
            "function.remove" => encode(self.remove_function(parse::<
                state_contract::function::FunctionRemoveInput,
            >(arguments)?)?),
            "call" => {
                let identity = identity(&arguments, name)?;
                encode(self.call_function(
                    parse::<state_contract::execution::CallInput>(arguments)?,
                    identity,
                )?)
            }
            "execute" => {
                let identity = identity(&arguments, name)?;
                encode(self.execute_script(
                    parse::<state_contract::execution::ExecuteInput>(arguments)?,
                    identity,
                )?)
            }
            "describe" => encode(self.describe(parse::<
                state_contract::discovery::DescribeInput,
            >(arguments)?)?),
            _ => {
                return Err(crate::Error::new(
                    crate::ErrorCode::InvalidArgument,
                    "unknown tool",
                ));
            }
        })
    }
}

pub fn memory_store(fs: &state_fs::MemoryFs) -> crate::Result<crate::Store> {
    let backing = state_store::Store::builder()
        .with_fs(fs.clone())
        .open("/state")
        .map_err(state_domain::Error::from)?;
    Ok(crate::Store::from_storage(backing)?)
}
pub fn memory_state(fs: &state_fs::MemoryFs) -> crate::Result<crate::State> {
    Ok(crate::State::with_backend(
        memory_store(fs)?,
        std::sync::Arc::new(crate::EmbeddedBackend),
    ))
}

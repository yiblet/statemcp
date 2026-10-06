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
            "namespace.create" => encode(
                self.create_namespace(parse::<crate::inputs::NamespaceCreateInput>(arguments)?)?,
            ),
            "namespace.list" => {
                let _: crate::inputs::NamespaceListInput = parse(arguments)?;
                encode(self.list_namespaces()?)
            }
            "namespace.get" => {
                encode(self.get_namespace(parse::<crate::inputs::NamespaceGetInput>(arguments)?)?)
            }
            "namespace.update" => encode(
                self.update_namespace(parse::<crate::inputs::NamespaceUpdateInput>(arguments)?)?,
            ),
            "namespace.copy" => {
                encode(self.copy_namespace(parse::<crate::inputs::NamespaceCopyInput>(arguments)?)?)
            }
            "namespace.delete" => encode(
                self.delete_namespace(parse::<crate::inputs::NamespaceDeleteInput>(arguments)?)?,
            ),
            "fs.read" => encode(self.read_file(parse::<crate::inputs::FsReadInput>(arguments)?)?),
            "fs.stat" => encode(self.stat_file(parse::<crate::inputs::FsStatInput>(arguments)?)?),
            "fs.list" => encode(self.list_files(parse::<crate::inputs::FsListInput>(arguments)?)?),
            "fs.write" => {
                encode(self.write_file(parse::<crate::inputs::FsWriteInput>(arguments)?)?)
            }
            "fs.append" => {
                encode(self.append_file(parse::<crate::inputs::FsAppendInput>(arguments)?)?)
            }
            "fs.move" => encode(self.move_file(parse::<crate::inputs::FsMoveInput>(arguments)?)?),
            "fs.copy" => encode(self.copy_file(parse::<crate::inputs::FsCopyInput>(arguments)?)?),
            "fs.delete" => {
                encode(self.delete_file(parse::<crate::inputs::FsDeleteInput>(arguments)?)?)
            }
            "db.list" => {
                encode(self.list_databases(parse::<crate::inputs::DbListInput>(arguments)?)?)
            }
            "db.create" => {
                encode(self.create_database(parse::<crate::inputs::DbCreateInput>(arguments)?)?)
            }
            "db.drop" => {
                encode(self.drop_database(parse::<crate::inputs::DbDropInput>(arguments)?)?)
            }
            "db.query" => {
                encode(self.query_database(parse::<crate::inputs::DbQueryInput>(arguments)?)?)
            }
            "db.execute" => {
                encode(self.execute_database(parse::<crate::inputs::DbExecuteInput>(arguments)?)?)
            }
            "db.inspect" => {
                encode(self.inspect_database(parse::<crate::inputs::DbInspectInput>(arguments)?)?)
            }
            "db.migrations" => encode(
                self.database_migrations(parse::<crate::inputs::DbMigrationsInput>(arguments)?)?,
            ),
            "db.migrate" => {
                encode(self.migrate_database(parse::<crate::inputs::DbMigrateInput>(arguments)?)?)
            }
            "function.list" => {
                encode(self.list_functions(parse::<crate::inputs::FunctionListInput>(arguments)?)?)
            }
            "function.get" => {
                encode(self.get_function(parse::<crate::inputs::FunctionGetInput>(arguments)?)?)
            }
            "function.declare" => encode(
                self.declare_function(parse::<crate::inputs::FunctionDeclareInput>(arguments)?)?,
            ),
            "function.update" => encode(
                self.update_function(parse::<crate::inputs::FunctionUpdateInput>(arguments)?)?,
            ),
            "function.remove" => encode(
                self.remove_function(parse::<crate::inputs::FunctionRemoveInput>(arguments)?)?,
            ),
            "call" => {
                let identity = identity(&arguments, name)?;
                encode(self.call_function(parse::<crate::inputs::CallInput>(arguments)?, identity)?)
            }
            "execute" => {
                let identity = identity(&arguments, name)?;
                encode(
                    self.execute_script(
                        parse::<crate::inputs::ExecuteInput>(arguments)?,
                        identity,
                    )?,
                )
            }
            "describe" => encode(self.describe(parse::<crate::inputs::DescribeInput>(arguments)?)?),
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

#![allow(dead_code)]
use serde_json::Value;
fn encode(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}
pub trait ToolCalls {
    fn tool(&self, name: &str, arguments: Value) -> state_core::Result<Value>;
    fn tool_as(&self, principal: &str, name: &str, arguments: Value) -> state_core::Result<Value>;
}
impl ToolCalls for state_core::State {
    fn tool_as(&self, principal: &str, name: &str, arguments: Value) -> state_core::Result<Value> {
        self.clone()
            .with_principal(principal)?
            .tool(name, arguments)
    }
    fn tool(&self, name: &str, arguments: Value) -> state_core::Result<Value> {
        fn parse<T: serde::de::DeserializeOwned>(arguments: Value) -> state_core::Result<T> {
            serde_json::from_value(arguments).map_err(|e| {
                state_core::Error::new(state_core::ErrorCode::InvalidArgument, e.to_string())
            })
        }
        fn identity(
            arguments: &Value,
            name: &str,
        ) -> state_core::Result<Option<state_core::ReceiptIdentity>> {
            arguments
                .get("idempotency_key")
                .map(|key| {
                    state_core::ReceiptIdentity::for_arguments(
                        key.as_str().ok_or_else(|| {
                            state_core::Error::new(
                                state_core::ErrorCode::InvalidArgument,
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
                state_core::inputs::NamespaceCreateInput,
            >(arguments)?)?),
            "namespace.list" => {
                let _: state_core::inputs::NamespaceListInput = parse(arguments)?;
                encode(self.list_namespaces()?)
            }
            "namespace.get" => encode(
                self.get_namespace(parse::<state_core::inputs::NamespaceGetInput>(arguments)?)?,
            ),
            "namespace.update" => encode(self.update_namespace(parse::<
                state_core::inputs::NamespaceUpdateInput,
            >(arguments)?)?),
            "namespace.copy" => encode(
                self.copy_namespace(parse::<state_core::inputs::NamespaceCopyInput>(arguments)?)?,
            ),
            "namespace.delete" => encode(self.delete_namespace(parse::<
                state_core::inputs::NamespaceDeleteInput,
            >(arguments)?)?),
            "fs.read" => {
                encode(self.read_file(parse::<state_core::inputs::FsReadInput>(arguments)?)?)
            }
            "fs.stat" => {
                encode(self.stat_file(parse::<state_core::inputs::FsStatInput>(arguments)?)?)
            }
            "fs.list" => {
                encode(self.list_files(parse::<state_core::inputs::FsListInput>(arguments)?)?)
            }
            "fs.write" => {
                encode(self.write_file(parse::<state_core::inputs::FsWriteInput>(arguments)?)?)
            }
            "fs.append" => {
                encode(self.append_file(parse::<state_core::inputs::FsAppendInput>(arguments)?)?)
            }
            "fs.move" => {
                encode(self.move_file(parse::<state_core::inputs::FsMoveInput>(arguments)?)?)
            }
            "fs.copy" => {
                encode(self.copy_file(parse::<state_core::inputs::FsCopyInput>(arguments)?)?)
            }
            "fs.delete" => {
                encode(self.delete_file(parse::<state_core::inputs::FsDeleteInput>(arguments)?)?)
            }
            "db.list" => {
                encode(self.list_databases(parse::<state_core::inputs::DbListInput>(arguments)?)?)
            }
            "db.create" => encode(
                self.create_database(parse::<state_core::inputs::DbCreateInput>(arguments)?)?,
            ),
            "db.drop" => {
                encode(self.drop_database(parse::<state_core::inputs::DbDropInput>(arguments)?)?)
            }
            "db.query" => {
                encode(self.query_database(parse::<state_core::inputs::DbQueryInput>(arguments)?)?)
            }
            "db.execute" => encode(
                self.execute_database(parse::<state_core::inputs::DbExecuteInput>(arguments)?)?,
            ),
            "db.inspect" => encode(
                self.inspect_database(parse::<state_core::inputs::DbInspectInput>(arguments)?)?,
            ),
            "db.migrations" => encode(
                self.database_migrations(parse::<state_core::inputs::DbMigrationsInput>(
                    arguments,
                )?)?,
            ),
            "db.migrate" => encode(
                self.migrate_database(parse::<state_core::inputs::DbMigrateInput>(arguments)?)?,
            ),
            "function.list" => encode(
                self.list_functions(parse::<state_core::inputs::FunctionListInput>(arguments)?)?,
            ),
            "function.get" => encode(
                self.get_function(parse::<state_core::inputs::FunctionGetInput>(arguments)?)?,
            ),
            "function.declare" => encode(self.declare_function(parse::<
                state_core::inputs::FunctionDeclareInput,
            >(arguments)?)?),
            "function.update" => encode(
                self.update_function(parse::<state_core::inputs::FunctionUpdateInput>(arguments)?)?,
            ),
            "function.remove" => encode(
                self.remove_function(parse::<state_core::inputs::FunctionRemoveInput>(arguments)?)?,
            ),
            "call" => {
                let identity = identity(&arguments, name)?;
                encode(
                    self.call_function(
                        parse::<state_core::inputs::CallInput>(arguments)?,
                        identity,
                    )?,
                )
            }
            "execute" => {
                let identity = identity(&arguments, name)?;
                encode(self.execute_script(
                    parse::<state_core::inputs::ExecuteInput>(arguments)?,
                    identity,
                )?)
            }
            "describe" => {
                encode(self.describe(parse::<state_core::inputs::DescribeInput>(arguments)?)?)
            }
            _ => {
                return Err(state_core::Error::new(
                    state_core::ErrorCode::InvalidArgument,
                    "unknown tool",
                ));
            }
        })
    }
}

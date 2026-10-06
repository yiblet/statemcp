#![allow(dead_code)]
use serde_json::Value;
fn encode(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}
pub trait ToolCalls {
    fn tool(&mut self, name: &str, arguments: Value) -> state_store::Result<Value>;
    fn commit_result(self) -> state_store::Result<Value>;
}
impl ToolCalls for state_store::Transaction {
    fn tool(&mut self, name: &str, arguments: Value) -> state_store::Result<Value> {
        fn parse<T: serde::de::DeserializeOwned>(arguments: Value) -> state_store::Result<T> {
            serde_json::from_value(arguments).map_err(Into::into)
        }
        Ok(match name {
            "namespace.create" => encode(self.create_namespace(&parse::<
                state_store::inputs::NamespaceCreateInput,
            >(arguments)?)?),
            "namespace.list" => {
                let _: state_store::inputs::NamespaceListInput = parse(arguments)?;
                encode(self.list_namespaces()?)
            }
            "namespace.get" => encode(
                self.get_namespace(&parse::<state_store::inputs::NamespaceGetInput>(arguments)?)?,
            ),
            "namespace.update" => encode(self.update_namespace(&parse::<
                state_store::inputs::NamespaceUpdateInput,
            >(arguments)?)?),
            "namespace.copy" => encode(self.copy_namespace(&parse::<
                state_store::inputs::NamespaceCopyInput,
            >(arguments)?)?),
            "namespace.delete" => encode(self.delete_namespace(&parse::<
                state_store::inputs::NamespaceDeleteInput,
            >(arguments)?)?),
            "fs.read" => {
                encode(self.read_file(&parse::<state_store::inputs::FsReadInput>(arguments)?)?)
            }
            "fs.stat" => {
                encode(self.stat_file(&parse::<state_store::inputs::FsStatInput>(arguments)?)?)
            }
            "fs.list" => {
                encode(self.list_files(&parse::<state_store::inputs::FsListInput>(arguments)?)?)
            }
            "fs.write" => {
                encode(self.write_file(&parse::<state_store::inputs::FsWriteInput>(arguments)?)?)
            }
            "fs.append" => {
                encode(self.append_file(&parse::<state_store::inputs::FsAppendInput>(arguments)?)?)
            }
            "fs.move" => {
                encode(self.move_file(&parse::<state_store::inputs::FsMoveInput>(arguments)?)?)
            }
            "fs.copy" => {
                encode(self.copy_file(&parse::<state_store::inputs::FsCopyInput>(arguments)?)?)
            }
            "fs.delete" => {
                encode(self.delete_file(&parse::<state_store::inputs::FsDeleteInput>(arguments)?)?)
            }
            "db.list" => {
                encode(self.list_databases(&parse::<state_store::inputs::DbListInput>(arguments)?)?)
            }
            "db.create" => encode(
                self.create_database(&parse::<state_store::inputs::DbCreateInput>(arguments)?)?,
            ),
            "db.drop" => {
                encode(self.drop_database(&parse::<state_store::inputs::DbDropInput>(arguments)?)?)
            }
            "db.query" => encode(
                self.query_database(&parse::<state_store::inputs::DbQueryInput>(arguments)?)?,
            ),
            "db.execute" => encode(
                self.execute_database(&parse::<state_store::inputs::DbExecuteInput>(arguments)?)?,
            ),
            "db.inspect" => encode(
                self.inspect_database(&parse::<state_store::inputs::DbInspectInput>(arguments)?)?,
            ),
            "db.migrations" => encode(self.database_migrations(&parse::<
                state_store::inputs::DbMigrationsInput,
            >(arguments)?)?),
            "db.migrate" => encode(
                self.migrate_database(&parse::<state_store::inputs::DbMigrateInput>(arguments)?)?,
            ),
            "function.list" => encode(
                self.list_functions(&parse::<state_store::inputs::FunctionListInput>(arguments)?)?,
            ),
            "function.get" => encode(
                self.get_function(&parse::<state_store::inputs::FunctionGetInput>(arguments)?)?,
            ),
            "function.declare" => encode(self.declare_function(&parse::<
                state_store::inputs::FunctionDeclareInput,
            >(arguments)?)?),
            "function.update" => encode(self.update_function(&parse::<
                state_store::inputs::FunctionUpdateInput,
            >(arguments)?)?),
            "function.remove" => encode(self.remove_function(&parse::<
                state_store::inputs::FunctionRemoveInput,
            >(arguments)?)?),
            _ => {
                return Err(state_store::Error::new(
                    state_store::ErrorCode::InvalidArgument,
                    "unknown tool",
                ));
            }
        })
    }
    fn commit_result(self) -> state_store::Result<Value> {
        Ok(match self.commit()? {
            state_store::records::CommitOutcome::Published(metadata) => encode(metadata),
            state_store::records::CommitOutcome::Replayed(result) => {
                #[derive(serde::Serialize)]
                struct Replay {
                    replayed: bool,
                    result: Value,
                }
                let result = match result {
                    state_store::records::ReceiptResult::Function(value) => value,
                    state_store::records::ReceiptResult::Script(result) => encode(result),
                };
                encode(Replay {
                    replayed: true,
                    result,
                })
            }
        })
    }
}

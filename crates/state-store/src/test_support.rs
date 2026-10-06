#![allow(dead_code)]
use serde_json::Value;
fn encode(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}
pub trait ToolCalls {
    fn tool(&mut self, name: &str, arguments: Value) -> crate::Result<Value>;
    fn commit_result(self) -> crate::Result<Value>;
}
impl ToolCalls for crate::Transaction {
    fn tool(&mut self, name: &str, arguments: Value) -> crate::Result<Value> {
        fn parse<T: serde::de::DeserializeOwned>(arguments: Value) -> crate::Result<T> {
            serde_json::from_value(arguments).map_err(Into::into)
        }
        Ok(match name {
            "namespace.create" => encode(
                self.create_namespace(&parse::<crate::inputs::NamespaceCreateInput>(arguments)?)?,
            ),
            "namespace.list" => {
                let _: crate::inputs::NamespaceListInput = parse(arguments)?;
                encode(self.list_namespaces()?)
            }
            "namespace.get" => {
                encode(self.get_namespace(&parse::<crate::inputs::NamespaceGetInput>(arguments)?)?)
            }
            "namespace.update" => encode(
                self.update_namespace(&parse::<crate::inputs::NamespaceUpdateInput>(arguments)?)?,
            ),
            "namespace.copy" => encode(
                self.copy_namespace(&parse::<crate::inputs::NamespaceCopyInput>(arguments)?)?,
            ),
            "namespace.delete" => encode(
                self.delete_namespace(&parse::<crate::inputs::NamespaceDeleteInput>(arguments)?)?,
            ),
            "fs.read" => encode(self.read_file(&parse::<crate::inputs::FsReadInput>(arguments)?)?),
            "fs.stat" => encode(self.stat_file(&parse::<crate::inputs::FsStatInput>(arguments)?)?),
            "fs.list" => encode(self.list_files(&parse::<crate::inputs::FsListInput>(arguments)?)?),
            "fs.write" => {
                encode(self.write_file(&parse::<crate::inputs::FsWriteInput>(arguments)?)?)
            }
            "fs.append" => {
                encode(self.append_file(&parse::<crate::inputs::FsAppendInput>(arguments)?)?)
            }
            "fs.move" => encode(self.move_file(&parse::<crate::inputs::FsMoveInput>(arguments)?)?),
            "fs.copy" => encode(self.copy_file(&parse::<crate::inputs::FsCopyInput>(arguments)?)?),
            "fs.delete" => {
                encode(self.delete_file(&parse::<crate::inputs::FsDeleteInput>(arguments)?)?)
            }
            "db.list" => {
                encode(self.list_databases(&parse::<crate::inputs::DbListInput>(arguments)?)?)
            }
            "db.create" => {
                encode(self.create_database(&parse::<crate::inputs::DbCreateInput>(arguments)?)?)
            }
            "db.drop" => {
                encode(self.drop_database(&parse::<crate::inputs::DbDropInput>(arguments)?)?)
            }
            "db.query" => {
                encode(self.query_database(&parse::<crate::inputs::DbQueryInput>(arguments)?)?)
            }
            "db.execute" => {
                encode(self.execute_database(&parse::<crate::inputs::DbExecuteInput>(arguments)?)?)
            }
            "db.inspect" => {
                encode(self.inspect_database(&parse::<crate::inputs::DbInspectInput>(arguments)?)?)
            }
            "db.migrations" => encode(
                self.database_migrations(&parse::<crate::inputs::DbMigrationsInput>(arguments)?)?,
            ),
            "db.migrate" => {
                encode(self.migrate_database(&parse::<crate::inputs::DbMigrateInput>(arguments)?)?)
            }
            "function.list" => {
                encode(self.list_functions(&parse::<crate::inputs::FunctionListInput>(arguments)?)?)
            }
            "function.get" => {
                encode(self.get_function(&parse::<crate::inputs::FunctionGetInput>(arguments)?)?)
            }
            "function.declare" => encode(
                self.declare_function(&parse::<crate::inputs::FunctionDeclareInput>(arguments)?)?,
            ),
            "function.update" => encode(
                self.update_function(&parse::<crate::inputs::FunctionUpdateInput>(arguments)?)?,
            ),
            "function.remove" => encode(
                self.remove_function(&parse::<crate::inputs::FunctionRemoveInput>(arguments)?)?,
            ),
            _ => {
                return Err(crate::Error::new(
                    crate::ErrorCode::InvalidArgument,
                    "unknown tool",
                ));
            }
        })
    }
    fn commit_result(self) -> crate::Result<Value> {
        Ok(match self.commit()? {
            crate::records::CommitOutcome::Published(metadata) => encode(metadata),
            crate::records::CommitOutcome::Replayed(result) => {
                #[derive(serde::Serialize)]
                struct Replay {
                    replayed: bool,
                    result: Value,
                }
                let result = match result {
                    crate::records::ReceiptResult::Function(value) => value,
                    crate::records::ReceiptResult::Script(result) => encode(result),
                };
                encode(Replay {
                    replayed: true,
                    result,
                })
            }
        })
    }
}

//! Pure translation from storage results to the public tool surface.
use crate::{DatabaseAction, FileAction, FunctionAction, Operation};
use serde_json::{Value, json};

pub(crate) fn tool_result(operation: Operation, mut result: Value) -> Value {
    // Keep storage metadata available internally and through explicit inspection.
    if let Operation::Function(action) = operation {
        match action {
            FunctionAction::Declare | FunctionAction::Update => {
                result =
                    json!({"name":result["name"],"version":result["version"],"published":true});
            }
            FunctionAction::List => {
                for function in result["functions"].as_array_mut().expect("function list") {
                    for field in ["source", "source_hash", "database_ids", "abi_version"] {
                        function
                            .as_object_mut()
                            .expect("function metadata")
                            .remove(field);
                    }
                }
            }
            FunctionAction::Get | FunctionAction::Remove => {}
        }
    }
    if operation == Operation::Database(DatabaseAction::List) {
        for database in result["databases"].as_array_mut().expect("database list") {
            database
                .as_object_mut()
                .expect("database metadata")
                .remove("snapshot");
        }
    }
    if operation == Operation::Database(DatabaseAction::Create) {
        result = json!({"name":result["name"],"created":true});
    }
    if matches!(operation, Operation::File(action) if action != FileAction::Stat) {
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
    if operation.publishes_namespace() {
        result
            .as_object_mut()
            .expect("namespace metadata")
            .remove("revision");
    }
    result
}

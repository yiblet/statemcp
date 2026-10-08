use crate::{database::*, discovery::*, execution::*, file::*, function::*, namespace::*};
use schemars::JsonSchema;
/// Generate the public argument schema for an operation without implementation dependencies.
pub fn schema(operation: crate::operations::Operation) -> schemars::Schema {
    use crate::operations::{
        DatabaseAction, FileAction, FunctionAction, NamespaceAction, Operation,
    };
    fn generate<T: JsonSchema>() -> schemars::Schema {
        let mut schema = schemars::generate::SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<T>();
        if let Some(object) = schema.as_object_mut() {
            object.remove("title");
            object.remove("description");
        }
        schema
    }
    match operation {
        Operation::Api(crate::operations::ApiAction::Publish) => {
            generate::<crate::http::ApiPublishInput>()
        }
        Operation::Api(crate::operations::ApiAction::Get) => generate::<crate::http::ApiGetInput>(),
        Operation::Api(crate::operations::ApiAction::List) => {
            generate::<crate::http::ApiListInput>()
        }
        Operation::Api(crate::operations::ApiAction::Remove) => {
            generate::<crate::http::ApiRemoveInput>()
        }
        Operation::Namespace(NamespaceAction::Create) => generate::<NamespaceCreateInput>(),

        Operation::Namespace(NamespaceAction::List) => generate::<NamespaceListInput>(),

        Operation::Namespace(NamespaceAction::Get) => generate::<NamespaceGetInput>(),

        Operation::Namespace(NamespaceAction::Update) => generate::<NamespaceUpdateInput>(),

        Operation::Namespace(NamespaceAction::Copy) => generate::<NamespaceCopyInput>(),

        Operation::Namespace(NamespaceAction::Delete) => generate::<NamespaceDeleteInput>(),

        Operation::File(FileAction::Read) => generate::<FsReadInput>(),

        Operation::File(FileAction::Stat) => generate::<FsStatInput>(),

        Operation::File(FileAction::List) => generate::<FsListInput>(),

        Operation::File(FileAction::Write) => generate::<FsWriteInput>(),

        Operation::File(FileAction::Append) => generate::<FsAppendInput>(),

        Operation::File(FileAction::Move) => generate::<FsMoveInput>(),

        Operation::File(FileAction::Copy) => generate::<FsCopyInput>(),

        Operation::File(FileAction::Delete) => generate::<FsDeleteInput>(),

        Operation::Database(DatabaseAction::List) => generate::<DbListInput>(),

        Operation::Database(DatabaseAction::Create) => generate::<DbCreateInput>(),

        Operation::Database(DatabaseAction::Drop) => generate::<DbDropInput>(),

        Operation::Database(DatabaseAction::Query) => generate::<DbQueryInput>(),

        Operation::Database(DatabaseAction::Execute) => generate::<DbExecuteInput>(),

        Operation::Database(DatabaseAction::Inspect) => generate::<DbInspectInput>(),

        Operation::Database(DatabaseAction::Migrations) => generate::<DbMigrationsInput>(),

        Operation::Database(DatabaseAction::Migrate) => generate::<DbMigrateInput>(),

        Operation::Function(FunctionAction::List) => generate::<FunctionListInput>(),

        Operation::Function(FunctionAction::Get) => generate::<FunctionGetInput>(),

        Operation::Function(FunctionAction::Remove) => generate::<FunctionRemoveInput>(),

        Operation::Function(FunctionAction::Declare) => generate::<FunctionDeclareInput>(),

        Operation::Function(FunctionAction::Update) => generate::<FunctionUpdateInput>(),

        Operation::Call => generate::<CallInput>(),

        Operation::Execute => generate::<ExecuteInput>(),

        Operation::Describe => generate::<DescribeInput>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{FileAction, Operation};
    use serde_json::json;

    #[test]
    fn contract_wire_and_schema_compatibility() -> Result<(), Box<dyn std::error::Error>> {
        let file: FsWriteInput = serde_json::from_value(json!({
            "namespace": "demo", "path": "/binary", "base64": "AP8="
        }))?;
        assert_eq!(
            serde_json::to_value(file)?,
            json!({
                "namespace": "demo", "path": "/binary", "base64": "AP8="
            })
        );
        assert!(
            serde_json::from_value::<FsWriteInput>(json!({
                "namespace": "demo", "path": "/binary", "base64": "AP8=", "text": "bad"
            }))
            .is_err()
        );
        let receipt = crate::execution::ReceiptResult::Script(crate::execution::ExecutionResult {
            value: json!({"ok": true}),
            stdout: "printed\n".into(),
        });
        assert_eq!(
            serde_json::to_value(receipt)?,
            json!({
                "kind": "script", "result": {"value": {"ok": true}, "stdout": "printed\n"}
            })
        );
        let read_schema = serde_json::to_value(schema(Operation::File(FileAction::Read)))?;
        assert_eq!(
            read_schema,
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {"namespace": {"type": "string"}, "path": {"type": "string"}},
                "required": ["namespace", "path"],
                "additionalProperties": false
            })
        );
        let unchanged: NamespaceUpdateInput = serde_json::from_value(json!({"namespace": "demo"}))?;
        assert_eq!(unchanged.description, DescriptionChange::Unchanged);
        assert_eq!(
            serde_json::to_value(unchanged)?,
            json!({"namespace": "demo"})
        );
        let cleared: NamespaceUpdateInput =
            serde_json::from_value(json!({"namespace": "demo", "description": null}))?;
        assert_eq!(cleared.description, DescriptionChange::Clear);
        assert_eq!(
            serde_json::to_value(cleared)?,
            json!({"namespace": "demo", "description": null})
        );
        Ok(())
    }
}

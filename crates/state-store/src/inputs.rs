//! Typed public MCP inputs shared by SDK schema generation and service validation.
use crate::DescriptionChange;
use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn object_root(schema: &mut Schema) {
    schema.insert("type".into(), Value::String("object".into()));
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EndpointSchema {
    Object(Map<String, Value>),
    Boolean(bool),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMode {
    #[default]
    Overview,
    Full,
    Runtime,
    Readme,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MigrationInput {
    pub id: String,
    pub sql: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceCreateInput {
    #[serde(default, skip_serializing_if = "DescriptionChange::is_unchanged")]
    #[schemars(with = "Option<String>")]
    pub description: DescriptionChange,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceListInput {}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceGetInput {
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceUpdateInput {
    #[serde(default, skip_serializing_if = "DescriptionChange::is_unchanged")]
    #[schemars(with = "Option<String>")]
    pub description: DescriptionChange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub name: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceCopyInput {
    #[serde(default, skip_serializing_if = "DescriptionChange::is_unchanged")]
    #[schemars(with = "Option<String>")]
    pub description: DescriptionChange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub name: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceDeleteInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsReadInput {
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsStatInput {
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsListInput {
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileTextInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileBase64Input {
    pub base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(transform = object_root)]
pub enum FsWriteInput {
    Text(FileTextInput),
    Binary(FileBase64Input),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct FsAppendInput(pub FsWriteInput);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsMoveInput {
    pub destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsCopyInput {
    pub destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsDeleteInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub recursive: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbListInput {
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbCreateInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbDropInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbQueryInput {
    pub database: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<crate::records::SqlCell>")]
    pub params: Option<Vec<crate::records::SqlCell>>,
    pub sql: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbExecuteInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<crate::records::SqlCell>")]
    pub params: Option<Vec<crate::records::SqlCell>>,
    pub sql: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbInspectInput {
    pub database: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbMigrationsInput {
    pub database: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbMigrateInput {
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    pub migrations: Vec<MigrationInput>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionListInput {
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionGetInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_version: Option<String>,
    pub name: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionRemoveInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_version: Option<String>,
    pub name: String,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionDeclareInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_version: Option<String>,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "EndpointSchema")]
    pub input_schema: Option<EndpointSchema>,
    pub name: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "EndpointSchema")]
    pub output_schema: Option<EndpointSchema>,
    pub symbol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionUpdateInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_version: Option<String>,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "EndpointSchema")]
    pub input_schema: Option<EndpointSchema>,
    pub name: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "EndpointSchema")]
    pub output_schema: Option<EndpointSchema>,
    pub symbol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Map<String, Value>")]
    pub arguments: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub expected_version: Option<String>,
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub idempotency_key: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Value")]
    pub inputs: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub namespace: Option<String>,
    pub script: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryOverviewInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DiscoveryMode")]
    pub mode: Option<DiscoveryMode>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryToolInput {
    pub tool: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryNamespaceInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub function: Option<String>,
    pub namespace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(transform = object_root)]
pub enum DescribeInput {
    Overview(DiscoveryOverviewInput),
    Tool(DiscoveryToolInput),
    Namespace(DiscoveryNamespaceInput),
}

pub fn schema(operation: crate::Operation) -> schemars::Schema {
    use crate::{DatabaseAction, FileAction, FunctionAction, NamespaceAction, Operation};
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

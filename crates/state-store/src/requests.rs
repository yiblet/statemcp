//! Fixed request envelopes. Only agent-defined inputs, schemas, and SQL cells are JSON.
use crate::{
    DatabaseAction, Error, FileAction, FunctionAction, NamespaceAction, Operation, Result, Tool,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamespaceRequest {
    pub action: NamespaceAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
}
fn root_path() -> String {
    "/".into()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRequest {
    pub action: FileAction,
    pub namespace: String,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
}
impl FileRequest {
    pub fn read(namespace: String, path: String) -> Self {
        Self {
            action: FileAction::Read,
            namespace,
            path,
            text: None,
            base64: None,
            destination: None,
            recursive: false,
            expected_revision: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MigrationSource {
    pub id: String,
    pub sql: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatabaseRequest {
    pub action: DatabaseAction,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(default)]
    pub params: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrations: Option<Vec<MigrationSource>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRequest {
    pub action: FunctionAction,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
}
impl FunctionRequest {
    pub fn lookup(namespace: String, name: String, expected_version: Option<String>) -> Self {
        Self {
            action: FunctionAction::Get,
            namespace,
            name: Some(name),
            file: None,
            symbol: None,
            input_schema: None,
            output_schema: None,
            description: None,
            expected_version,
            expected_revision: None,
        }
    }
    pub fn list(namespace: String) -> Self {
        Self {
            action: FunctionAction::List,
            name: None,
            ..Self::lookup(namespace, String::new(), None)
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallRequest {
    pub namespace: String,
    pub function: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}
fn empty_object() -> Value {
    Value::Object(Default::default())
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecuteRequest {
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default)]
    pub inputs: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMode {
    #[default]
    Overview,
    Full,
    Runtime,
    Readme,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DescribeRequest {
    #[serde(default)]
    pub mode: DiscoveryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Arguments {
    Namespace(NamespaceRequest),
    File(FileRequest),
    Database(DatabaseRequest),
    Function(FunctionRequest),
    Call(CallRequest),
    Execute(ExecuteRequest),
    Describe(DescribeRequest),
}
impl Arguments {
    pub fn parse(tool: Tool, value: Value) -> Result<Self> {
        macro_rules! decode {
            ($variant:ident) => {
                serde_json::from_value(value).map(Self::$variant)
            };
        }
        match tool {
            Tool::Namespace => decode!(Namespace),
            Tool::File => decode!(File),
            Tool::Database => decode!(Database),
            Tool::Function => decode!(Function),
            Tool::Call => decode!(Call),
            Tool::Execute => decode!(Execute),
            Tool::Describe => decode!(Describe),
        }
        .map_err(|error| {
            Error::invalid(format!("invalid {} arguments: {error}", tool.public_name()))
        })
    }
    pub fn operation(&self) -> Operation {
        match self {
            Self::Namespace(args) => Operation::Namespace(args.action),
            Self::File(args) => Operation::File(args.action),
            Self::Database(args) => Operation::Database(args.action),
            Self::Function(args) => Operation::Function(args.action),
            Self::Call(_) => Operation::Call,
            Self::Execute(_) => Operation::Execute,
            Self::Describe(_) => Operation::Describe,
        }
    }
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Self::Namespace(args) => args.namespace.as_deref(),
            Self::File(args) => Some(&args.namespace),
            Self::Database(args) => Some(&args.namespace),
            Self::Function(args) => Some(&args.namespace),
            Self::Call(args) => Some(&args.namespace),
            Self::Execute(args) => args.namespace.as_deref(),
            Self::Describe(args) => args.namespace.as_deref(),
        }
    }
    pub fn resolve_self(&mut self, namespace: &str) {
        let selector = match self {
            Self::Namespace(args) => args.namespace.as_mut(),
            Self::File(args) => Some(&mut args.namespace),
            Self::Database(args) => Some(&mut args.namespace),
            Self::Function(args) => Some(&mut args.namespace),
            Self::Call(args) => Some(&mut args.namespace),
            Self::Execute(args) => args.namespace.as_mut(),
            Self::Describe(args) => args.namespace.as_mut(),
        };
        if let Some(selector) = selector
            && selector == "self"
        {
            selector.clear();
            selector.push_str(namespace);
        }
    }
    pub fn idempotency_key(&self) -> Option<&str> {
        match self {
            Self::Call(args) => args.idempotency_key.as_deref(),
            Self::Execute(args) => args.idempotency_key.as_deref(),
            _ => None,
        }
    }
}

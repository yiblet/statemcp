//! Shared execution contracts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub value: Value,
    pub stdout: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReceiptResult {
    Function(Value),
    Script(ExecutionResult),
}

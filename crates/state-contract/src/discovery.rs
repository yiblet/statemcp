//! Shared discovery contracts.
use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn object_root(schema: &mut Schema) {
    schema.insert("type".into(), Value::String("object".into()));
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

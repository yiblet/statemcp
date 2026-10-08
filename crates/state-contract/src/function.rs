//! Shared function contracts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EndpointSchema {
    Object(Map<String, Value>),
    Boolean(bool),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub name: String,
    pub file: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<crate::function::EndpointSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<crate::function::EndpointSchema>,
    pub version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Functions {
    pub functions: Vec<FunctionSummary>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionRemoved {
    pub removed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionPublished {
    pub name: String,
    pub version: String,
    pub published: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
// Field order matches the established canonical declaration hash.
#[serde(deny_unknown_fields)]
pub struct FunctionDeclaration {
    pub abi_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<crate::function::EndpointSchema>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<crate::function::EndpointSchema>,
    pub source: String,
    pub source_hash: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn declaration_hash_is_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        // The established declaration format hashes fields in declaration order,
        // excluding an empty version. Schema booleans and pinned modules are retained.
        let declaration = FunctionDeclaration {
            abi_version: 1,
            description: Some("fixture".into()),
            file: "/api.py".into(),
            input_schema: Some(EndpointSchema::Boolean(false)),
            modules: BTreeMap::from([("helper".into(), "def helper(): return 2".into())]),
            name: "answer".into(),
            output_schema: Some(EndpointSchema::Boolean(true)),
            source: "def answer(): return 42".into(),
            source_hash: "pinned-source".into(),
            symbol: "answer".into(),
            version: String::new(),
        };
        let encoded = serde_json::to_vec(&declaration)?;
        assert_eq!(
            format!("{:x}", Sha256::digest(encoded)),
            "bcfcc01947e60c2c64a185f6b0bf3e90b2e380742617a74b359e5e3ff77cfed6"
        );
        // Existing records predate pinned module imports and omit that field.
        let old = r#"{"abi_version":1,"file":"/api.py","name":"answer","source":"def answer(): return 42","source_hash":"pinned-source","symbol":"answer","version":"published"}"#;
        let decoded: FunctionDeclaration = serde_json::from_str(old)?;
        assert!(decoded.modules.is_empty());
        assert_eq!(serde_json::to_string(&decoded)?, old);
        Ok(())
    }
}

/// Owned namespace source snapshot, loaded once for validation and publication.
/// This is an internal service contract and is not a persisted or protocol envelope.
#[derive(Clone, Debug)]
pub struct FunctionSources {
    pub source: String,
    pub modules: BTreeMap<String, String>,
}

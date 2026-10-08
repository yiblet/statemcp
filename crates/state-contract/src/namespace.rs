//! Shared namespace contracts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub files: usize,
    pub databases: usize,
    pub functions: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceInfo {
    #[serde(flatten)]
    pub summary: NamespaceSummary,
    pub revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Namespaces {
    pub namespaces: Vec<NamespaceInfo>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceDeleted {
    pub deleted: bool,
    pub id: String,
}
/// An omitted description preserves existing metadata; null explicitly clears it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DescriptionChange {
    Set(String),
    Clear,
    #[default]
    Unchanged,
}
impl DescriptionChange {
    pub fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }
    pub fn apply(&self, description: &mut Option<String>) {
        match self {
            Self::Unchanged => {}
            Self::Set(value) => *description = Some(value.clone()),
            Self::Clear => *description = None,
        }
    }
}
impl<'de> Deserialize<'de> for DescriptionChange {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)? {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}

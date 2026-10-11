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

/// Select a retained revision belonging to a namespace.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespaceRevisionInput {
    pub namespace: String,
    pub revision: String,
}
/// One retained metadata revision; database files are not historical snapshots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevisionInfo {
    pub revision: String,
    pub current: bool,
    pub retention: RevisionRetention,
}
/// Retained revisions, newest first, and the independent pin limit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceHistory {
    pub revisions: Vec<RevisionInfo>,
    pub pin_limit: u32,
}

/// Request to protect a retained revision, optionally giving its pin a name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamespacePinInput {
    pub namespace: String,
    pub revision: String,
    pub name: Option<PinName>,
}
/// A nonempty, bounded name for a revision pin.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct PinName(String);
impl PinName {
    /// Parse a nonblank pin name of at most 256 UTF-8 bytes.
    pub fn parse(value: String) -> Result<Self, crate::ParseError> {
        if value.trim().is_empty() || value.len() > 256 {
            return Err(crate::ParseError::invalid(
                "pin name must be nonblank and at most 256 bytes",
            ));
        }
        Ok(Self(value))
    }
    /// Name as supplied at the boundary.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for PinName {
    type Error = crate::ParseError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}
impl From<PinName> for String {
    fn from(value: PinName) -> Self {
        value.0
    }
}
/// Recent revisions are eligible for rolling pruning; pins retain their content.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevisionRetention {
    Recent,
    Pinned { name: Option<PinName> },
}

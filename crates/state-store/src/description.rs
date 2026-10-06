use serde::{Deserialize, Serialize};

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
    pub(crate) fn apply(&self, description: &mut Option<String>) {
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

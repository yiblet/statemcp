//! Shared JSON argument and virtual path validation.
use crate::{Error, Result};

/// A normalized absolute virtual path with no parent traversal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtualPath(String);
impl VirtualPath {
    pub fn parse(value: &str) -> Result<Self> {
        virtual_path(value).map(Self)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// True for this path and its descendants, respecting component boundaries.
    pub fn contains(&self, path: &Self) -> bool {
        self.0 == "/"
            || self == path
            || path
                .0
                .strip_prefix(&self.0)
                .is_some_and(|tail| tail.starts_with('/'))
    }
}

pub(crate) fn required<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str> {
    value.ok_or_else(|| Error::invalid(format!("{field} must be a string")))
}
pub(crate) fn name(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(Error::invalid(
            "name must contain 1–128 bytes without slash or control characters",
        ));
    }
    Ok(value.to_string())
}
pub(crate) fn virtual_path(value: &str) -> Result<String> {
    if !value.starts_with('/')
        || value.len() > 4096
        || value.chars().any(|c| c == '\0' || c == '\\')
    {
        return Err(Error::invalid(
            "path must be an absolute virtual POSIX path",
        ));
    }
    let mut parts = Vec::new();
    for part in value.split('/') {
        match PathComponent::from(part) {
            PathComponent::Current => {}
            PathComponent::Parent => return Err(Error::invalid("parent traversal is forbidden")),
            PathComponent::Name(name) => parts.push(name),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

/// A borrowed component of a virtual POSIX path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathComponent<'a> {
    Current,
    Parent,
    Name(&'a str),
}
impl<'a> From<&'a str> for PathComponent<'a> {
    fn from(value: &'a str) -> Self {
        if value.is_empty() || value == "." {
            Self::Current
        } else if value == ".." {
            Self::Parent
        } else {
            Self::Name(value)
        }
    }
}

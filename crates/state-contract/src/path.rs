//! Shared JSON argument and virtual path validation.
use crate::ParseError as Error;
type Result<T> = std::result::Result<T, Error>;

/// A normalized absolute virtual path with no parent traversal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtualPath(String);
impl VirtualPath {
    /// Parse an absolute POSIX path, normalizing repeated separators and dots.
    /// Rejects parent traversal, backslashes, NUL bytes, and paths over 4096 bytes.
    pub fn parse(value: &str) -> Result<Self> {
        virtual_path(value).map(Self)
    }
    /// Borrow the normalized absolute path.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{FileAction, Operation};

    #[test]
    fn parsed_contracts_retain_path_and_operation_invariants() -> Result<()> {
        let parent = VirtualPath::parse("//api/./")?;
        assert_eq!(parent.as_str(), "/api");
        assert!(parent.contains(&VirtualPath::parse("/api/handler.py")?));
        assert!(!parent.contains(&VirtualPath::parse("/apis/handler.py")?));
        for invalid in ["relative", "/api/../secret", "/api\\secret", "/api\0secret"] {
            assert_eq!(
                VirtualPath::parse(invalid).err().map(|e| e.code),
                Some(crate::ErrorCode::InvalidArgument)
            );
        }
        assert_eq!(
            Operation::from_public_name("fs.write")?,
            Operation::File(FileAction::Write)
        );
        assert!(Operation::from_public_name("fs.unknown").is_err());
        assert!(Operation::from_public_name("fs.write.extra").is_err());
        Ok(())
    }
}

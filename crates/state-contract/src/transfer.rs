//! HTTP transfer contracts and normalized destination paths.
use crate::{ParseError, VirtualPath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Maximum file data in one multipart upload (10 MiB).
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
/// Maximum paths authorized by one transfer token.
pub const MAX_TRANSFER_FILES: usize = 256;

/// Non-root, normalized POSIX file path; relative names resolve from namespace root.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TransferPath(String);
impl TransferPath {
    /// Reject traversal, control characters and directory-only names.
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        if value.is_empty() || value.ends_with('/') || value.chars().any(|c| c.is_control()) {
            return Err(ParseError::invalid(
                "transfer destination must be a literal file path",
            ));
        }
        let absolute = if value.starts_with('/') {
            value.to_owned()
        } else {
            format!("/{value}")
        };
        let path = VirtualPath::parse(&absolute)?;
        if path.as_str() == "/" {
            return Err(ParseError::invalid("root is not a file"));
        }
        Ok(Self(path.as_str().to_owned()))
    }
    /// Absolute namespace path, also used as the multipart field name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Relative archive entry path with directories preserved.
    pub fn archive_name(&self) -> &str {
        self.0.trim_start_matches('/')
    }
}
/// Validated, nonempty, nonoverlapping literal upload destinations.
#[derive(Clone, Debug, Serialize)]
pub struct UploadPaths(Vec<TransferPath>);
impl UploadPaths {
    /// Normalize paths; reject duplicates and file/directory collisions.
    pub fn parse(values: &[String]) -> Result<Self, ParseError> {
        if values.is_empty() || values.len() > MAX_TRANSFER_FILES {
            return Err(ParseError::invalid("files must contain 1..256 paths"));
        }
        let mut paths = BTreeSet::new();
        for value in values {
            let path = TransferPath::parse(value)?;
            if !paths.insert(path) {
                return Err(ParseError::invalid("duplicate transfer path"));
            }
        }
        for path in &paths {
            if paths.iter().any(|other| {
                other
                    .as_str()
                    .strip_prefix(path.as_str())
                    .is_some_and(|tail| tail.starts_with('/'))
            }) {
                return Err(ParseError::invalid("file collides with a directory"));
            }
        }
        Ok(Self(paths.into_iter().collect()))
    }
    /// Authorized paths in deterministic order.
    pub fn paths(&self) -> &[TransferPath] {
        &self.0
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsUploadInput {
    pub namespace: String,
    pub files: Vec<String>,
}
/// Requested archive encoding. Only tar is currently implemented.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    #[default]
    Tar,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsDownloadInput {
    pub namespace: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub format: ArchiveFormat,
}
/// A single-use transfer capability; never log the URL or token.
#[derive(Serialize)]
pub struct TransferTicket {
    pub token: String,
    pub url: String,
    pub expires_at: u64,
    pub files: Vec<TransferPath>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_paths_keep_normalization_and_reject_ambiguous_authority() -> Result<(), ParseError> {
        let paths = UploadPaths::parse(&["folder/file".into(), "/a".into()])?;
        assert_eq!(
            paths
                .paths()
                .iter()
                .map(TransferPath::as_str)
                .collect::<Vec<_>>(),
            ["/a", "/folder/file"]
        );
        for values in [
            vec!["../x"],
            vec!["/a", "a"],
            vec!["/a", "/a/b"],
            vec!["/"],
            vec!["a\\b"],
            vec!["a/"],
            vec!["a\0b"],
        ] {
            assert!(
                UploadPaths::parse(&values.into_iter().map(str::to_owned).collect::<Vec<_>>())
                    .is_err()
            );
        }
        assert!(serde_json::from_str::<ArchiveFormat>("\"zip\"").is_err());
        Ok(())
    }
}

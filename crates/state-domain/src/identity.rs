//! UUID identities and content digests used across manifests, files, and SQL schemas.
use sha2::{Digest, Sha256};

pub(crate) fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

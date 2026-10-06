//! Namespace isolation is the only boundary on published function state access.
use crate::{Error, Result};

#[derive(Clone)]
pub(crate) enum Access {
    Owner,
    Namespace(String),
}

pub(crate) fn check_namespace(scope: &str, target: &str) -> Result<()> {
    if scope == target {
        Ok(())
    } else {
        Err(Error::denied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolation_compares_stable_namespace_identities() {
        assert!(check_namespace("id-a", "id-a").is_ok());
        assert_eq!(
            check_namespace("id-a", "id-b").unwrap_err().code,
            crate::ErrorCode::PermissionDenied
        );
    }
}

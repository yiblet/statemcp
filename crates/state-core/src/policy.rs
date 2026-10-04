use crate::{DatabaseAction, Error, Result};
use state_store::{DatabaseAccess, FileAccess, VirtualPath};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub(crate) enum Access {
    Owner,
    Endpoint(EndpointAccess),
}
#[derive(Clone)]
pub(crate) struct EndpointAccess {
    pub namespace: String,
    pub databases: BTreeMap<String, (DatabaseAccess, String)>,
    pub files: BTreeMap<VirtualPath, FileAccess>,
    pub calls: BTreeSet<(String, String)>,
}
impl EndpointAccess {
    fn file(&self, path: &VirtualPath, access: FileAccess) -> Result<()> {
        if self.files.iter().any(|(prefix, mode)| {
            (access == FileAccess::Read || *mode == FileAccess::Write) && prefix.contains(path)
        }) {
            Ok(())
        } else {
            Err(Error::denied())
        }
    }
}

/// A structurally parsed target. Names still need resolution against current facts.
#[derive(Debug)]
pub(crate) enum AuthorizationRequest<'a> {
    Describe,
    Forbidden,
    Call {
        namespace: &'a str,
        function: &'a str,
    },
    File {
        namespace: &'a str,
        path: VirtualPath,
        access: FileAccess,
        destination: Option<VirtualPath>,
    },
    Database {
        namespace: &'a str,
        name: Option<&'a str>,
        action: DatabaseAction,
    },
}
impl<'a> AuthorizationRequest<'a> {
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Self::Describe | Self::Forbidden => None,
            Self::Call { namespace, .. }
            | Self::File { namespace, .. }
            | Self::Database { namespace, .. } => Some(namespace),
        }
    }
}

/// The core decides which additional fact the shell must check before executing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AuthorizationPlan<'a> {
    Allowed,
    VerifyDatabase {
        name: &'a str,
        expected_identity: &'a str,
    },
}

pub(crate) fn plan_authorization<'a>(
    endpoint: &'a EndpointAccess,
    request: &'a AuthorizationRequest<'_>,
    namespace_identity: Option<&str>,
) -> Result<AuthorizationPlan<'a>> {
    match request {
        AuthorizationRequest::Describe => return Ok(AuthorizationPlan::Allowed),
        AuthorizationRequest::Forbidden => return Err(Error::denied()),
        AuthorizationRequest::Call { function, .. } => {
            let namespace = namespace_identity.ok_or_else(Error::denied)?;
            return if endpoint
                .calls
                .iter()
                .any(|(ns, name)| ns == namespace && name == function)
            {
                Ok(AuthorizationPlan::Allowed)
            } else {
                Err(Error::denied())
            };
        }
        AuthorizationRequest::File { .. } | AuthorizationRequest::Database { .. } => {}
    }
    if namespace_identity != Some(endpoint.namespace.as_str()) {
        return Err(Error::denied());
    }
    match request {
        AuthorizationRequest::File {
            path,
            access,
            destination,
            ..
        } => {
            endpoint.file(path, *access)?;
            if let Some(destination) = destination {
                endpoint.file(destination, FileAccess::Write)?;
            }
            Ok(AuthorizationPlan::Allowed)
        }
        AuthorizationRequest::Database { name, action, .. } => {
            let name = name.ok_or_else(Error::denied)?;
            let (mode, identity) = endpoint.databases.get(name).ok_or_else(Error::denied)?;
            let allowed = match action {
                DatabaseAction::Query | DatabaseAction::Inspect | DatabaseAction::Migrations => {
                    true
                }
                DatabaseAction::Execute => {
                    matches!(mode, DatabaseAccess::Write | DatabaseAccess::Migrate)
                }
                DatabaseAction::Migrate => *mode == DatabaseAccess::Migrate,
                DatabaseAction::List | DatabaseAction::Create | DatabaseAction::Drop => false,
            };
            if !allowed {
                return Err(Error::denied());
            }
            Ok(AuthorizationPlan::VerifyDatabase {
                name,
                expected_identity: identity,
            })
        }
        AuthorizationRequest::Call { .. }
        | AuthorizationRequest::Describe
        | AuthorizationRequest::Forbidden => unreachable!("handled above"),
    }
}

pub(crate) fn verify_database_identity(expected: &str, current: Option<&str>) -> Result<()> {
    if current == Some(expected) {
        Ok(())
    } else {
        Err(Error::denied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileAction;
    use serde_json::json;
    use state_store::{Arguments, Tool};

    fn endpoint(mode: DatabaseAccess) -> EndpointAccess {
        EndpointAccess {
            namespace: "ns-id".into(),
            databases: BTreeMap::from([("app".into(), (mode, "db-id".into()))]),
            files: BTreeMap::from([
                (VirtualPath::parse("/notes").unwrap(), FileAccess::Read),
                (VirtualPath::parse("/out").unwrap(), FileAccess::Write),
            ]),
            calls: BTreeSet::from([("callee-id".into(), "notify".into())]),
        }
    }

    #[test]
    fn database_policy_requires_the_right_grant_and_a_live_pin() {
        for mode in [
            DatabaseAccess::Read,
            DatabaseAccess::Write,
            DatabaseAccess::Migrate,
        ] {
            let endpoint = endpoint(mode);
            for action in DatabaseAction::ALL {
                let request = AuthorizationRequest::Database {
                    namespace: "a-friendly-name",
                    name: Some("app"),
                    action: *action,
                };
                let allowed = match action {
                    DatabaseAction::Query
                    | DatabaseAction::Inspect
                    | DatabaseAction::Migrations => true,
                    DatabaseAction::Execute => mode != DatabaseAccess::Read,
                    DatabaseAction::Migrate => mode == DatabaseAccess::Migrate,
                    DatabaseAction::List | DatabaseAction::Create | DatabaseAction::Drop => false,
                };
                let plan = plan_authorization(&endpoint, &request, Some("ns-id"));
                assert_eq!(plan.is_ok(), allowed, "{mode:?} {action:?}");
                if let Ok(AuthorizationPlan::VerifyDatabase {
                    name,
                    expected_identity,
                }) = plan
                {
                    assert_eq!(name, "app");
                    assert!(verify_database_identity(expected_identity, Some("db-id")).is_ok());
                    assert!(
                        verify_database_identity(expected_identity, Some("replacement-id"))
                            .is_err()
                    );
                    assert!(verify_database_identity(expected_identity, None).is_err());
                }
                assert!(plan_authorization(&endpoint, &request, Some("other-ns")).is_err());
            }
        }
        let endpoint = endpoint(DatabaseAccess::Read);
        for name in [None, Some("ungranted")] {
            let request = AuthorizationRequest::Database {
                namespace: "ns",
                name,
                action: DatabaseAction::Query,
            };
            assert!(plan_authorization(&endpoint, &request, Some("ns-id")).is_err());
        }
    }

    #[test]
    fn file_policy_checks_source_and_destination_independently() {
        let endpoint = endpoint(DatabaseAccess::Read);
        for (action, path, destination, allowed) in [
            (FileAction::Read, "/notes//./file", None, true),
            (FileAction::Read, "/notes-other/file", None, false),
            (FileAction::Write, "/notes/file", None, false),
            (FileAction::Write, "/out/file", None, true),
            (FileAction::Copy, "/notes/file", Some("/out/copied"), true),
            (
                FileAction::Copy,
                "/notes/file",
                Some("/notes/copied"),
                false,
            ),
            (FileAction::Move, "/notes/file", Some("/out/moved"), false),
            (FileAction::Move, "/out/file", Some("/out/moved"), true),
        ] {
            let args = Arguments::parse(
                Tool::File,
                json!({"action":action, "namespace":"ns", "path":path, "destination":destination}),
            )
            .unwrap();
            let request = AuthorizationRequest::from_arguments(&args).unwrap();
            assert_eq!(
                plan_authorization(&endpoint, &request, Some("ns-id")).is_ok(),
                allowed,
                "{action:?} {path}"
            );
            assert!(plan_authorization(&endpoint, &request, Some("other-ns")).is_err());
        }
        let args = Arguments::parse(
            Tool::File,
            json!({"action":"read", "namespace":"ns", "path":"/notes/../secret"}),
        )
        .unwrap();
        assert!(AuthorizationRequest::from_arguments(&args).is_err());
    }

    #[test]
    fn calls_use_resolved_identity_and_discovery_needs_no_namespace() {
        let endpoint = endpoint(DatabaseAccess::Read);
        assert_eq!(
            plan_authorization(&endpoint, &AuthorizationRequest::Describe, None).unwrap(),
            AuthorizationPlan::Allowed
        );
        assert!(plan_authorization(&endpoint, &AuthorizationRequest::Forbidden, None).is_err());
        let request = AuthorizationRequest::Call {
            namespace: "renamed-callee",
            function: "notify",
        };
        assert!(plan_authorization(&endpoint, &request, Some("callee-id")).is_ok());
        assert!(plan_authorization(&endpoint, &request, Some("renamed-callee")).is_err());
        let ungranted = AuthorizationRequest::Call {
            namespace: "callee",
            function: "other",
        };
        assert!(plan_authorization(&endpoint, &ungranted, Some("callee-id")).is_err());
    }
}

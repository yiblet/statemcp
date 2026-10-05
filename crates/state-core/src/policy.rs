//! Namespace isolation is the only boundary on published function state access.
use crate::{Error, Result};
use state_store::{Arguments, NamespaceAction};

#[derive(Clone)]
pub(crate) enum Access {
    Owner,
    Namespace(String),
}

pub(crate) enum Target<'a> {
    ApiDiscovery,
    Namespace(&'a str),
    GlobalState,
}

/// Namespace creation, enumeration, and copying affect state outside a function's scope.
pub(crate) fn target(arguments: &Arguments) -> Target<'_> {
    match arguments {
        Arguments::Namespace(args) => match args.action {
            NamespaceAction::Create | NamespaceAction::List | NamespaceAction::Copy => {
                Target::GlobalState
            }
            NamespaceAction::Get | NamespaceAction::Update | NamespaceAction::Delete => {
                Target::Namespace(args.namespace.as_deref().expect("parsed namespace"))
            }
        },
        Arguments::Describe(args) if args.namespace.is_none() => Target::ApiDiscovery,
        _ => Target::Namespace(arguments.namespace().expect("scoped request namespace")),
    }
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
    use serde_json::json;
    use state_store::Tool;

    #[test]
    fn isolation_compares_stable_namespace_identities() {
        assert!(check_namespace("id-a", "id-a").is_ok());
        assert_eq!(
            check_namespace("id-a", "id-b").unwrap_err().code,
            "PERMISSION_DENIED"
        );
    }

    #[test]
    fn global_state_operations_are_distinct_from_api_discovery() {
        for action in ["create", "list", "copy"] {
            let args = Arguments::parse(
                Tool::Namespace,
                json!({"action":action, "namespace":"own", "name":"other"}),
            )
            .unwrap();
            assert!(matches!(target(&args), Target::GlobalState));
        }
        let args = Arguments::parse(Tool::Describe, json!({"mode":"full"})).unwrap();
        assert!(matches!(target(&args), Target::ApiDiscovery));
        let args = Arguments::parse(Tool::Describe, json!({"namespace":"other"})).unwrap();
        assert!(matches!(target(&args), Target::Namespace("other")));
    }
}

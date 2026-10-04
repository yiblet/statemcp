//! Typed operation identifiers shared by validation, authorization, and storage.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal $(| $alias:literal)*),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $wire $(, alias = $alias)*)] $variant),+
        }
        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                const NAMES: &[(&str, $name)] = &[
                    $(($wire, $name::$variant), $(($alias, $name::$variant),)*)+
                ];
                NAMES.iter().find_map(|(name, variant)| (*name == value).then_some(*variant))
                    .ok_or_else(|| Error::invalid(concat!("unknown ", stringify!($name))))
            }
        }
    };
}

wire_enum!(Tool {
    Namespace => "state_namespace",
    File => "state_fs",
    Database => "state_db",
    Function => "state_function",
    Call => "state_call",
    Execute => "state_execute",
    Describe => "state_describe",
});
impl Tool {
    pub fn public_name(self) -> &'static str {
        match self {
            Self::Namespace => "namespace",
            Self::File => "fs",
            Self::Database => "db",
            Self::Function => "function",
            Self::Call => "call",
            Self::Execute => "execute",
            Self::Describe => "describe",
        }
    }
    pub fn operation(self, args: &Value) -> Result<Operation> {
        self.with_action(args["action"].as_str())
    }
    pub fn with_action(self, action: Option<&str>) -> Result<Operation> {
        let action = || action.ok_or_else(|| Error::invalid("action must be a string"));
        Ok(match self {
            Self::Namespace => Operation::Namespace(
                action()?
                    .parse()
                    .map_err(|_| Error::invalid("unknown namespace action"))?,
            ),
            Self::File => Operation::File(
                action()?
                    .parse()
                    .map_err(|_| Error::invalid("unknown file action"))?,
            ),
            Self::Database => Operation::Database(
                action()?
                    .parse()
                    .map_err(|_| Error::invalid("unknown database action"))?,
            ),
            Self::Function => Operation::Function(
                action()?
                    .parse()
                    .map_err(|_| Error::invalid("unknown function action"))?,
            ),
            Self::Call => Operation::Call,
            Self::Execute => Operation::Execute,
            Self::Describe => Operation::Describe,
        })
    }
}

wire_enum!(NamespaceAction {
    Create => "create",
    List => "list",
    Get => "get",
    Update => "update" | "rename",
    Copy => "copy",
    Delete => "delete",
});
wire_enum!(FileAction {
    Read => "read",
    Stat => "stat",
    List => "list",
    Write => "write",
    Append => "append",
    Move => "move",
    Copy => "copy",
    Delete => "delete",
});
wire_enum!(DatabaseAction {
    List => "list",
    Create => "create",
    Drop => "drop",
    Query => "query",
    Execute => "execute",
    Inspect => "inspect",
    Migrations => "migrations",
    Migrate => "migrate",
});
wire_enum!(FunctionAction {
    List => "list",
    Get => "get",
    Remove => "remove",
    Declare => "declare",
    Update => "update",
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Namespace(NamespaceAction),
    File(FileAction),
    Database(DatabaseAction),
    Function(FunctionAction),
    Call,
    Execute,
    Describe,
}
impl Operation {
    pub fn tool(self) -> Tool {
        match self {
            Self::Namespace(_) => Tool::Namespace,
            Self::File(_) => Tool::File,
            Self::Database(_) => Tool::Database,
            Self::Function(_) => Tool::Function,
            Self::Call => Tool::Call,
            Self::Execute => Tool::Execute,
            Self::Describe => Tool::Describe,
        }
    }
    pub fn action_name(self) -> Option<&'static str> {
        match self {
            Self::Namespace(action) => Some(action.as_str()),
            Self::File(action) => Some(action.as_str()),
            Self::Database(action) => Some(action.as_str()),
            Self::Function(action) => Some(action.as_str()),
            Self::Call | Self::Execute | Self::Describe => None,
        }
    }
    pub fn public_name(self) -> String {
        let family = self.tool().public_name();
        if let Some(action) = self.action_name() {
            format!("{family}.{action}")
        } else {
            family.to_owned()
        }
    }
    pub fn from_public_name(name: &str) -> Result<Self> {
        let (family, action) = name
            .split_once('.')
            .map_or((name, None), |(family, action)| (family, Some(action)));
        let tool = Tool::ALL
            .iter()
            .copied()
            .find(|tool| tool.public_name() == family)
            .ok_or_else(|| Error::invalid("unknown statemcp tool"))?;
        let operation = tool.with_action(action)?;
        // Aliases belong to the legacy/CLI boundary, not the public MCP surface.
        if operation.action_name() != action {
            return Err(Error::invalid("unknown statemcp tool"));
        }
        Ok(operation)
    }
    pub fn mutates_resource(self) -> bool {
        matches!(
            self,
            Self::File(
                FileAction::Write
                    | FileAction::Append
                    | FileAction::Move
                    | FileAction::Copy
                    | FileAction::Delete
            ) | Self::Database(
                DatabaseAction::Create
                    | DatabaseAction::Drop
                    | DatabaseAction::Execute
                    | DatabaseAction::Migrate
            ) | Self::Function(
                FunctionAction::Declare | FunctionAction::Update | FunctionAction::Remove
            )
        )
    }
    pub fn publishes_namespace(self) -> bool {
        matches!(
            self,
            Self::Namespace(
                NamespaceAction::Create | NamespaceAction::Copy | NamespaceAction::Update
            )
        )
    }
}

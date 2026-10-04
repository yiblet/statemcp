//! Borrowed boundary names become small, allocation-free selectors.
macro_rules! selector {
    ($name:ident { $($variant:ident => $($wire:literal)|+),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum $name { $($variant),+ }
        impl $name {
            pub fn parse(value: &str) -> Option<Self> {
                const NAMES: &[(&str, $name)] = &[$($(($wire, $name::$variant),)+)+];
                NAMES.iter().find_map(|(name, variant)| (*name == value).then_some(*variant))
            }
        }
    };
}

selector!(Field {
    Inputs => "inputs",
    Mode => "mode",
    Arguments => "arguments",
    Schema => "input_schema" | "output_schema",
    Recursive => "recursive",
    Params => "params",
    Databases => "databases",
    Files => "files",
    Calls => "calls",
    Migrations => "migrations",
});
selector!(SchemaKeyword {
    Reference => "$ref" | "$dynamicRef" | "$recursiveRef",
    Dialect => "$schema",
    Id => "$id",
    InstanceData => "const" | "enum" | "default" | "examples",
    SchemaMap => "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" | "dependencies",
});
selector!(SchemaDialect {
    Draft04 => "http://json-schema.org/draft-04/schema#",
    Draft06 => "http://json-schema.org/draft-06/schema#",
    Draft07 => "http://json-schema.org/draft-07/schema#",
    Draft2019 => "https://json-schema.org/draft/2019-09/schema",
    Draft2020 => "https://json-schema.org/draft/2020-12/schema",
});

use state_store::{DatabaseAction, FileAction};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostFunction {
    Mcp,
    Call,
    Database(DatabaseAction),
    File(FileAction),
}
impl HostFunction {
    pub fn parse(value: &str) -> Option<Self> {
        const NAMES: &[(&str, HostFunction)] = &[
            ("mcp", HostFunction::Mcp),
            ("call", HostFunction::Call),
            ("db_query", HostFunction::Database(DatabaseAction::Query)),
            (
                "db_execute",
                HostFunction::Database(DatabaseAction::Execute),
            ),
            (
                "db_inspect",
                HostFunction::Database(DatabaseAction::Inspect),
            ),
            ("read_text", HostFunction::File(FileAction::Read)),
            ("write_text", HostFunction::File(FileAction::Write)),
        ];
        NAMES
            .iter()
            .find_map(|(name, function)| (*name == value).then_some(*function))
    }
}

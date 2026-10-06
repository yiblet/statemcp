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

selector!(HostFunction {
    Mcp => "mcp",
    Call => "call",
    DbQuery => "db_query",
    DbExecute => "db_execute",
    DbInspect => "db_inspect",
    ReadText => "read_text",
    WriteText => "write_text",
});

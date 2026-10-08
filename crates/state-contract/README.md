# state-contract

Typed inputs and results shared by StateMCP's services, storage adapters,
runtimes, and transports. Import contracts from their owning capability module:
`namespace`, `file`, `database`, `function`, `execution`, or `discovery`.
`publication` contains commit results; `operations` parses external tool names
into exhaustive operation enums. `schema::schema` generates argument schemas.

`VirtualPath` retains a normalized absolute path without parent traversal.
`ParseError` reports malformed contract values independently of implementation
errors. `ErrorCode` uses stable `SCREAMING_SNAKE_CASE` names through `as_str`,
`Display`, and Serde. Layer-specific errors add messages and implementation context.

This crate contains no services, persistence, interpreter, or transport machinery.
Its normal dependencies are Serde, serde_json, and Schemars; it has no dependency
on any other workspace crate. JSON values represent user-defined schemas,
arguments, and results. Known metadata and request fields use named types.

Published declaration field order, omitted fields, and receipt serialization
retain the existing storage format. SHA-256 is a test dependency used to verify
that declaration hashes remain unchanged.

Run its crate-local checks with `cargo test -p state-contract`.

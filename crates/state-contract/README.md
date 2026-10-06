# state-contract

Shared error categories for StateMCP's storage, domain, service, and runtime
boundaries. `ErrorCode` is an exhaustive enum; `as_str`, `Display`, and Serde
serialization use the stable `SCREAMING_SNAKE_CASE` wire names.

This crate contains no execution, persistence, or transport logic. It depends
only on Serde, so the other crates can share error categories without depending
on each other. Context and messages belong to each layer's structured error type.

Run its crate-local checks with `cargo test -p state-contract`.

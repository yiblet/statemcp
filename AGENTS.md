# Write good Rust

- Avoid stringly typed control flow. Parse external names into enums at the
  boundary, then use typed values internally. Keep wire names and serialization
  compatible unless a change explicitly requires otherwise. Use explicit parsers
  and existing types; do not add `strum`.
- Use structs, enums, and newtypes to express meaningful concepts and invariants.
  Prefer exhaustive matches. Add types when they prevent mistakes or make code
  easier to understand; do not wrap every value or introduce redundant types.
- Avoid unnecessary allocations. Prefer borrowed data, static strings, and typed
  structs or enums over temporary strings, cloned values, and JSON objects when
  the structure is known. Use JSON where it belongs: protocol boundaries and
  user-defined data.
- Separate concerns. Keep transport, argument parsing, validation, authorization,
  execution, and persistence distinct. Parse and validate once where practical;
  pass typed values between layers.
- Parse, don't validate: constructors should return domain values that retain
  established invariants. Separate structural parsing from decisions based on
  current state; avoid checking input and then passing the unchecked value onward.
- Keep a functional core and an imperative shell. Pure rules receive facts and
  return decisions or typed plans. The shell fetches facts and executes plans,
  retaining transaction, concurrency, and idempotency guarantees. Pass time and
  randomness into rules when they need them.
- Use hexagonal boundaries in domain terms. Translate transport JSON, stored
  records, and runtime representations in adapters. Keep infrastructure out of
  policy functions; a focused function or module can be a port without a trait.
- Use traits for meaningful contracts and interchangeable implementations.
  Prefer concrete types when there is only one implementation and no useful
  abstraction boundary. Keep helpers focused and control flow easy to follow.
- Use Clap for CLI argument parsing, `anyhow` with context for application-level
  errors, and structured error types for service and protocol contracts.
- Use `tracing` for diagnostics. Send logs to stderr so stdout remains suitable
  for MCP and machine-readable CLI results. Keep blocking SQLite and runtime
  work off Tokio's async workers.
- Use typed request and record envelopes for known fields. Keep `serde_json::Value`
  in the fields that hold agent-defined data or schemas, rather than using it for
  an entire request or persisted record. Serialize at boundaries, not between
  internal collaborators.
- Test observable behavior and failure paths, including protocol interoperability,
  permission enforcement, transactions, and concurrency. Keep tests deterministic
  and run the checks relevant to the change.

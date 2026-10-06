# Write good Rust

Write Rust in the style of typed functional programming, similar to OCaml.
Use types to represent meaning, alternatives, and established invariants.

## Types and invariants

- Keep inputs, intermediate values, decisions, and results typed throughout the
  call path. Adding typed wrappers around a dynamic implementation does not
  satisfy this. When refactoring toward stronger types, follow the data through
  every layer and remove the dynamic implementation.
- Prefer direct, named functions with specific parameter and result types. Do
  not route ordinary internal calls through generic request envelopes, operation
  dispatchers, or adapters that reconstruct those envelopes.
- Avoid stringly typed control flow. Parse external names into enums at the
  boundary, then use typed values internally. Keep wire names and serialization
  compatible unless a change explicitly requires otherwise. Use explicit parsers
  and existing types; do not add `strum`.
- Avoid numeric codes as discriminants. Actual text, counts, and measurements
  can remain strings and numbers.
- Use structs, enums, and newtypes to express meaningful concepts and invariants.
  Prefer exhaustive matches. Add types when they prevent mistakes or make code
  easier to understand; do not wrap every value or introduce redundant types.
- Model valid combinations directly. Prefer enums with data per variant over
  structs whose flags and optional fields allow invalid combinations.
- Keep fields private when arbitrary mutation would invalidate a type.
  Constructors establish invariants; methods preserve them.
- Parse, don't validate: constructors should return domain values that retain
  established invariants. Separate structural parsing from decisions based on
  current state; avoid checking input and then passing the unchecked value onward.

## Ownership, mutation, and allocation

- Mutation is acceptable; keep it localized in focused functions or clearly
  bounded sections. Avoid interleaving mutation with unrelated parsing, policy,
  or I/O. Make state changes and their ordering easy to see, and preserve
  invariants across each completed operation.
- Prefer simple ownership: borrow, move, and destructure before cloning. Do not
  clone merely to satisfy the borrow checker without considering a simpler
  ownership structure. Accept a justified clone when avoiding it would spread
  lifetime parameters through otherwise simple APIs or materially complicate
  the design. Explain non-obvious or expensive clones where they occur.
- Do not reach for `Arc`, locks, interior mutability, or `'static` requirements
  to avoid deciding who owns a value. Use them when shared ownership or
  concurrency actually requires them.
- Avoid unnecessary allocations. Prefer borrowed data, static strings, and typed
  structs or enums over temporary strings, cloned values, and JSON objects when
  the structure is known. Use JSON where it belongs: protocol boundaries and
  user-defined data.
- Avoid unnecessary intermediate collections and serialization round trips.

## Architecture and boundaries

- Separate concerns. Keep transport, argument parsing, validation, authorization,
  execution, and persistence distinct. Parse and validate once where practical;
  pass typed values between layers.
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
- Use typed request and record envelopes at transport and storage boundaries.
  Reserve `serde_json::Value` for arbitrary agent-defined data or schemas. Use
  types for known request fields, metadata, configuration, records, and results.
  Serialize at external boundaries, not between internal collaborators.

## Errors and panic safety

- Use Clap for CLI argument parsing, `anyhow` with context for application-level
  errors, and structured error types for service and protocol contracts.
- Keep errors typed internally, with meaningful variants. Never make decisions
  by matching error messages. Add human-readable context at application boundaries.
- Prefer `Result`, `Option`, `?`, and exhaustive matches over panics. Avoid
  `unwrap()`, `expect()`, unchecked indexing, `panic!()`, `unreachable!()`, and
  similar panicky logic.
- If a potentially panicking operation is necessary, place a `// SAFETY:` comment
  immediately before each operation explaining why it cannot panic. Establish
  that proof within the current function. It may rely on invariants enforced by
  types available in that function, provided safe code cannot violate them. It
  must not depend on caller conventions, validation elsewhere, or assumptions
  about how the function is normally called. If the proof requires explaining
  the call chain, change the code or types instead.
- Treat malformed input and inconsistent external or persisted state as
  recoverable errors. Do not silently discard errors with `.ok()` or replace
  failures with defaults unless that fallback is intentional behavior.

## Blocking and async execution

- Keep blocking I/O and async I/O separate. Use async I/O in async execution
  paths; isolate unavoidable blocking I/O and blocking runtime work in synchronous
  functions executed on dedicated threads or through `spawn_blocking`. Never run
  blocking SQLite, filesystem, network, or runtime operations on Tokio's async
  workers. Keep transactions within the blocking execution boundary and pass
  owned inputs and results across it. Do not use nested runtimes or `block_on`
  inside async code to bridge back to synchronous execution.

## Documentation and diagnostics

- Document public interfaces with Rust doc comments explaining their purpose,
  contract, and relevant errors or side effects. Explain tricky reasoning,
  invariants, ordering constraints, and tradeoffs in comments. Avoid comments
  that merely repeat the code.
- Use `tracing` for diagnostics. Send logs to stderr so stdout remains suitable
  for MCP and machine-readable CLI results.

## Tests and final review

- Prefer in-module `#[cfg(test)]` tests alongside the code they exercise over
  separate integration tests in a `tests/` directory.
- Test observable behavior and failure paths, including protocol interoperability,
  permission enforcement, transactions, and concurrency. Keep tests deterministic
  and run the checks relevant to the change.
- Before finishing, trace representative operations from input through execution
  and persistence to output. Check for dynamic internal envelopes, unnecessary
  clones, lost invariants, and panic paths without a local proof.

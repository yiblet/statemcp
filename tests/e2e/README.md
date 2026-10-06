# End-to-end tests

This is the workspace's real-I/O integration suite. Cargo registers
`tests/e2e/mod.rs` as a single target named `e2e`; its sibling files are modules,
not separate test executables. Crate-local `#[cfg(test)]` unit tests remain
in-memory.

```sh
cargo test --test e2e
cargo test --test e2e worker::      # select a scenario group
cargo test --workspace --lib --bins # unit tests only
cargo test --workspace --all-targets # unit and E2E tests
```

The suite covers:

- Physical file handles, reopen/sync/rename/truncate/unlink, symlink metadata,
  shared/exclusive locks across processes, and lock release.
- Real executable help, argument errors, CLI JSON/file/stdin inputs, state
  publication and rollback, maintenance, and persisted example workflows.
- Official SDK stdio and HTTP clients, multiple listeners, shared sessions and
  protocol versions, bind-before-state ordering, and listener release on shutdown.
- Isolated worker execution, invocation and validation, recursive callbacks,
  resource limits, malformed frames/replies, and startup with an empty PATH.
- Aggregate allocator enforcement, fatal memory exits, and subsequent recovery.
- On Linux, actual worker reaping while a callback is running and after host errors.

Tests create temporary files, bind ephemeral localhost ports, and launch child
processes. The suite requires an environment that permits those operations.
Scenarios share a serial guard: unrelated forks can briefly inherit lock
file descriptors before exec, causing spurious contention if real-I/O scenarios
run concurrently. Async tests acquire that guard asynchronously.

The allocator and filesystem probe tests are ignored subprocess entry points.
Their parent tests invoke them with `--ignored --exact`; they are not independent
scenarios. The allocator is installed on the E2E executable, and only its isolated
allocator-probe child arms the process-global limit.

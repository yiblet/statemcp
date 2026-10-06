# state-migrations

Construct a `Migration` with a stable name and positive version, implement
`Migrator::forward` in ordinary Rust, and pass an ordered list of implementors to
`Service::new`. Versions must increase and names must be unique. Keep already
released migrations in the list with unchanged names, versions, and behavior.

The service reads `catalog.migrations` (the `migrations` table in the supplied
catalog connection), whose rows contain `name` and `version`. Stored history must
be an exact prefix of the supplied list. It executes only missing migrations.
It does not use `version.txt` or acquire filesystem locks. The caller guarantees
exclusive root access. Each forward implementation receives the root path and a
SQLite catalog transaction; explicit transaction-control SQL is denied.
File operations should use `Context::fs()`, which receives the overlay store's
injected filesystem. Standalone `Service::run` uses `PhysicalFs`;
`Service::run_with_fs` accepts an explicit filesystem.

An ordinary SQLite catalog commits each migration's changes and history together,
retaining earlier successes if a later migration fails. With `state-store`,
use `Store::open_with_migrations` or `Store::migrate`: these hold `overlay.lock`
exclusively and publish the whole pending batch through the redo journal. Any
failure before publication discards the whole pending batch, including its history.
Filesystem effects outside the catalog require retry-safe migration code.

All APIs are blocking. Run them on a dedicated thread or with `spawn_blocking`
when called from async code. No coordination with the old store's lock is provided;
the initial transition assumes there are no concurrent old-store users.

## Tests

Tests live in crate-local `#[cfg(test)]` modules under `src/`. They use
in-memory storage and embedded execution where applicable; they do not create
host files, bind sockets, or launch processes.

```sh
cargo test -p state-migrations
```

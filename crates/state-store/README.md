# state-store

SQLite VFS overlays for a blocking, root-scoped storage invocation. Uses the
`sqlite-plugin` crate's Rust traits; there is no FUSE mount or filesystem dependency.
The base databases stay untouched while SQL executes. Only changed 4 KiB blocks
are materialized in memory, including partial-block writes and SQLite rollback
journals. Unchanged bytes are read directly from the base files.

Building uses libclang through `sqlite-plugin`'s binding generator. No libclang
or extra storage service is needed at runtime.

```rust
use state_store::{DatabaseName, Store, fs::MemoryFs};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let store = Store::builder().with_fs(MemoryFs::new()).open("/state")?;
let database = DatabaseName::parse("app.sqlite")?;
let transaction = store.begin()?;
{
    let connection = transaction.connect(&database)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS counter(value INTEGER);
         INSERT INTO counter VALUES (1);"
    )?;
} // Close every connection before publishing the outer invocation.
transaction.commit()?;
# Ok(())
# }
```

SQLite `COMMIT` and `ROLLBACK` operate on the private overlay, so even committed
inner transactions can be discarded by dropping the outer invocation. This crate
does not implement SQL authorization or namespace permissions. The service layer
decides whether callers may issue transaction commands and whether SQL errors
abort the entire invocation. Dropping a connection rolls back an outstanding
SQLite transaction before outer publication.

## Publication and recovery

`Store::begin_read` holds `overlay.lock` shared through read invocation cleanup,
so multiple readers can use the same root concurrently. It returns a
`ReadTransaction` with read-only SQLite connections, ending with `finish` or drop.
Read invocations cannot create, update, or delete persistent databases. Queries
can still use private in-memory SQLite scratch tables.

`Store::begin` holds the lock exclusively through write invocation cleanup.
Writers exclude readers and other writers. Contention returns `Error::Busy`
immediately; locks do not wait and read invocations cannot upgrade to writers.
The connection lifetime is tied to its invocation, and each database may have
only one open connection within that invocation. Independent roots use independent
locks. Run the blocking APIs on dedicated threads or through `spawn_blocking` in
async applications.

Opening a clean root also uses shared ownership. When a read invocation or open
finds a recovery journal, it releases its shared lock, acquires an exclusive lock
to recover, then reacquires and rechecks shared ownership before permitting reads.
This prevents queries from observing a partially applied commit. Recovery is never
run under a shared lock.

Outer commit streams changed blocks and file lengths into a checksummed
`overlay.pending` redo journal and syncs it. Renaming that file to `overlay.redo`
and syncing the root directory establishes the durable commit decision. Recovery
validates the entire journal, applies its updates and deletions, syncs every
updated database and the root, then removes the journal and syncs the root again.
Creation, truncation, extension, and deletion participate in the same publication.

A crash before the decision leaves original files unchanged; the incomplete
pending journal is discarded. A crash after the decision is recovered by replaying
the whole committed journal before any reads. Replay is idempotent, including
when some files were already updated. Corrupt committed journals are retained and
fail closed. `Error::RecoveryRequired` means a commit decision may already be
durable: fix the underlying I/O problem and reopen or begin to finish recovery;
do not rerun the invocation assuming it rolled back.

The normal directory layout is:

```text
overlay-data/
├── overlay.lock
├── catalog.sqlite
├── app.sqlite
└── overlay.redo       # Only during publication or interrupted recovery
```

## Boundaries and limits

- Use a dedicated root. Every reader and writer must participate in this lock and
  recovery protocol; ordinary SQLite access can observe incomplete application.
  This store replaces the previous snapshot implementation; existing snapshot roots require a forward format migration.
- File names are validated ASCII basenames ending in `.sqlite`; paths and symlinks
  are rejected. SQLite journals and scratch files never become host files.
- Databases imported from ordinary SQLite must be closed and have no WAL, shared
  memory, or rollback-journal sidecars. WAL mode is disabled in this VFS.
- Main databases use SQLite rollback journals in memory. SQLite's `xSync` completes
  an inner operation; durability is provided only by the outer `commit`.
- Maximum database/scratch file size: 256 MiB. Maximum managed overlay memory:
  64 MiB of blocks. Maximum files/changes per invocation: 1,024. Anonymous SQLite
  scratch files have an independent 64 MiB block budget each. Normal SQL temporary
  tables use `temp_store=MEMORY`; SQLite's own caches and those tables are outside
  the overlay budget.
- This implementation overwrites base files after durable publication; it does
  not retain historical snapshots or create cheap independent namespace copies.
- The redo journal detects corruption with SHA-256. It is not a defense against
  an external process intentionally modifying this exclusively managed directory.

Tests cover actual SQLite commit/rollback behavior, multi-database publication,
small updates, VACUUM/truncation, temporary tables, database deletion/recreation,
root locking, invalid paths/sidecars, corrupt journal handling, and injected I/O
failures before and after the commit decision and during multi-file publication.
All store tests live in crate-local `#[cfg(test)]` modules and use the in-memory
filesystem. The suite tests recovery logic and injected failures; it does not
exercise OS locks, process crashes, or physical power loss.

```sh
cargo test -p state-store
cargo clippy -p state-store --all-targets -- -D warnings
```

## Rust migrations

Use `Store::open_with_migrations(path, &service)` to apply an ordered
`state_migrations::Service` at startup, or `store.migrate(&service)` to apply it explicitly.
The service takes `Box<dyn state_migrations::Migrator>` implementors containing ordinary
Rust forward migration code. `catalog.sqlite` records each applied migration's
name and version in its `migrations` table. The overlay holds an exclusive root
lock through history checks, migration execution, and durable publication. The
whole pending catalog batch rolls back on a migration failure. Direct filesystem
side effects must be safe to retry. There is no transition between the old store's
lock and `overlay.lock`; the initial transition assumes no concurrent old users.

## Filesystems for unit tests

`Store::open` uses the host filesystem. Inject a `MemoryFs` to run the same VFS,
locks, commit journal, and recovery code without accessing the host filesystem:

```rust
use state_store::{Store, fs::MemoryFs};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let fs = MemoryFs::new();
let store = Store::builder().with_fs(fs.clone()).open("/state")?;
// Reopening with the same backend sees the committed databases and root locks.
let reopened = Store::builder().with_fs(fs).open("/state")?;
# Ok(())
# }
```

`state-fs` defines the focused `FileSystem`, `File`, and lock guard contracts.
Custom wrappers can inject I/O failures. Clones of `MemoryFs` share state;
independent instances are isolated. Memory paths are rooted at `/` and reject
`..`. Sync operations are no-ops; this backend tests recovery logic and injected
failures. Physical file and cross-process lock behavior is covered by the workspace
[end-to-end suite](../../tests/e2e/README.md); physical power loss is not simulated.

Migrations receive the injected filesystem through `Context::fs()`. Their direct
file side effects remain outside overlay publication and must be retry-safe.

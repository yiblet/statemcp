# state-domain

The namespace and resource domain layer for statemcp, backed by `state-store`. It does not execute Python or validate endpoint schemas;
the core must validate declarations before handing them to storage.

```rust,no_run
use state_domain::Store;
use state_contract::namespace::{DescriptionChange, NamespaceCreateInput};
use state_contract::database::{DbCreateInput, DbExecuteInput};
let backing = state_store::Store::builder().with_fs(state_store::fs::MemoryFs::new()).open("/state")?;
let store = Store::from_storage(backing)?;
let mut tx = store.begin()?;
tx.create_namespace(&NamespaceCreateInput { name: "notes".into(), description: DescriptionChange::Unchanged })?;
tx.create_database(&DbCreateInput { namespace: "notes".into(), database: "app".into(), expected_revision: None })?;
tx.execute_database(&DbExecuteInput {
    namespace: "notes".into(), database: "app".into(),
    sql: "CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT)".into(),
    params: None, expected_revision: None,
})?;
let commit = tx.commit()?;
# Ok::<(), state_domain::Error>(())
```

## Rust interface

- `Store::open(path) -> Result<Store>` opens physical storage and runs the catalog migrations.
- `Store::begin() -> Result<Transaction>` captures a consistent namespace view.
- Named `Transaction` methods accept specific input structs and return typed records.
- `Transaction::commit() -> Result<CommitOutcome>` publishes every staged change together.
- `Transaction::set_receipt(principal, key, request_hash, result)` stages a receipt.
- `Store::receipt(principal, key) -> Result<Option<Receipt>>` retrieves one.
- `Store::maintenance(retain_receipts) -> Result<MaintenanceReport>` explicitly
  prunes history and unreferenced virtual-file objects and retains the newest N receipts globally.
- Errors expose public `code` and `message` fields. Any operation error poisons the
  invocation: catching it cannot allow partial publication. Drop rolls back.

`commit` returns `CommitOutcome::Published(CommitMetadata)` with generation, revisions, and changed fields.
A repeated identical receipt returns `CommitOutcome::Replayed(ReceiptResult)` and publishes
none of the duplicate attempt's changes. Different request hashes produce
`IDEMPOTENCY_MISMATCH`. Receipt lookup authorization belongs to the caller.

## JSON operations

All resource operations take `namespace` as a name or immutable UUID. Names cannot
alias another namespace UUID. Mutation operations accept `expected_revision`.
Namespace names remain reserved after deletion.

| Tool | Actions and arguments |
| --- | --- |
| `namespace.*` | `create {name}`, `list`, `get {namespace}`, `update {namespace,name}`, `copy {namespace,name}`, `delete {namespace}` |
| `fs.*` | `read`, `stat`, `write`, `append`, `list`, `delete`, `move`, `copy`; `path` is an absolute virtual POSIX path; writes take exactly one of `text` or `base64`; moves/copies take `destination`; directory deletion requires `recursive:true` |
| `db.*` | `create`, `list`, `drop`, `query`, `execute`, `inspect`, `migrate`, `migrations`; database operations take `database`; SQL takes `sql` and optional array `params`; migrations take `migrations:[{id,sql}]` |
| `function.*` | `declare`/`update`, `get`, `list`, `remove`; each endpoint has `name`; declaration includes `file`, `symbol`, schemas; optional `expected_version` |

Namespace/file/database/function lists are wrapped in `namespaces`, `entries`,
`databases`, or `functions` respectively. Function get/declare returns metadata
**directly**, including pinned `source`, `modules` (virtual paths to Python source),
`source_hash`, `version`, and `abi_version`. Source edits do not change declarations.
The public `state-core` API compacts declaration acknowledgments; these full
metadata responses describe the lower-level storage API.
Functions resolve database names within their namespace on each call; declarations
do not prevent database dropping or recreation.

SQL query/execute takes one statement and returns
`{columns:[...],rows:[[...]],rows_affected:n}`. Duplicate column names are retained.
BLOBs use `{"$base64":"..."}`; integers outside JavaScript's exact range use
`{"$integer":"9223372036854775807"}`. These tagged forms also work as parameters.
No values are interpolated into SQL. Multiple statements belong in a migration.

Migrations are ordered append-only IDs with SHA-256 checksums. Submit an exact
history prefix (optionally with new migrations appended), or a batch of entirely
new IDs. Existing IDs must occur in their original order starting at the first
migration. IDs are opaque strings: lexical ordering is not assumed. Exact replay
writes nothing. A migration batch and its external catalog ledger publish together.
Application SQL sees no internal migration tables.

`inspect` returns `name`, `id`, `snapshot`, `staged`, `schema_fingerprint`, `migrations`, the raw ordered
`schema` query, and `tables`. Table entries include `name`, `type`, `sql`, `columns`
(query result), `indexes` (objects with detailed index-column query results), and
`foreign_keys` (query result). Views and triggers also appear in `schema`.

## Storage boundary

`state-domain` owns resource identities, manifests, catalog schema, receipts,
revision guards, virtual files, pinned function declarations, and SQL permissions.
It has no filesystem handles, snapshot copies, staging directories, root locks,
or independent durable publication path. `state-store` supplies those storage
services through its SQLite overlay and injected filesystem.

Use `Store::from_storage(backing)` with a memory-backed `state-store::Store` in
tests or with an explicitly configured storage builder. `Store::open` defaults to
physical storage. Initialization is a Rust `state-migrations::Migrator`; applied
versions live in the catalog's migrations table. The initial transition assumes
there are no concurrent users of the old store. Existing resource snapshot files
require a separate format migration and are rejected instead of silently opened
as empty databases.

The catalog and every changed application database publish in one outer overlay
invocation. A domain operation failure poisons publication, even if its caller
catches the error. SQL transaction/savepoint commands, attachment, unsafe PRAGMAs,
and file/extension functions are denied by the domain's authorizer. SQLite
connections remain alive through an invocation so temporary tables and connection
state persist across operations. They close before outer publication.

Every namespace owns independent application database files. Copying a namespace
uses SQLite backup to copy its current database view, including pending writes,
into fresh database identities. Temporary tables are not copied. Copied virtual
files and function sources may share immutable content-addressed objects.

Domain invocations take exclusive root ownership. Concurrent invocations fail
immediately with `CONFLICT`; retry after the active invocation finishes. Receipt
retries return the exact committed result. Physical storage locking, recovery,
and durability are documented in [state-store](../state-store/README.md).

The compatibility field `db.inspect.snapshot` now identifies the backing database
file, not a historical snapshot or concurrency token. `staged` reports writes in
the current invocation. Namespace revisions change for database writes even when
the backing file identity stays fixed.

Maintenance removes obsolete metadata and unreferenced virtual-file objects and
retains the newest requested number of receipts. Deleted resource databases are
removed in their original atomic invocation; there are no orphan snapshots or
staging directories to collect. The legacy maintenance report fields for those
kinds remain zero. Maintenance is explicit; expired idempotency keys may execute
again.

Limits remain 8 MiB per virtual file, 256 MiB per application database, 1 MiB per
SQL statement/value/result/receipt, 10,000 SQL result rows, and a two-second SQL
progress deadline. The overlay additionally bounds changed blocks to 64 MiB per
invocation. Domain tests use the memory filesystem; disk is tested only by the
physical filesystem adapter.

## Tests

Tests live in crate-local `#[cfg(test)]` modules under `src/`. They use
in-memory storage and embedded execution where applicable; they do not create
host files, bind sockets, or launch processes.

```sh
cargo test -p state-domain
```

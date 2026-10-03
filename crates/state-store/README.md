# state-store

The storage layer for State MCP: an immutable catalog plus separate SQLite files
for application databases. It does not execute Python or validate endpoint schemas;
the core must validate declarations before handing them to storage.

```rust,no_run
use serde_json::json;
use state_store::Store;
let store = Store::open("./state")?;
let mut tx = store.begin()?;
tx.dispatch("state_namespace", json!({"action":"create", "name":"notes"}))?;
tx.dispatch("state_db", json!({"action":"create", "namespace":"notes", "database":"app"}))?;
tx.dispatch("state_db", json!({
    "action":"execute", "namespace":"notes", "database":"app",
    "sql":"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT)"
}))?;
tx.commit()?;
# Ok::<(), state_store::Error>(())
```

## Rust interface

- `Store::open(path) -> Result<Store>` creates or opens catalog format 1.
- `Store::begin() -> Result<Transaction>` captures a consistent namespace view.
- `Transaction::dispatch(tool, args) -> Result<Value>` handles storage operations.
- `Transaction::commit() -> Result<Value>` publishes every staged change together.
- `Transaction::set_receipt(principal, key, request_hash, result)` stages a receipt.
- `Store::receipt(principal, key) -> Result<Option<Receipt>>` retrieves one.
- Errors expose public `code` and `message` fields. Any dispatch error poisons the
  invocation: catching it cannot allow partial publication. Drop rolls back.

`commit` returns `{generation,revisions:{namespace_id:revision_id},changed}`.
An identical concurrent receipt returns `{replayed:true,result}` and publishes
none of the duplicate attempt's changes. Different request hashes produce
`IDEMPOTENCY_MISMATCH`. Receipt lookup authorization belongs to the caller.

## JSON operations

All resource operations take `namespace` as a name or immutable UUID. Names cannot
alias another namespace UUID. Mutation operations accept `expected_revision`.
Namespace names remain reserved after deletion.

| Tool | Actions and arguments |
| --- | --- |
| `state_namespace` | `create {name}`, `list`, `get {namespace}`, `update {namespace,name}`, `copy {namespace,name}`, `delete {namespace}` |
| `state_fs` | `read`, `stat`, `write`, `append`, `list`, `delete`, `move`, `copy`; `path` is an absolute virtual POSIX path; writes take exactly one of `text` or `base64`; moves/copies take `destination`; directory deletion requires `recursive:true` |
| `state_db` | `create`, `list`, `drop`, `query`, `execute`, `inspect`, `migrate`, `migrations`; database operations take `database`; SQL takes `sql` and optional array `params`; migrations take `migrations:[{id,sql}]` |
| `state_function` | `declare`/`update`, `get`, `list`, `remove`; each endpoint has `name`; declaration includes `file`, `symbol`, schemas and `databases:{name:"read"|"write"|"migrate"}`; optional `expected_version` |

Namespace/file/database/function lists are wrapped in `namespaces`, `entries`,
`databases`, or `functions` respectively. Function get/declare returns metadata
**directly**, including pinned `source`, `source_hash`, `version`, `abi_version`,
and `database_ids:{alias:stable_local_id}`. Source edits do not change declarations.
Removing a database referenced by a declaration returns `CONFLICT`.

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

`inspect` returns `name`, `id`, `schema_fingerprint`, `migrations`, the raw ordered
`schema` query, and `tables`. Table entries include `name`, `type`, `sql`, `columns`
(query result), `indexes` (objects with detailed index-column query results), and
`foreign_keys` (query result). Views and triggers also appear in `schema`.

## Storage and isolation

`catalog.sqlite` stores private metadata, immutable manifests, virtual file bytes,
and receipts. `snapshots/<uuid>.sqlite` stores closed, immutable database files.
Application connections never attach the catalog. Reads open snapshots read-only;
writes copy a snapshot into an invocation's private `staging/<uuid>/` directory.
One writable connection is reused per namespace/database in a root transaction,
so temporary tables and `last_insert_rowid()` persist across callbacks. Temporary
state is not published or inherited by copies.

Committed namespace forks reuse the same manifest and snapshot references: they
copy no file/database payload. Forking a namespace with staged database changes
freezes the current main database into another snapshot, preserving the original
connection's temporary state. Subsequent writes diverge. This implementation uses
the portable full-file copy fallback, without reflink acceleration. Each database
needs one working copy per root transaction, plus a frozen copy for each staged
fork. Database size is a disk limit, not an interpreter-memory requirement.

Publication closes working connections, fsyncs files, moves them into `snapshots`,
and fsyncs that directory **before** a synchronous catalog transaction publishes
all namespace heads and the receipt. Conservative global generation validation
rejects conflicting writes, including write skew across read dependencies. A
read-only transaction can finish against its captured view. Directory fsync is a
platform requirement. Managed files must not be modified out of band.

The SQL authorizer denies attachment, transaction/savepoint control, unsafe
PRAGMAs, extension/file functions, and unrecognized virtual-table modules. It is
installed before user SQL is prepared on every connection. Ordinary schema
PRAGMAs and table-valued introspection work. Allowed virtual-table modules are
SQLite's built-in FTS5 and RTree families, when available. The query API additionally
requires a read-only statement. Other transaction control belongs to this library.

## Bounds and current limits

- 8 MiB per virtual file.
- 256 MiB on disk per application database (65,536 4 KiB pages).
- 1 MiB per SQL statement, SQL value, SQL result, or receipt; 10,000 result rows.
- Two-second SQLite progress deadline per statement/migration batch; a core
  invocation deadline and total memory/operation budget must wrap these limits.
- SQLite expression depth, column count, bytecode size and worker threads capped.
- Lists are currently unpaginated; the core must bound total output.
- Historical revisions, tombstones, receipts, and unreachable snapshots are
  retained. There is no automatic garbage collection or receipt expiration here.
  Crash leftovers are unreachable and harmless but consume disk until maintenance.

The tests cover fork payload counts, staged divergence, connection-state reuse,
rollback across namespaces, generation conflicts, migration integrity/replay,
introspection, SQL boundaries, receipts, pinned declarations, result encodings,
deadlines, and abrupt process exits before and after atomic publication. They do
not simulate power loss at every individual filesystem operation.

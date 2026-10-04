# statemcp

A Rust library and JSON-RPC server for agent-defined stateful APIs. Each namespace
contains virtual files, named SQLite databases, and published Python functions
executed by the embedded Pydantic Monty Rust runtime. Python installation is only
needed for the optional demo client.

```sh
cargo build
cargo run -- stdio /path/to/state
```

The stdio transport speaks newline-delimited JSON-RPC; stdout contains protocol
messages. It uses MCP-style initialization, `tools/list`, and `tools/call`, with
one deliberate difference: tool results contain `content` as the returned JSON
value directly. Standard MCP clients require an adapter for this result shape.
Initialize with protocol version `2025-06-18`, send `notifications/initialized`,
then call tools. Errors have `isError: true` and `{error: {code, message}}` content.

The data directory is required as the first positional argument of the chosen
subcommand. There is no default data directory.

For HTTP:

```sh
cargo run -- http /path/to/state --bind 127.0.0.1:8000
cargo run -- http /path/to/state --auth-bearer X
```

POST JSON-RPC requests to `http://127.0.0.1:8000/mcp`. HTTP and stdio use the
same dispatcher and result format. HTTP returns JSON, with a session ID from
initialization required on subsequent requests. DELETE ends a session. The default
listen address is `127.0.0.1:8000`; `--bind` changes it. With `--auth-bearer X`,
every request must include `Authorization: Bearer X`; missing or wrong tokens
receive HTTP 401. Without that flag, authentication is optional. This shared
bearer grants owner access; it is not per-user authorization or OAuth. TLS is
not provided by this listener. Host and browser Origin validation match the
bound address (plus `localhost` when bound to loopback). Proxy host/origin
configuration is a follow-up TODO.

HTTP networking runs on Tokio. SQLite remains synchronous `rusqlite`; complete
invocations run via `spawn_blocking` with at most 16 simultaneous blocking jobs
shared across HTTP sessions. SQLite and Monty host callbacks stay on those
threads, keeping the async network workers responsive.

## Direct CLI operations

All seven tools are available as `statemcp cli <TOOL> <DATA_DIR>`. These commands
open the same managed store directly, using the same validation, transactions,
endpoint grants, and isolated Monty backend as MCP. They can share a data directory
with a running MCP server. No server needs to be running for CLI operations.

```sh
statemcp cli state_namespace ./data create --name notes
statemcp cli state_db ./data create --namespace notes --database app
statemcp cli state_db ./data migrate --namespace notes --database app \
  --migrations '[{"id":"initial","sql":"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT NOT NULL)"}]'
statemcp cli state_fs ./data write --namespace notes --path /api.py --text 'def add(text):
    return db_execute("app", "INSERT INTO notes(text) VALUES (?) RETURNING id, text", [text])["rows"][0]'
statemcp cli state_function ./data declare --namespace notes --name add \
  --file /api.py --symbol add --databases '[{"database":"app","access":"write"}]'
statemcp cli state_call ./data notes add --arguments '{"text":"hello"}'
statemcp cli state_describe ./data --namespace notes --function add
statemcp cli state_execute ./data 'call("notes", "add", {"text": inputs})' --inputs '"composed"'
```

Use `--help` on any tool for its flags. `state_namespace` accepts `rename` as an
alias for its `update` action. `state_execute` accepts inline source, `@script.py`,
or `-` to read Python from stdin. A single script composes multiple operations
atomically; separate CLI commands each commit their own transaction.

Every tool also accepts its complete MCP argument object through `--json`:

```sh
statemcp cli state_namespace ./data --json '{"action":"list"}'
statemcp cli state_function ./data --json @declaration.json
printf '%s' '{"script":"inputs + 1","inputs":41}' | statemcp cli state_execute ./data --json -
```

JSON flags such as `--arguments`, `--params`, `--migrations`, `--input-schema`,
and `--databases` also accept `@FILE` or `-`. Duplicate fields between `--json`
and named arguments are rejected. Binary virtual files use the `--base64` field.

Success prints one JSON value to stdout. Service errors print a JSON error object
to stderr and exit nonzero; argument/input errors print diagnostics to stderr.
Scripts return `{value,stdout}`. Endpoint results are returned directly, without
MCP's result envelope. Use `--idempotency-key` for safe identical
retries of `state_call` and `state_execute`. Concurrent writers can return
`CONFLICT`; retry the complete operation. Maintenance remains
`statemcp maintenance DATA_DIR --retain-receipts N`.

Run the notes, todos, and two-client chat demo:

```sh
cargo build
python3 examples/demo.py --binary target/debug/statemcp --data-dir .statemcp-demo
cargo test --workspace --all-targets
```

The demo publishes [these fixtures](examples/setup.json), adds a note, completes a
todo, and posts a message that another server process polls from the same state.
Run it again to reuse persisted data. The demo setup expects an empty directory or
its own previously installed fixtures; it currently skips existing namespaces.

## MCP tools

MCP exposes 30 tools. Each resource action has its own name and schema; omit the
`action` argument. For example, call `db.query` with
`{"namespace":"notes","database":"app","sql":"SELECT * FROM notes"}`.

| Tools | Operations |
| --- | --- |
| `namespace.*` | `create`, `list`, `get`, `update` (rename), `copy`, `delete` |
| `fs.*` | `read`, `write`, `append`, `stat`, `list`, `delete`, `copy`, `move` |
| `db.*` | `create`, `list`, `drop`, `query`, `execute`, `migrate`, `inspect`, `migrations` |
| `function.*` | `declare`, `update`, `get`, `list`, `remove` |
| `call` | Invoke `{namespace, function, arguments?, expected_version?, idempotency_key?}` |
| `execute` | Run `{script, namespace?, inputs?, idempotency_key?}` |
| `describe` | Discover tool and endpoint contracts |

Python's `mcp()` helper also accepts these names. Existing `state_*` calls remain
supported by the embedded service and persisted Python code, and the CLI keeps
its grouped commands. MCP accepts only the names above.

`describe({})` returns a compact overview with links to discovery modes.
Use `{"mode":"readme"}` to read this bundled README over MCP; the response
contains `title`, `format:"markdown"`, and `text`. Documentation is embedded at
build time and requires no local checkout or network access. Use `{"mode":"runtime"}` for
Python host signatures, result shapes, grants, ABI policy, and a runnable endpoint
example; `{"mode":"full"}` includes all tool schemas and the runtime guide.
Use `{"tool":"db.create"}` to inspect one operation, or
`{"namespace":"notes","function":"add"}` for an endpoint contract.

File responses omit content hashes except in `fs.stat`, where hashes are
available for content verification.

Function declaration/update acknowledgments return `name`, `published`, and
`version`; `function.get` retains pinned source, source hash, database
identities, and ABI metadata. Function lists omit these diagnostic fields.
Database creation returns `name` and `created`; `db.inspect` retains its
UUID and snapshot identifier. Database lists omit snapshots. A snapshot is an
opaque immutable file reference, not a content hash or concurrency token, and
may be collected once unreferenced. Inspection's `staged` flag means pending
writes are not yet represented by that snapshot. No historical read/restore API
accepts snapshot IDs. Direct mutations return the committed namespace `revision`
when they publish a new revision; namespace UUIDs and endpoint `version` remain
available for stable references and concurrency checks.

Runtime discovery reports ABI 1. Incompatible host signatures, invocation
conventions, or result representations require an ABI increment. Only ABI 1 is
supported today; mismatches return `ABI_MISMATCH` with expected/actual versions.
Adapt source and redeclare unsupported functions; there is no automatic ABI
migration.

`tools/list` exposes action-specific argument schemas. SQL accepts bound `params`;
query/execute results are `{columns, rows, rows_affected}`. Query/execute use one
statement; migrations take `[{id, sql}]`. Schema inspection includes tables,
columns, indexes, foreign keys, and ordinary SQLite schema information. Each
named application database is a separate SQLite file; private catalog metadata
is inaccessible through application SQL.

A `function.declare` call pins a virtual source file and callable symbol:

```json
{
  "namespace": "notes", "name": "add",
  "file": "/api.py", "symbol": "add", "databases": [{"database":"app","access":"write"}],
  "input_schema": {
    "type": "object", "properties": {"text": {"type": "string"}},
    "required": ["text"], "additionalProperties": false
  }
}
```

Its file can contain:

```python
def add(text):
    return db_execute("app", "INSERT INTO notes(text) VALUES (?) RETURNING id, text", [text])["rows"][0]
```

Create the namespace, database, table/migration, and virtual file first; the demo
shows all requests. Editing a source file does not change a published endpoint:
explicitly update the declaration. Modules initialize with host effects denied.
Endpoint input/output JSON schemas are checked in Rust. Local schema references
are supported; remote/file reference resolution is disabled.

## Composition and persistence

Scripts return `{value, stdout}`; endpoints return their JSON value directly.
Tool responses use one `content` field containing the JSON result directly.
Objects, arrays, and scalars retain their shape. `isError` distinguishes tool
failures, whose content is `{error:{code,message}}`. There are no text blocks,
`structuredContent`, or extra value wrappers. This is a custom JSON-RPC tool
API: standard MCP clients need an adapter for this result shape. Both HTTP and
stdio use the same dispatcher. Monty
provides `mcp(name, arguments={})`, `call(namespace, function, arguments={})`,
`db_query`, `db_execute`, `db_inspect`, `read_text`, and `write_text`. The database
and file helpers use the script's selected namespace or endpoint's namespace.
There is no package loader or virtual-file import resolver in this MVP.

A root script can compose all tools. Published endpoint bodies use explicit
grants: `databases:[{database,access:"read"|"write"|"migrate"}]`, path-prefix
`files:[{path,access:"read"|"write"}]`, and `calls:[{namespace,function}]`.
Each grant is an object with named fields. Database access is `read`, `write`,
or `migrate`; file access is `read` or `write`. Empty arrays grant nothing.
Unknown fields and duplicate resources are rejected, including duplicate paths
after normalization. Existing persisted map-shaped grants remain readable; new
declarations use the array shape above.

`self` calls follow namespace copies. Other namespace grants resolve to stable
UUIDs; use that UUID in code when an external namespace may be renamed.

Every root operation shares one transaction across nested calls, databases,
files, and namespaces. Failure rolls back its staged effects. Committed
namespace copies share immutable snapshots cheaply; the first subsequent write
uses a full SQLite file copy. Concurrent writes can return `CONFLICT`; retry the
whole operation. `state_call`/`state_execute` receipts with `idempotency_key`
replay identical committed requests; changed arguments with the same key fail.

## Rust embedding

```rust,no_run
use serde_json::json;
use statemcp::State;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = State::open("./state")?;
    state.dispatch("state_namespace", json!({"action":"create", "name":"notes"}))?;
    let result = state.dispatch("state_execute", json!({"script":"inputs + 1", "inputs":41}))?;
    assert_eq!(result["value"], 42);
    Ok(())
}
```

Library embedding executes Monty in process. The supplied server runs Monty in
workers of the same Rust executable to isolate interpreter crashes and enforce
an allocator ceiling; host callbacks and SQLite stay in the parent. This is a
process boundary, not a complete OS sandbox. The service is for trusted local
owners. `dispatch_as` scopes receipts to an embedding-provided identity; it does
not authenticate users or implement multi-user authorization.

Default limits include five seconds per root execution, 256 tool operations,
16 nested executions, 64 MiB allocator ceiling per server worker, 1 MiB runtime
JSON/SQL results, 8 MiB virtual files, and 256 MiB per SQLite database. The worker
memory limit is per process; embedded mode has no aggregate hard memory ceiling.
Synchronous host callbacks cannot be interrupted mid-callback. Lists are bounded
but unpaginated. See the [core](crates/state-core/README.md),
[storage](crates/state-store/README.md), and [runtime](crates/state-runtime/README.md)
contracts for exact behavior and limits.

Deferred work is recorded in source `TODO` comments in [chat.py](examples/chat.py),
[demo.py](examples/demo.py), and the implementation (`rg TODO crates src examples`).
The chat example currently supplies persistent post/poll and unique message IDs;
acknowledgements and leased claims are follow-up work.

Maintenance is explicit through `State::store().maintenance(retain_receipts)`:
it removes unused history/snapshots and retains the latest N receipts globally.
The CLI also supports `statemcp maintenance PATH --retain-receipts 10000`
and prints a JSON report. Expired receipt keys can execute again. Managed state
files should only be modified through the service.

<!-- TODO(audit): review aggregate parent SQLite memory, per-worker versus whole-root budgets, platform durability, authorization, and unpaginated discovery before expanding deployment scope. -->
<!-- TODO(performance): add reflink acceleration and measure first-write cost on deployment filesystems. -->
<!-- TODO(chat): add monotonic acknowledgements, leased claims/fencing, and bounded request validation; the current example only implements persistent post/poll and unique message IDs. -->
<!-- TODO(setup): atomic fixture installation and versioned upgrades for existing namespaces. -->

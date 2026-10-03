# State MCP

A Rust library and MCP stdio server for agent-defined stateful APIs. Each namespace
contains virtual files, named SQLite databases, and published Python functions
executed by the embedded Pydantic Monty Rust runtime. Python installation is only
needed for the optional demo client.

```sh
cargo build
cargo run -- --data-dir .state-mcp
```

For an MCP client, configure `target/debug/state-mcp` as its command and
`["--data-dir", "/absolute/path/to/state"]` as arguments. The server speaks
newline-delimited JSON-RPC, MCP `2025-06-18`; stdout contains protocol messages.
Initialize the connection, send `notifications/initialized`, then use
`tools/list` and `tools/call`. Errors have `isError: true` and a structured
`{error: {code, message}}` result.

Run the notes, todos, and two-client chat demo:

```sh
cargo build
python3 examples/demo.py --binary target/debug/state-mcp --data-dir .state-mcp-demo
cargo test --workspace --all-targets
```

The demo publishes [these fixtures](examples/setup.json), adds a note, completes a
todo, and posts a message that another server process polls from the same state.
Run it again to reuse persisted data. The demo setup expects an empty directory or
its own previously installed fixtures; it currently skips existing namespaces.

## Fixed tools

| Tool | Purpose |
| --- | --- |
| `state_namespace` | Create, list, get, rename (`update`), copy, delete namespaces |
| `state_fs` | Read/write/append/stat/list/delete/copy/move virtual files |
| `state_db` | Create/list/drop named databases; query, execute, migrate, inspect, migration history |
| `state_function` | Declare/update/get/list/remove published endpoints |
| `state_call` | Call `{namespace, function, arguments?, expected_version?, idempotency_key?}` |
| `state_execute` | Run `{script, namespace?, inputs?, idempotency_key?}` |
| `state_describe` | Discover tool and endpoint contracts |

`tools/list` exposes action-specific argument schemas. SQL accepts bound `params`;
query/execute results are `{columns, rows, rows_affected}`. Query/execute use one
statement; migrations take `[{id, sql}]`. Schema inspection includes tables,
columns, indexes, foreign keys, and ordinary SQLite schema information. Each
named application database is a separate SQLite file; private catalog metadata
is inaccessible through application SQL.

A declaration pins a virtual source file and callable symbol:

```json
{
  "action": "declare", "namespace": "notes", "name": "add",
  "file": "/api.py", "symbol": "add", "databases": {"app": "write"},
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
MCP wraps non-object values as `{value: ...}` in `structuredContent`. Monty
provides `mcp(name, arguments={})`, `call(namespace, function, arguments={})`,
`db_query`, `db_execute`, `db_inspect`, `read_text`, and `write_text`. The database
and file helpers use the script's selected namespace or endpoint's namespace.
There is no package loader or virtual-file import resolver in this MVP.

A root script can compose all seven tools. Published endpoint bodies use explicit
grants: `databases:{name:"read"|"write"|"migrate"}`, path-prefix
`files:{"/path":"read"|"write"}`, and `calls:[{namespace,function}]`.
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
use state_mcp::State;

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
The CLI also supports `state-mcp --data-dir PATH --maintenance --retain-receipts 10000`
and prints a JSON report. Expired receipt keys can execute again. Managed state
files should only be modified through the service.

<!-- TODO(audit): review aggregate parent SQLite memory, per-worker versus whole-root budgets, platform durability, authorization, and unpaginated discovery before expanding deployment scope. -->
<!-- TODO(performance): add reflink acceleration and measure first-write cost on deployment filesystems. -->
<!-- TODO(chat): add monotonic acknowledgements, leased claims/fencing, and bounded request validation; the current example only implements persistent post/poll and unique message IDs. -->
<!-- TODO(setup): atomic fixture installation and versioned upgrades for existing namespaces. -->

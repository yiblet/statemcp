# state-core

The library service behind statemcp's seven fixed tools. It composes immutable
storage and Monty through one dispatcher, with the same validation and permission
checks for direct calls and Python host calls. No Python installation is needed.

```rust,no_run
use serde_json::json;
use state_core::State;
let state = State::open("./state")?; // embedded Monty
state.dispatch("state_namespace", json!({"action":"create", "name":"notes"}))?;
let result = state.dispatch("state_execute", json!({"script":"inputs + 1", "inputs":41}))?;
assert_eq!(result["value"], 42);
# Ok::<(), state_core::Error>(())
```

`State::with_backend(Store, Arc<dyn RuntimeBackend>)` selects execution placement.
`RuntimeBackend` exposes shared-reference `execute`, `invoke`, and
`validate_module` methods with the argument types from `state-runtime`.
`EmbeddedBackend` runs Monty in process. `state_runtime::WorkerConfig` implements
the same trait for same-executable workers; the executable must install the
runtime allocator and worker entrypoint as described in `state-runtime`.
`State::with_limits(CoreLimits)` changes root/runtime budgets.

`dispatch(tool, arguments)` uses the trusted `owner` identity. `dispatch_as`
accepts an embedding-provided identity **only for receipt scoping**; it does not
authenticate users or establish tenant permissions. Every root operation has
local owner access. This v1 API is intended for trusted local builders; only
published function bodies execute with access restricted to their own namespace. The `Store`
accessor is similarly an owner API, not a restricted interface for untrusted code.

## Fixed operations and results

`tool_definitions()` returns the canonical MCP discovery definitions and strict
JSON schemas that the service itself enforces. Unknown fields, actions, malformed
argument types, and unknown host keyword arguments are errors. Storage operation
shapes are documented in `state-store/README.md` and exposed through
`state_describe`. The core supports all storage actions, including `state_fs`
`stat`/`copy`, and `state_db` `drop`/`migrations`.

`state_call` takes `{namespace,function,arguments?,expected_version?,idempotency_key?}`
and returns the endpoint's JSON value directly. `arguments` defaults to `{}`.
`state_execute` takes `{script,inputs?,namespace?,idempotency_key?}` and returns
`{value,stdout}`. `inputs` defaults to null, and `namespace` selects the default
namespace for helpers. Top-level scripts have owner access and may call all seven
tools; scripts nested through `mcp` retain that authority and share the root budget.
Scripts nested inside functions inherit the function's namespace scope.

There is no revision envelope around composable results. Root namespace
create/copy/update results contain the actual committed revision; corresponding
nested results omit a revision because their changes are still staged. Use
`state_namespace get` in a subsequent root call to inspect a committed head.
Nested SQL/file operations always operate on the root's private staged state.
Python print output from an endpoint is bounded and discarded; script prints are
returned in that script's `stdout` (nested script output is returned by its own
`state_execute` call, not spliced into the outer print stream).

## Publication and namespace scope

A declaration takes `action: declare|update`, `namespace`, `name`, `file`, and
`symbol`. Optional fields are `description`, `input_schema`, `output_schema`,
`expected_version`, and `expected_revision`.

```json
{
  "action": "declare",
  "namespace": "notes",
  "name": "add",
  "file": "/api.py",
  "symbol": "add",
  "input_schema": {
    "type": "object",
    "properties": {
      "text": {
        "type": "string"
      }
    },
    "required": [
      "text"
    ],
    "additionalProperties": false
  },
  "output_schema": {
    "type": "integer"
  }
}
```

Declarations compile and initialize the module with all host effects denied,
check the callable, and pin the entry source, namespace Python files, and contracts. Initialization is
also effect-free on every invocation; no VM globals persist between calls.
Editing the entry or imported helper files does not change the published endpoint;
update its declaration to publish the current files. Imports have separate module
globals and a fresh per-invocation cache.
`expected_version` checks stale deployment or invocation versions. Input schemas
(default `{"type":"object"}`) run before invocation; output schemas (default
`true`) run before commit. A contract violation produces `SCHEMA_VALIDATION`
and rolls back every nested change.

Schema compilation disables the JSON Schema library's file/network resolver
features and explicitly denies retrieval even if another dependency enables them. External `$ref`, `$dynamicRef`, and `$recursiveRef` values and `$id`
resources are rejected. Local fragment references, including `$defs`, are
supported. Schemas are capped at 64 KiB and 32 schema nesting levels. No implicit
schema-related file or network IO is available.

A function has full access to every file, database, and function in its own
namespace. It can manage databases and publish or update functions there.
`self` follows namespace renames and copies. Explicit selectors are checked
against the same stable namespace identity. Cross-namespace state access and
calls are rejected. Nested scripts retain this scope; namespace creation,
copying, and global listing require root owner access.

Discovery defaults to a compact overview. `describe({"mode":"runtime"})`
returns host signatures, result shapes, namespace scope, ABI policy, and a runnable
endpoint example. `mode:"full"` returns full schemas; `tool:"db.create"` focuses discovery on one operation.

Declaration/update responses contain `name`, `published`, and `version`;
explicit `state_function get` retains source and diagnostic metadata. Database
creation omits UUID/snapshot metadata, which remains available through inspect.
Direct mutations include their committed namespace revision when one is created.

## Monty helpers

```python
mcp(name, arguments={})               # all fixed tool operations
call(namespace, function, arguments={})
db_query(database, sql, params=[])
db_execute(database, sql, params=[])
db_inspect(database)
read_text(path)
write_text(path, text)
```

Arguments can be positional or use these keyword names. Helpers use the default
namespace of the script or the owning namespace of the endpoint. `call("self",
...)` and `mcp` namespace `self` resolve locally inside endpoint bodies. Use
`mcp("state_db", {...})` for migrations, or to select an explicit namespace in an
owner script. Database values remain bound parameters, never interpolated code.
Imports resolve Python files and packages inside the selected namespace.
There is no third-party package installer.

## Atomicity, limits, and receipts

One root dispatch owns one storage transaction. Every nested call and script
shares it. Any operation, host callback, schema validation, or runtime failure
poisons the root: even a custom runtime backend that swallows a callback error
cannot publish partial changes. Files, multiple databases/namespaces, endpoint
code, migrations, and the optional receipt publish together. This retains the
storage layer's conflict detection and durability guarantees.

Defaults are a five-second root duration, 256 total tool operations (including
the root), 16 simultaneously executing scripts/endpoints, 1 MiB runtime JSON,
and 16 MiB direct service results. Each nested VM receives only the remaining
root wall time. Python recursion, allocation, source, and print budgets also use
`state-runtime::Limits`. Diagnostic byte accounting is shared across script and
endpoint invocations. SQL/schema callbacks are synchronous: deadlines are checked
before and after them, and expiration prevents publication; a host callback itself
cannot be preempted by the cooperative embedded service.

The isolated backend's allocator ceiling applies **per worker**, with at most
`max_depth` active execution workers plus one module-validation worker. It is not a root-wide shared allocator/RSS
ceiling. Embedded mode does not promise an aggregate memory cap; use workers
for memory/crash isolation. Worker isolation also does not constitute an OS
sandbox for native code.

Direct file IO supports storage's 8 MiB files, while Monty's JSON bridge is 1 MiB.
Large files can be read directly but cannot be passed whole through that bridge.
After write/append the service checks the serialized read result, including
escaping/base64 overhead, against the same boundary used for the write, and
rejects oversized output before committing. Runtime creation of an unreadable
file therefore cannot silently succeed. Lists are bounded but unpaginated.
History, old snapshots, and receipts remain until explicit storage maintenance.
This service does not run automatic GC or receipt expiration.

Only root `state_call`/`state_execute` accept `idempotency_key` (1..256 bytes).
Nested keys are rejected. Receipts are scoped by trusted principal and a
canonical hash of tool/arguments; identical retries return the exact committed
result, changed requests return `IDEMPOTENCY_MISMATCH`. Concurrent duplicates
return the winning receipt and publish once. Failed runs leave no receipt.

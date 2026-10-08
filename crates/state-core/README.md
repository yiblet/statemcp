# state-core

Application controllers and typed feature services behind StateMCP’s 34 tools.
Controllers coordinate root transactions, result validation and receipts.
Namespaces, Files, Databases, Functions, Execution and DiscoveryService receive
contract types and an explicitly borrowed Context. No Python installation is needed.

```rust,no_run
use state_core::Controller;
use state_contract::namespace::NamespaceCreateInput;
use state_contract::namespace::DescriptionChange;
let state = Controller::open("./state")?;
let namespace = state.create_namespace(NamespaceCreateInput {
    name: "notes".into(), description: DescriptionChange::Set("Shared notes".into()),
})?;
assert!(namespace.revision.is_some());
# Ok::<(), state_core::Error>(())
```

`Controller::with_backend(Store, Arc<dyn RuntimeBackend>)` selects execution placement.
`RuntimeBackend` exposes shared-reference `execute`, `invoke`, and
`validate_module` methods with the argument types from `state-runtime`.
`EmbeddedBackend` runs Monty in process. `state_runtime::WorkerConfig` implements
the same trait for same-executable workers; the executable must install the
runtime allocator and worker entrypoint as described in `state-runtime`.
`Controller::with_limits(CoreLimits)` changes root/runtime budgets.

`Controller::with_principal(identity)` scopes receipts to an embedding-provided identity.
It does not authenticate users or establish tenant permissions. Root operations
have owner access. Published function bodies are restricted to their own namespace.

## Services and execution context

`Controller::new(store, runtime)` constructs the application from explicit storage
and interpreter dependencies. `Runtime` is a shared handle to an injected
`RuntimeBackend`; controller/service clones retain the same backend. Primitive
services are concrete types, not generic request dispatchers or service locators.

`Context` owns one transaction, established namespace scope, deadline, counters,
and sticky failure. It contains no services or runtime handles. Only a controller
can create or finish it. Typed custom use cases can compose services atomically:

```rust,no_run
use state_core::Controller;
use state_contract::file::{FsWriteInput, FileTextInput};
# let controller = Controller::open("./state")?;
let (written, commit) = controller.transaction(|context| {
    controller.files().write_file(FsWriteInput::Text(FileTextInput {
        namespace: "notes".into(), path: "/note.txt".into(), text: "hello".into(),
        expected_revision: None,
    }), context)
})?;
# Ok::<(), state_core::Error>(())
```

Each primitive enforces scope and charges one operation before accessing domain
storage. Failures poison Context even when a caller catches the error. Execution
narrows function scope and supplies a per-run host adapter. That adapter invokes
the same services with the same Context, never starts a nested root transaction,
and depends on the neutral `CodeExecutor` interface rather than concrete Execution.

Heavy boundaries can be injected independently: `Sources` accepts a
`SourceProvider`, Runtime accepts a RuntimeBackend, and storage accepts its
filesystem implementation. Functions uses Sources and Runtime; neither neutral
port imports its consuming service. Publication carries the exact validated
`FunctionSources` into domain persistence, which checks source/module hashes and
snapshot membership before publishing. Tests can inject source/runtime failures
while using actual in-memory storage; no mock trait is needed for every primitive.
Dependency direction is a design constraint: boundary contracts may depend on
shared values and Context, but never on their consuming service or concrete
implementation. Implementations satisfy those contracts; the composition root
selects them. Add a narrow injectable boundary when execution is expensive,
external, or nondeterministic, rather than making every service a trait. Keep
pure rules directly callable and test real transaction/SQL guarantees against
actual storage alongside tests with controlled boundary failures.

Public operation contracts live in the capability modules of `state-contract`.
Persistent manifests, receipts and declaration hashes keep their existing format;
there are no storage migrations or code compatibility facades.

Async transports use state-http’s `BlockingExecutor`, supplied explicitly to Server.
Its clones share bounded admission, acquired before scheduling blocking work and
held until that work finishes, including when its async caller is canceled. SQL,
source reads, interpreter execution and commit remain within the blocking task.
Synchronous embedding calls controllers directly; storage still enforces exclusive
root ownership and concurrent roots fail with Conflict.

## Fixed operations and results

`tool_definitions()` returns the canonical MCP discovery definitions and strict
JSON schemas that the service itself enforces. Unknown fields, actions, malformed
argument types, and unknown host keyword arguments are errors. Storage operation
shapes are documented in `state-domain/README.md` and exposed through
`describe`. The core supports all storage actions, including `fs`
`stat`/`copy`, and `db.query` `drop`/`migrations`.

`call` takes `{namespace,function,arguments?,expected_version?,idempotency_key?}`
and returns the endpoint's JSON value directly. `arguments` defaults to `{}`.
`execute` takes `{script,inputs?,namespace?,idempotency_key?}` and returns
`{value,stdout}`. `inputs` defaults to null, and `namespace` selects the default
namespace for helpers. Top-level scripts have owner access and may call all 34
tools; scripts nested through `mcp` retain that authority and share the root budget.
Scripts nested inside functions inherit the function's namespace scope.

There is no revision envelope around composable results. Root namespace
create/copy/update results contain the actual committed revision; corresponding
nested results omit a revision because their changes are still staged. Use
`namespace.get` in a subsequent root call to inspect a committed head.
Nested SQL/file operations always operate on the root's private staged state.
Python print output from an endpoint is bounded and discarded; script prints are
returned in that script's `stdout` (nested script output is returned by its own
`execute` call, not spliced into the outer print stream).

## Publication and namespace scope

`function.declare` and `function.update` take `namespace`, `name`, `file`, and
`symbol`. Optional fields are `description`, `input_schema`, `output_schema`,
`expected_version`, and `expected_revision`.

```json
{
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
explicit `function.get` retains source and diagnostic metadata. Database
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
`mcp("db.query", {...})` for migrations, or to select an explicit namespace in an
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
Historical domain metadata and receipts remain until explicit maintenance. Resource database deletion publishes with the invocation.
This service does not run automatic GC or receipt expiration.

Only root `call`/`execute` accept `idempotency_key` (1..256 bytes).
Nested keys are rejected. Receipts are scoped by trusted principal and a
canonical hash of tool/arguments; identical retries return the exact committed
result, changed requests return `IDEMPOTENCY_MISMATCH`. Concurrent invocations return `CONFLICT` while another invocation holds the root;
retrying then returns the committed receipt and publishes once. Failed runs leave no receipt.

## Tests

Tests live in crate-local `#[cfg(test)]` modules under `src/`. They use
in-memory storage and embedded execution where applicable; they do not create
host files, bind sockets, or launch processes.

```sh
cargo test -p state-core
```

## HTTP services

`Api` publishes pinned HTTP handlers or live static mounts, with Sources and
Runtime injected explicitly. `HttpApplication` resolves typed routes and calls
Execution/Files in a borrowed Context. `Controller::handle_http` validates raw
response bytes/metadata and commits before transport writing; it does not JSON
encode the response internally. HTTP callbacks use the same namespace scope and
primitive services as ordinary functions. No per-route permissions are added.
The state-http crate owns async body collection, shared BlockingExecutor admission,
URL reporting and Axum response writing. Core/domain/runtime never import it.

# State MCP

A Rust library and MCP stdio binary for agent-defined stateful APIs. The fixed
surface contains namespace management, virtual files, named SQLite databases,
declared Monty functions, endpoint calls, scripts, and discovery.

```sh
cargo run -- --data-dir .state-mcp
cargo test --all-targets
```

The protocol adapter supports MCP `2025-06-18`, newline-delimited JSON-RPC,
`initialize`, `notifications/initialized`, `ping`, `tools/list`, and `tools/call`.
The fixed tool list does not send change notifications. Each input frame is
limited to 8 MiB. Requests execute synchronously; closing stdin stops the server.
This implementation does not advertise cancellation, streaming, HTTP, resources,
or prompts. Protocol messages exclusively occupy stdout during service operation.

Embed `Server::new(dispatcher)` with any implementation of `Dispatcher`, including
a closure `FnMut(&str, serde_json::Value) -> Result<Value, ToolError>`. The service
receives raw tool arguments and owns action-specific validation. Application
errors produce MCP `isError: true` with a stable `{error: {code, message, details?}}`
object. Invalid JSON-RPC envelopes and unknown tools produce JSON-RPC errors.

The initial transport scaffold uses `UnsupportedDispatcher`. Storage and Monty
integration are separate implementation tickets; `--data-dir` is reserved for
that integration and the scaffold does not create files.

Tool argument contracts are defined in `src/protocol.rs::tool_definitions`.
`state_call` uses `{namespace, function, arguments?}`; `state_execute` uses
`{script, namespace?, inputs?}`. Both reserve `idempotency_key`. Namespace rename
is `state_namespace {action:"update", namespace, name}` and copy uses the same
shape with `action:"copy"`. File moves use `{action:"move", namespace, path,
destination}`. Endpoint declarations use `{action:"declare", namespace, name,
file, symbol, input_schema?, output_schema?, databases?}`. Database SQL uses
`{action:"query"|"execute", namespace, database, sql, params?}`; migrations use
`{action:"migrate", namespace, database, migrations:[{id,sql}]}`.

Protocol references: [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)
and [MCP tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools).

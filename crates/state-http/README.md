# state-http

Axum hosting for namespace HTTP APIs and static websites, plus shared admission
into synchronous StateMCP controllers. Namespace API and static routes are public;
the executable applies bearer authentication to MCP only and checks Host and Origin
on all routes. This crate does not create its own storage/runtime dependencies.

`BlockingExecutor` clones share admission and retain a running worker's slot after
caller cancellation. `namespace_router(executor, prefix)` collects bounded request
bodies asynchronously, then runs the complete use case on a blocking thread.
`Hosting` reports URL patterns using actual listener addresses or an origin
established by the executable's host policy. Forwarded headers are not trusted.

HTTP execution uses namespace-wide authority: handlers can access every primitive
and call every function in their namespace. Cross-namespace operations are denied.
Response normalization and validation precede transaction publication. Valid
application 4xx/5xx results commit; runtime/host/invalid-response errors roll back.
HEAD suppresses bytes after execution/commit. Static mounts read live virtual files.

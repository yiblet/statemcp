# state-runtime

A Rust library embedding the official Pydantic Monty interpreter, pinned to
`monty = 1.0.0`, `monty-types = 1.0.0`, and `monty-alloc = 1.0.0` (Rust 1.96+).
No Python interpreter, subprocess Python runner, filesystem mount, or network
adapter is used.

The public `execute(source, inputs, limits, host)` binds JSON as the `inputs`
global and returns `RunResult { value, stdout }`. `execute_with_bindings` accepts
an object of explicit global names. `invoke(source, symbol, arguments, limits,
host)` first initializes a fresh module with all host calls denied, then invokes
the selected callable with object fields as keyword arguments.
`validate_module(source, symbol, limits)` validates initialization and callable
existence without invoking it. Module state is discarded after every operation.

`HostCallback` is a synchronous `FnMut(&str, Vec<Value>, Map<String, Value>) ->
Result<Value, RuntimeError>`. Its seven names are `mcp`, `call`, `db_query`,
`db_execute`, `db_inspect`, `read_text`, and `write_text`. Monty suspends at each
call; the host dispatches it and resumes with JSON. Host errors end the Rust run
immediately; Python cannot catch them and continue making effects. The caller
must share its transaction and overall depth/call/time budgets across recursive
runtime invocations. This crate limits one invocation, not the complete tree.

JSON supports null, booleans, signed 64-bit integers, finite floats, strings,
arrays, and string-keyed objects. Returned tuples become arrays. Sets, classes,
bytes, non-finite numbers, larger integers, and cyclic outputs are rejected;
encode such values explicitly as ordinary JSON before crossing the boundary.
JSON depth and byte budgets also apply to callback arguments/results. Print
output is captured with a byte cap; `stdout` contains both stdout and stderr in
emission order. No diagnostics are emitted to the server's protocol stdout.

Python support is exactly Monty's supported language and standard-library
subset. Arbitrary package imports, OS/file operations requiring a host handler,
unresolved host futures, and unknown host names are rejected. Monty's built-in
clock and random behavior remains available; host sleeps are denied at OS
suspension. There is no import resolver for virtual Python files in v1.

## Resource enforcement and worker integration

Execution duration, callback count, Python recursion, source size, print size,
JSON size/depth, and individual VM allocation preflight checks are bounded.
The duration check is cooperative in-process: a synchronous host callback must
implement its own deadline; the runtime checks again when it returns. Parser,
VM bugs, native-stack exhaustion, and allocator aborts are not crash-isolated.
A separate worker and parent-side wall deadline are required for hostile code.

`Limits::max_memory` defaults to `None`: embedded execution does **not** claim an
aggregate memory cap. `max_allocation_bytes` defaults to 64 MiB and constrains
Monty's allocation preflight checks, not the sum of small allocations. Requesting
`max_memory: Some(bytes)` without active allocator tracking returns
`UNSUPPORTED_FEATURE` before executing code.

For a dedicated worker executable, install the exported `LimitedAllocator` as
`#[global_allocator]` and call `arm_worker_memory_limit(bytes)` once on a fresh
worker, before decoding execution inputs. Then pass `max_memory: Some(bytes)`.
The allocator accounts cumulative process allocations relative to the initial
worker baseline. The hard ceiling includes Monty's 4 MiB exception headroom;
exceeding it exits with code 65. These settings are **process global**: do not arm
them per session in a concurrent/shared parent server. Prefer one worker per root
invocation, let its parent enforce a deadline and interpret exit 65 as
`LIMIT_EXCEEDED`, and do not attempt further work after a failed run. The embedded APIs do not launch a worker or install a global allocator.

Verified against the release Rust source and official documentation:
- https://pydantic.dev/docs/monty/quickstart/rust/
- https://docs.rs/monty/1.0.0/monty/
- https://docs.rs/monty-types/1.0.0/monty_types/struct.ResourceTracker.html
- https://docs.rs/monty-alloc/1.0.0/monty_alloc/

Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test --all-targets` from this crate (or use `--manifest-path`). The tests
exercise actual Monty execution, callback JSON fidelity, pinned-source function
invocation, effect-free initialization, uncatchable host errors, budgets,
filesystem denial, fresh scopes, JSON rejection, and an isolated cumulative
allocator limit probe.


## Same-executable isolated execution

`WorkerConfig::new(executable)` supplies `execute`, `invoke`, and
`validate_module` with the same arguments as the embedded functions. Equivalent
free functions `execute_isolated`, `invoke_isolated`, and
`validate_module_isolated` take `&WorkerConfig` first. The executable must install
`LimitedAllocator` globally and route `--worker MEMORY_BYTES MAX_FRAME_BYTES` to
`worker_main`. The State MCP binary already does this; its parent server never
arms the memory ceiling or sets a baseline. Library users choose the executable
explicitly instead of accidentally launching their own test harness or application.

Each operation creates a fresh child, so recursive host invocations can launch
another child without a worker pool deadlock. The default aggregate allocator
budget is 64 MiB plus Monty's 4 MiB exception headroom;
`Limits::max_memory` overrides the default and `WorkerConfig::default_memory_bytes`
changes it. This measures live allocator bytes, not operating-system RSS or all
native stack pages. Monty recursion limits still apply, and a native stack crash
is confined to the child.

Requests, host calls, host results, completion, and errors use a four-byte
big-endian length prefix followed by JSON. Both serialized output and incoming
frame allocation are bounded: the default frame cap is 8 MiB, configurable from
1 KiB through 64 MiB. Worker stdout carries only frames; Python prints remain in
`RunResult::stdout`. The child receives no state directory, database handles, or
filesystem/network adapters; all host callbacks run in the parent. This is crash
and resource isolation, not an operating-system security sandbox for native code.

An independent parent watchdog kills and reaps the child at the wall deadline,
even while a host callback is running. Cleanup also kills/reaps on protocol errors,
callback errors, and Rust panic unwinding. Callback time counts against the same
deadline. A synchronous borrowed Rust callback cannot itself be preempted: its
owner must impose deadlines on blocking operations; dispatch returns the timeout
when that callback returns. Each nested worker gets the limits supplied by its
caller; the State service must carry root budgets across nested operations.

The actual-binary integration suite covers execute/invoke/validation, recursive
workers, callback failures and panics, time/memory/recursion/output recovery,
malformed/oversized frames, and operation with an empty PATH (no Python needed).
Unit tests additionally verify an uncooperative child is killed and reaped while
the parent is blocked and that dropping its guard reaps immediately.

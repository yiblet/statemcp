# StateMCP's Monty import extension

Based on the published `monty` 1.0.0 crate from
[github.com/pydantic/monty](https://github.com/pydantic/monty).
The upstream source is MIT licensed; see LICENSE. Cargo patches only the
interpreter crate; monty-types and monty-alloc remain registry dependencies.

Upstream supports built-in imports only. This extension keeps imports in the
native bytecode VM instead of rewriting or concatenating Python source files.
Source modules use existing dict-backed exec scopes, so functions retain their
module globals. Import frames share the interpreter's time, recursion, memory,
and host-call limits and can suspend/resume through the worker protocol.

- `builtins/source_import.rs`: namespace source lookup, package initialization,
  module caching, namespace packages, wildcard exports, and failed-import cleanup.
- `builtins/eval_exec.rs`: compile imported source in a separate module dictionary.
- `bytecode/compiler.rs`, `parse.rs`: dotted, relative, and wildcard import syntax.
- `bytecode/vm/{mod,attr}.rs`: load source modules and resolve package submodules.
- `types/module.rs`, `heap_data.rs`, `heap/mod.rs`: native module objects backed
  by shared globals, mutable attributes, and reference-count/GC traversal.

The host supplies immutable source files, an entry directory, and a fresh module
cache per operation. StateMCP snapshots namespace Python files at publication and
passes them explicitly through both embedded and worker backends. No host import
paths or installed packages are exposed. Import behavior is covered in
`crates/state-core/tests/service.rs` and `tests/worker.rs`.

The crate manifest omits upstream integration tests and dev dependencies; this
copy is a dependency, not a workspace member. To update it, compare these files
with a new upstream release and preserve or upstream the import extension.

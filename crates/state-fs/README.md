# state-fs

A blocking filesystem boundary for the SQLite store and Rust migrations.
`FileSystem` supplies root preparation, file operations, metadata, directory
sync, and nonblocking shared/exclusive locks. `File` supplies reading, writing,
seeking, truncation, and sync. Lock guards release ownership on drop.

`PhysicalFs` uses the host filesystem and OS locks. Run it on dedicated threads
or through `spawn_blocking` when used from async code.

`MemoryFs` implements the same contracts entirely in memory. Clones share files
and locks; separate instances are isolated. Paths are rooted at `/` and reject
parent traversal. Existing handles survive rename and unlink. File sync and
directory sync are no-ops: memory fixtures model publication and recovery, not
physical durability or power loss.

Inject `MemoryFs` with `state_store::Store::builder().with_fs(fs)` for tests.
The crate-local `#[cfg(test)]` modules cover handle lifetime, truncation,
permissions, atomic creation, and lock contention without external I/O. Physical
file operations, symlink metadata, and cross-process locking are exercised by
the workspace [end-to-end suite](../../tests/e2e/README.md).

```sh
cargo test -p state-fs
```

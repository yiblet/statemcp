//! Filesystem durability shared by catalog initialization and snapshot publication.
use crate::Result;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

/// Persist new directory entries all the way to the closest existing ancestor.
pub(crate) fn create_root(path: &Path) -> Result<PathBuf> {
    let mut existing = path.to_path_buf();
    while !existing.try_exists()? {
        existing.pop();
        if existing.as_os_str().is_empty() {
            existing = PathBuf::from(".");
            break;
        }
    }
    let existing = fs::canonicalize(existing)?;
    fs::create_dir_all(path)?;
    let root = fs::canonicalize(path)?;
    let mut directory = root.as_path();
    loop {
        sync_dir(directory)?;
        if directory == existing {
            break;
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        directory = parent;
    }
    Ok(root)
}

pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

/// Every invocation opens its own handle: locks must also coordinate threads and
/// independently opened Store handles in the same process. Never unlink this file.
pub(crate) fn root_lock(root: &Path, exclusive: bool) -> Result<File> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("maintenance.lock"))?;
    let result = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    match result {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(crate::Error::new(
            crate::ErrorCode::Conflict,
            "storage maintenance conflicts with an active invocation; retry later",
        )),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

// Test builds only: production never interprets process environment as failpoints.
#[cfg(test)]
pub(crate) fn publication_failpoint(point: &str) {
    if std::env::var("STATE_STORE_PUBLICATION_FAILPOINT").as_deref() == Ok(point) {
        std::process::exit(79);
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;
    use crate::test_support::ToolCalls;
    use serde_json::{Value, json};

    fn run(store: &Store, tool: &str, args: Value) -> Value {
        let mut tx = store.begin().unwrap();
        let result = tx.tool(tool, args).unwrap();
        tx.commit_result().unwrap();
        result
    }

    #[test]
    fn publication_crash_helper() {
        let Ok(root) = std::env::var("STATE_STORE_PUBLICATION_ROOT") else {
            return;
        };
        let store = Store::open(root).unwrap();
        let mut tx = store.begin().unwrap();
        for ns in ["a", "b"] {
            tx.tool(
                "db.execute",
                json!({"namespace":ns,"database":"app","sql":"INSERT INTO n VALUES(1)"}),
            )
            .unwrap();
            tx.tool(
                "fs.write",
                json!({"namespace":ns,"path":"/note","text":"new"}),
            )
            .unwrap();
        }
        tx.set_receipt(
            "owner",
            "key",
            "request",
            crate::records::ReceiptResult::Function(json!("done")),
        )
        .unwrap();
        tx.commit_result().unwrap();
        panic!("failpoint did not exit");
    }

    #[test]
    fn publication_crash_boundaries_recover_atomic_heads_and_receipt() {
        for point in [
            "after_snapshot_seal",
            "before_catalog_commit",
            "after_catalog_commit",
        ] {
            let dir = tempfile::TempDir::new().unwrap();
            let store = Store::open(dir.path()).unwrap();
            for ns in ["a", "b"] {
                run(&store, "namespace.create", json!({"name":ns}));
                run(
                    &store,
                    "db.create",
                    json!({"namespace":ns,"database":"app"}),
                );
                run(
                    &store,
                    "db.execute",
                    json!({"namespace":ns,"database":"app","sql":"CREATE TABLE n(x)"}),
                );
                run(
                    &store,
                    "fs.write",
                    json!({"namespace":ns,"path":"/note","text":"old"}),
                );
            }
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "durability::tests::publication_crash_helper",
                    "--nocapture",
                ])
                .env("STATE_STORE_PUBLICATION_ROOT", dir.path())
                .env("STATE_STORE_PUBLICATION_FAILPOINT", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(79), "{point}");
            let reopened = Store::open(dir.path()).unwrap();
            let committed = point == "after_catalog_commit";
            // GC must be safe even immediately after recovery, before any reads.
            reopened.maintenance(100).unwrap();
            for ns in ["a", "b"] {
                assert_eq!(
                    run(
                        &reopened,
                        "db.query",
                        json!({"namespace":ns,"database":"app","sql":"SELECT x FROM n"})
                    )["rows"],
                    if committed { json!([[1]]) } else { json!([]) }
                );
                assert_eq!(
                    run(&reopened, "fs.read", json!({"namespace":ns,"path":"/note"}))["text"],
                    if committed { "new" } else { "old" }
                );
            }
            assert_eq!(
                reopened.receipt("owner", "key").unwrap().is_some(),
                committed
            );
        }
    }
}

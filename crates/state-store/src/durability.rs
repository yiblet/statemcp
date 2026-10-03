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
            "CONFLICT",
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
    use serde_json::{Value, json};

    fn run(store: &Store, tool: &str, args: Value) -> Value {
        let mut tx = store.begin().unwrap();
        let result = tx.dispatch(tool, args).unwrap();
        tx.commit().unwrap();
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
            tx.dispatch("state_db", json!({"action":"execute","namespace":ns,"database":"app","sql":"INSERT INTO n VALUES(1)"})).unwrap();
            tx.dispatch(
                "state_fs",
                json!({"action":"write","namespace":ns,"path":"/note","text":"new"}),
            )
            .unwrap();
        }
        tx.set_receipt("owner", "key", "request", json!("done"))
            .unwrap();
        tx.commit().unwrap();
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
                run(
                    &store,
                    "state_namespace",
                    json!({"action":"create","name":ns}),
                );
                run(
                    &store,
                    "state_db",
                    json!({"action":"create","namespace":ns,"database":"app"}),
                );
                run(
                    &store,
                    "state_db",
                    json!({"action":"execute","namespace":ns,"database":"app","sql":"CREATE TABLE n(x)"}),
                );
                run(
                    &store,
                    "state_fs",
                    json!({"action":"write","namespace":ns,"path":"/note","text":"old"}),
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
                        "state_db",
                        json!({"action":"query","namespace":ns,"database":"app","sql":"SELECT x FROM n"})
                    )["rows"],
                    if committed { json!([[1]]) } else { json!([]) }
                );
                assert_eq!(
                    run(
                        &reopened,
                        "state_fs",
                        json!({"action":"read","namespace":ns,"path":"/note"})
                    )["text"],
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

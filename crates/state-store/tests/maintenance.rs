mod support;
use serde_json::{Value, json};
use state_store::Store;
use std::{
    fs,
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use support::ToolCalls;
use tempfile::TempDir;

fn run(store: &Store, tool: &str, args: Value) -> Value {
    let mut tx = store.begin().unwrap();
    let value = tx.tool(tool, args).unwrap();
    tx.commit_result().unwrap();
    value
}
fn sql(store: &Store, action: &str, source: &str) -> Value {
    run(
        store,
        &format!("db.{action}"),
        json!({"namespace":"a","database":"app","sql":source}),
    )
}
fn setup() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    run(&store, "namespace.create", json!({"name":"a"}));
    run(
        &store,
        "db.create",
        json!({"namespace":"a","database":"app"}),
    );
    sql(&store, "execute", "CREATE TABLE n(x)");
    sql(&store, "execute", "INSERT INTO n VALUES(1)");
    (dir, store)
}

#[test]
fn maintenance_prunes_history_orphans_and_receipts_preserving_forks_and_pins() {
    let (dir, store) = setup();
    run(
        &store,
        "fs.write",
        json!({"namespace":"a","path":"/api.py","text":"def f(): return 1"}),
    );
    run(
        &store,
        "function.declare",
        json!({"namespace":"a","name":"f","file":"/api.py","symbol":"f"}),
    );
    run(
        &store,
        "fs.write",
        json!({"namespace":"a","path":"/api.py","text":"def f(): return 2"}),
    );
    run(
        &store,
        "namespace.copy",
        json!({"namespace":"a","name":"b"}),
    );
    sql(&store, "execute", "UPDATE n SET x=2");
    run(&store, "namespace.delete", json!({"namespace":"a"}));
    run(&store, "namespace.create", json!({"name":"deleted"}));
    run(
        &store,
        "fs.write",
        json!({"namespace":"deleted","path":"/orphan","text":"unreferenced payload"}),
    );
    run(&store, "namespace.delete", json!({"namespace":"deleted"}));
    for key in ["first", "second", "third"] {
        let mut tx = store.begin().unwrap();
        tx.set_receipt(
            "owner",
            key,
            key,
            state_store::records::ReceiptResult::Function(json!(key)),
        )
        .unwrap();
        tx.commit_result().unwrap();
    }
    // Orphans and abandoned staging are reclaimed even after restart.
    let orphan = dir
        .path()
        .join("snapshots")
        .join(format!("{}.sqlite", uuid::Uuid::new_v4()));
    fs::write(&orphan, "unpublished").unwrap();
    let abandoned = dir
        .path()
        .join("staging")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&abandoned).unwrap();
    fs::write(abandoned.join("private.sqlite"), "unpublished").unwrap();
    let report = store.maintenance(2).unwrap();
    assert_eq!(report.tombstones_compacted, 2);
    assert_eq!(report.objects_removed, 1);
    assert!(report.revisions_removed > 0);
    assert!(report.snapshots_removed > 1);
    assert_eq!(report.staging_directories_removed, 1);
    assert_eq!(report.receipts_removed, 1);
    assert!(!orphan.exists());
    assert!(!abandoned.exists());
    assert!(store.receipt("owner", "first").unwrap().is_none());
    assert!(store.receipt("owner", "second").unwrap().is_some());
    assert!(store.receipt("owner", "third").unwrap().is_some());
    assert_eq!(
        run(
            &store,
            "db.query",
            json!({"namespace":"b","database":"app","sql":"SELECT x FROM n"})
        )["rows"],
        json!([[1]])
    );
    assert_eq!(
        run(&store, "function.get", json!({"namespace":"b","name":"f"}))["source"],
        "def f(): return 1"
    );
    assert_eq!(
        fs::read_dir(dir.path().join("snapshots")).unwrap().count(),
        1
    );
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.tool("namespace.create", json!({"name":"a"}))
            .unwrap_err()
            .code,
        state_store::ErrorCode::AlreadyExists
    );
    drop(tx);
    assert_eq!(store.maintenance(2).unwrap().snapshots_removed, 0);
}

#[test]
fn active_invocations_block_maintenance_and_exclusive_lock_blocks_begin() {
    let (dir, store) = setup();
    let first = store.begin().unwrap();
    let independent = Store::open(dir.path()).unwrap();
    let second = independent.begin().unwrap();
    assert_eq!(
        independent.maintenance(0).unwrap_err().code,
        state_store::ErrorCode::Conflict
    );
    drop(first);
    assert_eq!(
        store.maintenance(0).unwrap_err().code,
        state_store::ErrorCode::Conflict
    );
    drop(second);
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .open(dir.path().join("maintenance.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert_eq!(
        store.begin().err().unwrap().code,
        state_store::ErrorCode::Conflict
    );
    assert_eq!(
        Store::open(dir.path()).err().unwrap().code,
        state_store::ErrorCode::Conflict
    );
    // Parallel process tests can briefly inherit this descriptor between fork
    // and exec. Unlock the shared file description before asserting availability.
    lock.unlock().unwrap();
    drop(lock);
    store.maintenance(0).unwrap();
}

#[test]
fn reader_process_helper() {
    let Ok(root) = std::env::var("STATE_STORE_READER_ROOT") else {
        return;
    };
    let store = Store::open(&root).unwrap();
    let mut tx = store.begin().unwrap();
    fs::write(std::path::Path::new(&root).join("ready"), b"ready").unwrap();
    let mut signal = [0_u8; 1];
    std::io::stdin().read_exact(&mut signal).unwrap();
    if signal[0] == b'q' {
        let value = tx
            .tool(
                "db.query",
                json!({"namespace":"a","database":"app","sql":"SELECT x FROM n"}),
            )
            .unwrap();
        assert_eq!(value["rows"], json!([[1]]));
    }
    // Simulate a crash: no transaction destructors or lock cleanup in Rust.
    std::process::exit(73);
}

#[test]
fn cross_process_reader_keeps_old_snapshot_until_process_exits() {
    let (dir, store) = setup();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reader_process_helper", "--nocapture"])
        .env("STATE_STORE_READER_ROOT", dir.path())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !dir.path().join("ready").exists() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    sql(&store, "execute", "UPDATE n SET x=2");
    assert_eq!(
        store.maintenance(0).unwrap_err().code,
        state_store::ErrorCode::Conflict
    );
    child.stdin.take().unwrap().write_all(b"q").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(73));
    let report = store.maintenance(0).unwrap();
    assert!(report.snapshots_removed > 0);
    assert_eq!(report.staging_directories_removed, 1);
    assert_eq!(
        sql(&store, "query", "SELECT x FROM n")["rows"],
        json!([[2]])
    );
}

//! Manual wall-clock probe; not a performance assertion or a default test.
use serde_json::{Value, json};
use state_store::Store;
use std::{fs, time::Instant};

fn run(store: &Store, tool: &str, args: Value) -> Value {
    let mut tx = store.begin().unwrap();
    let value = tx.dispatch(tool, args).unwrap();
    tx.commit().unwrap();
    value
}

#[test]
#[ignore = "manual disk benchmark: creates and copies databases up to 256 MiB"]
fn portable_copy_and_namespace_fork() {
    println!(
        "target_mib,actual_bytes,raw_copy_ms,fork_ms,fork_new_snapshot_count,first_write_ms,write_new_snapshot_count"
    );
    for mib in [1_u64, 32, 256] {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        run(
            &store,
            "state_namespace",
            json!({"action":"create","name":"a"}),
        );
        run(
            &store,
            "state_db",
            json!({"action":"create","namespace":"a","database":"app"}),
        );
        let rows = mib * 16 - 2;
        run(
            &store,
            "state_db",
            json!({"action":"migrate","namespace":"a","database":"app","migrations":[{"id":"seed","sql":format!("CREATE TABLE payload(id INTEGER PRIMARY KEY, bytes BLOB); WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<{rows}) INSERT INTO payload SELECT n,zeroblob(65000) FROM seq;")}]}),
        );
        let db = run(&store, "state_db", json!({"action":"list","namespace":"a"}));
        let snapshot = db["databases"][0]["snapshot"].as_str().unwrap();
        let path = dir
            .path()
            .join("snapshots")
            .join(format!("{snapshot}.sqlite"));
        let bytes = fs::metadata(&path).unwrap().len();
        let start = Instant::now();
        fs::copy(&path, dir.path().join("copy.sqlite")).unwrap();
        let copy_ms = start.elapsed().as_secs_f64() * 1000.0;
        let count = || fs::read_dir(dir.path().join("snapshots")).unwrap().count();
        let before = count();
        let start = Instant::now();
        run(
            &store,
            "state_namespace",
            json!({"action":"copy","namespace":"a","name":"b"}),
        );
        let fork_ms = start.elapsed().as_secs_f64() * 1000.0;
        let fork_count = count() - before;
        assert_eq!(fork_count, 0);
        let start = Instant::now();
        run(
            &store,
            "state_db",
            json!({"action":"execute","namespace":"b","database":"app","sql":"UPDATE payload SET bytes=zeroblob(65000) WHERE id=1"}),
        );
        let write_ms = start.elapsed().as_secs_f64() * 1000.0;
        let write_count = count() - before;
        assert_eq!(write_count, 1);
        println!(
            "{mib},{bytes},{copy_ms:.3},{fork_ms:.3},{fork_count},{write_ms:.3},{write_count}"
        );
    }
}

use serde_json::{Value, json};
use state_store::{Store, Transaction};
use tempfile::TempDir;

fn setup() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}
fn run(store: &Store, tool: &str, args: Value) -> Value {
    let mut tx = store.begin().unwrap();
    let result = tx.dispatch(tool, args).unwrap();
    tx.commit().unwrap();
    result
}
fn namespace(store: &Store, name: &str) -> Value {
    run(
        store,
        "state_namespace",
        json!({"action":"create","name":name}),
    )
}
fn db(store: &Store, ns: &str) {
    run(
        store,
        "state_db",
        json!({"action":"create","namespace":ns,"database":"app"}),
    );
}
fn sql(store: &Store, ns: &str, action: &str, sql: &str) -> Value {
    run(
        store,
        "state_db",
        json!({"action":action,"namespace":ns,"database":"app","sql":sql}),
    )
}
fn txsql(tx: &mut Transaction, ns: &str, sql: &str) -> Value {
    tx.dispatch(
        "state_db",
        json!({"action":"execute","namespace":ns,"database":"app","sql":sql}),
    )
    .unwrap()
}
fn snapshot_count(dir: &TempDir) -> usize {
    std::fs::read_dir(dir.path().join("snapshots"))
        .unwrap()
        .count()
}

#[test]
fn fork_preserves_then_diverges_without_payload_copy() {
    let (dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    sql(
        &store,
        "a",
        "execute",
        "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT)",
    );
    sql(
        &store,
        "a",
        "execute",
        "INSERT INTO notes(body) VALUES ('first')",
    );
    run(
        &store,
        "state_fs",
        json!({"action":"write","namespace":"a","path":"/one.txt","text":"one"}),
    );
    let count = snapshot_count(&dir);
    run(
        &store,
        "state_namespace",
        json!({"action":"copy","namespace":"a","name":"b"}),
    );
    assert_eq!(snapshot_count(&dir), count);
    let a = run(
        &store,
        "state_namespace",
        json!({"action":"get","namespace":"a"}),
    );
    let b = run(
        &store,
        "state_namespace",
        json!({"action":"get","namespace":"b"}),
    );
    assert_eq!(a["revision"], b["revision"]);
    assert_ne!(a["id"], b["id"]);
    sql(&store, "b", "execute", "UPDATE notes SET body='second'");
    run(
        &store,
        "state_fs",
        json!({"action":"write","namespace":"b","path":"/one.txt","text":"two"}),
    );
    assert_eq!(
        sql(&store, "a", "query", "SELECT body FROM notes")["rows"],
        json!([["first"]])
    );
    assert_eq!(
        sql(&store, "b", "query", "SELECT body FROM notes")["rows"],
        json!([["second"]])
    );
    assert_eq!(
        run(
            &store,
            "state_fs",
            json!({"action":"read","namespace":"a","path":"/one.txt"})
        )["text"],
        "one"
    );
}

#[test]
fn staged_copy_is_independent() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let mut tx = store.begin().unwrap();
    txsql(&mut tx, "a", "CREATE TABLE n(x)");
    txsql(&mut tx, "a", "INSERT INTO n VALUES(1)");
    tx.dispatch(
        "state_namespace",
        json!({"action":"copy","namespace":"a","name":"b"}),
    )
    .unwrap();
    txsql(&mut tx, "a", "UPDATE n SET x=2");
    txsql(&mut tx, "b", "INSERT INTO n VALUES(3)");
    tx.commit().unwrap();
    assert_eq!(
        sql(&store, "a", "query", "SELECT x FROM n")["rows"],
        json!([[2]])
    );
    assert_eq!(
        sql(&store, "b", "query", "SELECT x FROM n ORDER BY x")["rows"],
        json!([[1], [3]])
    );
}

#[test]
fn rename_preserves_uuid_and_reserves_deleted_names() {
    let (_dir, store) = setup();
    let a = namespace(&store, "a");
    assert_eq!(
        run(
            &store,
            "state_namespace",
            json!({"action":"update","namespace":"a","name":"b"})
        )["id"],
        a["id"]
    );
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.dispatch("state_namespace", json!({"action":"get","namespace":"a"}))
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
    run(
        &store,
        "state_namespace",
        json!({"action":"delete","namespace":"b"}),
    );
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.dispatch("state_namespace", json!({"action":"create","name":"b"}))
            .unwrap_err()
            .code,
        "ALREADY_EXISTS"
    );
}

#[test]
fn names_cannot_alias_existing_uuids() {
    let (_dir, store) = setup();
    let a = namespace(&store, "a");
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.dispatch("state_namespace", json!({"action":"create","name":a["id"]}))
            .unwrap_err()
            .code,
        "ALREADY_EXISTS"
    );
}

#[test]
fn paths_cannot_escape_or_conflict_with_directories() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    for path in ["/../x", "../../x", "/a\0x", "/a\\x", "relative"] {
        let mut tx = store.begin().unwrap();
        assert_eq!(
            tx.dispatch(
                "state_fs",
                json!({"action":"write","namespace":"a","path":path,"text":"x"})
            )
            .unwrap_err()
            .code,
            "INVALID_ARGUMENT"
        );
    }
    run(
        &store,
        "state_fs",
        json!({"action":"write","namespace":"a","path":"/dir/x","text":"ok"}),
    );
    let mut tx = store.begin().unwrap();
    assert!(
        tx.dispatch(
            "state_fs",
            json!({"action":"write","namespace":"a","path":"/dir","text":"bad"})
        )
        .is_err()
    );
}

#[test]
fn restart_preserves_committed_state_and_rollback_hides_everything() {
    let (dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    sql(&store, "a", "execute", "CREATE TABLE n(x)");
    namespace(&store, "b");
    db(&store, "b");
    sql(&store, "b", "execute", "CREATE TABLE n(x)");
    let mut tx = store.begin().unwrap();
    txsql(&mut tx, "a", "INSERT INTO n VALUES(1)");
    txsql(&mut tx, "b", "INSERT INTO n VALUES(2)");
    tx.dispatch(
        "state_fs",
        json!({"action":"write","namespace":"a","path":"/pending","text":"x"}),
    )
    .unwrap();
    assert!(
        tx.dispatch(
            "state_db",
            json!({"action":"execute","namespace":"b","database":"app","sql":"invalid sql"})
        )
        .is_err()
    );
    assert_eq!(tx.commit().unwrap_err().code, "TRANSACTION_ABORTED");
    drop(store);
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(
        sql(&reopened, "a", "query", "SELECT * FROM n")["rows"],
        json!([])
    );
    assert_eq!(
        sql(&reopened, "b", "query", "SELECT * FROM n")["rows"],
        json!([])
    );
    assert_eq!(
        run(
            &reopened,
            "state_fs",
            json!({"action":"list","namespace":"a"})
        )["entries"],
        json!([])
    );
}

#[test]
fn read_dependency_prevents_write_skew() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    namespace(&store, "b");
    let mut first = store.begin().unwrap();
    let mut second = store.begin().unwrap();
    first
        .dispatch("state_namespace", json!({"action":"get","namespace":"b"}))
        .unwrap();
    second
        .dispatch(
            "state_fs",
            json!({"action":"write","namespace":"b","path":"/x","text":"2"}),
        )
        .unwrap();
    second.commit().unwrap();
    first
        .dispatch(
            "state_fs",
            json!({"action":"write","namespace":"a","path":"/x","text":"1"}),
        )
        .unwrap();
    assert_eq!(first.commit().unwrap_err().code, "CONFLICT");
}

#[test]
fn migration_is_atomic_checksummed_and_ordered() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let first = json!({"id":"001","sql":"CREATE TABLE n(x); INSERT INTO n VALUES(1);"});
    let second = json!({"id":"002","sql":"CREATE INDEX n_x ON n(x);"});
    let migrations = json!([first, second]);
    let apply = |migrations: Value| {
        run(
            &store,
            "state_db",
            json!({"action":"migrate","namespace":"a","database":"app","migrations":migrations}),
        )
    };
    assert_eq!(apply(migrations.clone())["applied"], 2);
    assert_eq!(apply(migrations.clone())["applied"], 0);
    for invalid in [
        json!([{"id":"001","sql":"CREATE TABLE wrong(x)"}]),
        json!([second, first]),
        json!([first,{"id":"003","sql":"CREATE TABLE bad(x)"}]),
    ] {
        let mut tx = store.begin().unwrap();
        assert_eq!(
            tx.dispatch(
                "state_db",
                json!({"action":"migrate","namespace":"a","database":"app","migrations":invalid})
            )
            .unwrap_err()
            .code,
            "MIGRATION_MISMATCH"
        );
    }
    let mut tx = store.begin().unwrap();
    assert!(tx.dispatch("state_db",json!({"action":"migrate","namespace":"a","database":"app","migrations":[{"id":"003","sql":"CREATE TABLE uncommitted(x); BAD SQL"}]})).is_err());
    assert_eq!(
        sql(
            &store,
            "a",
            "query",
            "SELECT name FROM sqlite_schema WHERE name='uncommitted'"
        )["rows"],
        json!([])
    );
    assert_eq!(
        apply(json!([{"id":"003","sql":"INSERT INTO n VALUES(2)"}]))["applied"],
        1
    );
}

#[test]
fn introspection_matches_schema_and_internal_ledger_is_invisible() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    run(
        &store,
        "state_db",
        json!({"action":"migrate","namespace":"a","database":"app","migrations":[{"id":"001","sql":"CREATE TABLE p(id INTEGER PRIMARY KEY); CREATE TABLE c(id INTEGER PRIMARY KEY, p INTEGER REFERENCES p(id), title TEXT NOT NULL DEFAULT 'hello'); CREATE INDEX c_p ON c(p); CREATE VIEW cv AS SELECT title FROM c; CREATE TRIGGER ct AFTER INSERT ON p BEGIN INSERT INTO c(p) VALUES(new.id); END;"}]}),
    );
    let info = run(
        &store,
        "state_db",
        json!({"action":"inspect","namespace":"a","database":"app"}),
    );
    let c = info["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "c")
        .unwrap();
    assert_eq!(c["columns"]["rows"].as_array().unwrap().len(), 3);
    assert_eq!(c["foreign_keys"]["rows"][0][2], "p");
    assert_eq!(c["indexes"][0]["name"], "c_p");
    assert_eq!(info["migrations"].as_array().unwrap().len(), 1);
    assert_eq!(
        sql(
            &store,
            "a",
            "query",
            "SELECT name FROM sqlite_schema WHERE name IN ('receipts','migrations','namespaces')"
        )["rows"],
        json!([])
    );
    assert_eq!(
        sql(&store, "a", "query", "PRAGMA table_info(c)")["rows"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn sql_cannot_attach_catalog_write_host_files_or_mutate_pragmas() {
    let (dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let target = dir.path().join("escape.sqlite");
    let hostile = [
        format!(
            "ATTACH '{}' AS private",
            dir.path().join("catalog.sqlite").display()
        ),
        format!("VACUUM INTO '{}'", target.display()),
        "PRAGMA writable_schema=ON".into(),
        "PRAGMA journal_mode=WAL".into(),
        "PRAGMA temp_store_directory='/tmp'".into(),
        "PRAGMA user_version=2".into(),
        "PRAGMA hard_heap_limit=1".into(),
        "PRAGMA soft_heap_limit=1".into(),
        "PRAGMA shrink_memory".into(),
        "PRAGMA locking_mode=EXCLUSIVE".into(),
        "PRAGMA main.writable_schema=ON".into(),
        "SELECT readfile('/etc/passwd')".into(),
        "SELECT writefile('/tmp/state-store-escape', 'bad')".into(),
        "DETACH DATABASE main".into(),
        "CREATE VIRTUAL TABLE escape USING csv(filename='/etc/passwd')".into(),
        "SELECT load_extension('/tmp/bad')".into(),
        "BEGIN".into(),
        "SAVEPOINT x".into(),
    ];
    for statement in hostile {
        for action in ["query", "execute"] {
            let mut tx = store.begin().unwrap();
            assert!(
                tx.dispatch(
                    "state_db",
                    json!({"action":action,"namespace":"a","database":"app","sql":statement})
                )
                .is_err(),
                "{action}: {statement}"
            );
        }
    }
    assert!(!target.exists());
}

#[test]
fn read_only_query_and_single_statement_are_enforced() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    for statement in [
        "CREATE TABLE bad(x)",
        "SELECT 1; CREATE TABLE bad(x)",
        "SELECT 1; SELECT 2",
    ] {
        let mut tx = store.begin().unwrap();
        assert!(
            tx.dispatch(
                "state_db",
                json!({"action":"query","namespace":"a","database":"app","sql":statement})
            )
            .is_err()
        );
    }
    assert_eq!(
        sql(&store, "a", "query", "SELECT name FROM sqlite_schema")["rows"],
        json!([])
    );
}

#[test]
fn endpoint_source_and_database_ids_are_pinned() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    run(
        &store,
        "state_fs",
        json!({"action":"write","namespace":"a","path":"/api.py","text":"def f():\n return 1\n"}),
    );
    let declaration = run(
        &store,
        "state_function",
        json!({"action":"declare","namespace":"a","name":"f","file":"/api.py","symbol":"f","databases":{"app":"read"}}),
    );
    run(
        &store,
        "state_fs",
        json!({"action":"write","namespace":"a","path":"/api.py","text":"def f():\n return 2\n"}),
    );
    assert_eq!(
        run(
            &store,
            "state_function",
            json!({"action":"get","namespace":"a","name":"f"})
        )["source"],
        "def f():\n return 1\n"
    );
    run(
        &store,
        "state_namespace",
        json!({"action":"copy","namespace":"a","name":"b"}),
    );
    assert_eq!(
        run(
            &store,
            "state_function",
            json!({"action":"get","namespace":"b","name":"f"})
        )["database_ids"],
        declaration["database_ids"]
    );
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.dispatch(
            "state_db",
            json!({"action":"drop","namespace":"a","database":"app"})
        )
        .unwrap_err()
        .code,
        "CONFLICT"
    );
}

#[test]
fn receipts_commit_with_state_and_resolve_concurrent_duplicate() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    let mut first = store.begin().unwrap();
    let mut duplicate = store.begin().unwrap();
    for tx in [&mut first, &mut duplicate] {
        tx.dispatch(
            "state_fs",
            json!({"action":"write","namespace":"a","path":"/x","text":"once"}),
        )
        .unwrap();
        tx.set_receipt("owner", "key", "hash", json!({"ok":true}))
            .unwrap();
    }
    first.commit().unwrap();
    let replay = duplicate.commit().unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["result"], json!({"ok":true}));
    assert_eq!(
        store.receipt("owner", "key").unwrap().unwrap().request_hash,
        "hash"
    );
    let mut wrong = store.begin().unwrap();
    wrong
        .set_receipt("owner", "key", "other", json!(1))
        .unwrap();
    assert_eq!(wrong.commit().unwrap_err().code, "IDEMPOTENCY_MISMATCH");
    let mut dropped = store.begin().unwrap();
    dropped
        .set_receipt("owner", "dropped", "hash", json!(1))
        .unwrap();
    drop(dropped);
    assert!(store.receipt("owner", "dropped").unwrap().is_none());
}

#[test]
fn blobs_large_integers_duplicate_columns_and_returning_round_trip() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    sql(&store, "a", "execute", "CREATE TABLE n(a,b)");
    let result = run(
        &store,
        "state_db",
        json!({"action":"execute","namespace":"a","database":"app","sql":"INSERT INTO n VALUES(?,?) RETURNING a AS x,b AS x","params":[{"$base64":"AAH/"},{"$integer":"9223372036854775807"}]}),
    );
    assert_eq!(result["columns"], json!(["x", "x"]));
    assert_eq!(
        result["rows"],
        json!([[{"$base64":"AAH/"},{"$integer":"9223372036854775807"}]])
    );
    assert_eq!(result["rows_affected"], 1);
}

#[test]
fn excessive_output_and_sql_time_are_bounded() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let mut tx = store.begin().unwrap();
    let err=tx.dispatch("state_db",json!({"action":"query","namespace":"a","database":"app","sql":"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<11000) SELECT x FROM n"})).unwrap_err();
    assert_eq!(err.code, "LIMIT_EXCEEDED");
    let start = std::time::Instant::now();
    let mut tx = store.begin().unwrap();
    let err=tx.dispatch("state_db",json!({"action":"query","namespace":"a","database":"app","sql":"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n"})).unwrap_err();
    assert_eq!(err.code, "LIMIT_EXCEEDED");
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(sql(&store, "a", "query", "SELECT 1")["rows"], json!([[1]]));
}

#[test]
fn stale_revision_and_missing_database_are_explicit_errors() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    let mut tx = store.begin().unwrap();
    assert_eq!(tx.dispatch("state_fs",json!({"action":"write","namespace":"a","path":"/x","text":"x","expected_revision":"stale"})).unwrap_err().code,"CONFLICT");
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.dispatch(
            "state_db",
            json!({"action":"query","namespace":"a","database":"typo","sql":"SELECT 1"})
        )
        .unwrap_err()
        .code,
        "NOT_FOUND"
    );
}

#[test]
fn working_connection_preserves_temporary_state_and_copies_once() {
    let (dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let mut tx = store.begin().unwrap();
    let before = snapshot_count(&dir);
    txsql(&mut tx, "a", "CREATE TABLE n(id INTEGER PRIMARY KEY, x)");
    txsql(&mut tx, "a", "INSERT INTO n(x) VALUES(7)");
    txsql(&mut tx, "a", "CREATE TEMP TABLE scratch(v)");
    txsql(&mut tx, "a", "INSERT INTO scratch VALUES(42)");
    assert_eq!(
        snapshot_count(&dir),
        before,
        "writes remain private until publication"
    );
    let query = |tx: &mut Transaction, source: &str| {
        tx.dispatch(
            "state_db",
            json!({"action":"query","namespace":"a","database":"app","sql":source}),
        )
        .unwrap()
    };
    assert_eq!(
        query(&mut tx, "SELECT last_insert_rowid()")["rows"],
        json!([[1]])
    );
    tx.dispatch(
        "state_namespace",
        json!({"action":"copy","namespace":"a","name":"b"}),
    )
    .unwrap();
    assert_eq!(
        query(&mut tx, "SELECT v FROM scratch")["rows"],
        json!([[42]])
    );
    txsql(&mut tx, "a", "UPDATE n SET x=8");
    tx.commit().unwrap();
    assert_eq!(
        snapshot_count(&dir),
        before + 2,
        "one frozen fork and one final generation"
    );
    assert_eq!(
        sql(&store, "b", "query", "SELECT x FROM n")["rows"],
        json!([[7]])
    );
    assert_eq!(
        sql(&store, "a", "query", "SELECT x FROM n")["rows"],
        json!([[8]])
    );
    assert_eq!(
        sql(&store, "a", "query", "SELECT name FROM sqlite_temp_schema")["rows"],
        json!([])
    );
}

#[test]
fn exact_migration_replay_does_not_create_a_revision_or_snapshot() {
    let (dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let args = json!({"action":"migrate","namespace":"a","database":"app","migrations":[{"id":"1","sql":"CREATE TABLE n(x)"}]});
    run(&store, "state_db", args.clone());
    let before = snapshot_count(&dir);
    let ns = run(
        &store,
        "state_namespace",
        json!({"action":"get","namespace":"a"}),
    );
    let mut tx = store.begin().unwrap();
    assert_eq!(tx.dispatch("state_db", args).unwrap()["applied"], 0);
    assert_eq!(tx.commit().unwrap()["changed"], false);
    assert_eq!(before, snapshot_count(&dir));
    assert_eq!(
        ns["revision"],
        run(
            &store,
            "state_namespace",
            json!({"action":"get","namespace":"a"})
        )["revision"]
    );
}

#[test]
fn crash_helper() {
    let Ok(root) = std::env::var("STATE_STORE_CRASH_ROOT") else {
        return;
    };
    let store = Store::open(root).unwrap();
    let mut tx = store.begin().unwrap();
    txsql(&mut tx, "a", "INSERT INTO n VALUES(1)");
    txsql(&mut tx, "b", "INSERT INTO n VALUES(2)");
    // A copy freezes staged bytes into snapshots, but does not publish them.
    tx.dispatch(
        "state_namespace",
        json!({"action":"copy","namespace":"a","name":"copy"}),
    )
    .unwrap();
    tx.set_receipt("owner", "crash-key", "request", json!("committed"))
        .unwrap();
    if std::env::var("STATE_STORE_CRASH_COMMIT").as_deref() == Ok("yes") {
        tx.commit().unwrap();
    }
    std::process::exit(71);
}

#[test]
fn abrupt_process_exit_publishes_all_or_none_and_receipt_survives() {
    for commit in [false, true] {
        let (dir, store) = setup();
        for ns in ["a", "b"] {
            namespace(&store, ns);
            db(&store, ns);
            sql(&store, ns, "execute", "CREATE TABLE n(x)");
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_helper", "--nocapture"])
            .env("STATE_STORE_CRASH_ROOT", dir.path())
            .env(
                "STATE_STORE_CRASH_COMMIT",
                if commit { "yes" } else { "no" },
            )
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(71));
        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(
            sql(&reopened, "a", "query", "SELECT x FROM n")["rows"],
            if commit { json!([[1]]) } else { json!([]) }
        );
        assert_eq!(
            sql(&reopened, "b", "query", "SELECT x FROM n")["rows"],
            if commit { json!([[2]]) } else { json!([]) }
        );
        assert_eq!(
            reopened.receipt("owner", "crash-key").unwrap().is_some(),
            commit
        );
    }
}

#[test]
fn large_schema_inspection_stops_at_cumulative_result_budget() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let columns = (0..128)
        .map(|i| format!("column_{i} TEXT"))
        .collect::<Vec<_>>()
        .join(",");
    let source = (0..180)
        .map(|i| format!("CREATE TABLE wide_{i}({columns});"))
        .collect::<String>();
    run(
        &store,
        "state_db",
        json!({"action":"migrate","namespace":"a","database":"app","migrations":[{"id":"wide","sql":source}]}),
    );
    let mut tx = store.begin().unwrap();
    let error = tx
        .dispatch(
            "state_db",
            json!({"action":"inspect","namespace":"a","database":"app"}),
        )
        .unwrap_err();
    assert_eq!(error.code, "LIMIT_EXCEEDED");
    assert!(error.message.contains("1 MiB"));
}

#[test]
fn explicit_temporary_tables_have_a_page_quota() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let mut tx = store.begin().unwrap();
    txsql(&mut tx, "a", "CREATE TEMP TABLE bounded(bytes BLOB)");
    let error = tx.dispatch("state_db", json!({"action":"execute","namespace":"a","database":"app","sql":"WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<40) INSERT INTO bounded SELECT zeroblob(524288) FROM seq"})).unwrap_err();
    assert_eq!(error.code, "LIMIT_EXCEEDED");
    assert_eq!(tx.commit().unwrap_err().code, "TRANSACTION_ABORTED");
    assert_eq!(
        sql(&store, "a", "query", "SELECT name FROM sqlite_schema")["rows"],
        json!([])
    );
}

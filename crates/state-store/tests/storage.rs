mod support;
use serde_json::{Value, json};
use state_store::{Store, Transaction};
use support::ToolCalls;
use tempfile::TempDir;

fn setup() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}
fn run(store: &Store, tool: &str, args: Value) -> Value {
    let mut tx = store.begin().unwrap();
    let result = tx.tool(tool, args).unwrap();
    tx.commit_result().unwrap();
    result
}
fn namespace(store: &Store, name: &str) -> Value {
    run(store, "namespace.create", json!({"name":name}))
}
fn db(store: &Store, ns: &str) {
    run(store, "db.create", json!({"namespace":ns,"database":"app"}));
}
fn sql(store: &Store, ns: &str, action: &str, sql: &str) -> Value {
    run(
        store,
        &format!("db.{action}"),
        json!({"namespace":ns,"database":"app","sql":sql}),
    )
}
fn txsql(tx: &mut Transaction, ns: &str, sql: &str) -> Value {
    tx.tool(
        "db.execute",
        json!({"namespace":ns,"database":"app","sql":sql}),
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
        "fs.write",
        json!({"namespace":"a","path":"/one.txt","text":"one"}),
    );
    let count = snapshot_count(&dir);
    run(
        &store,
        "namespace.copy",
        json!({"namespace":"a","name":"b"}),
    );
    assert_eq!(snapshot_count(&dir), count);
    let a = run(&store, "namespace.get", json!({"namespace":"a"}));
    let b = run(&store, "namespace.get", json!({"namespace":"b"}));
    assert_eq!(a["revision"], b["revision"]);
    assert_ne!(a["id"], b["id"]);
    sql(&store, "b", "execute", "UPDATE notes SET body='second'");
    run(
        &store,
        "fs.write",
        json!({"namespace":"b","path":"/one.txt","text":"two"}),
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
            "fs.read",
            json!({"namespace":"a","path":"/one.txt"})
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
    tx.tool("namespace.copy", json!({"namespace":"a","name":"b"}))
        .unwrap();
    txsql(&mut tx, "a", "UPDATE n SET x=2");
    txsql(&mut tx, "b", "INSERT INTO n VALUES(3)");
    tx.commit_result().unwrap();
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
            "namespace.update",
            json!({"namespace":"a","name":"b"})
        )["id"],
        a["id"]
    );
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.tool("namespace.get", json!({"namespace":"a"}))
            .unwrap_err()
            .code,
        state_store::ErrorCode::NotFound
    );
    run(&store, "namespace.delete", json!({"namespace":"b"}));
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.tool("namespace.create", json!({"name":"b"}))
            .unwrap_err()
            .code,
        state_store::ErrorCode::AlreadyExists
    );
}

#[test]
fn names_cannot_alias_existing_uuids() {
    let (_dir, store) = setup();
    let a = namespace(&store, "a");
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.tool("namespace.create", json!({"name":a["id"]}))
            .unwrap_err()
            .code,
        state_store::ErrorCode::AlreadyExists
    );
}

#[test]
fn paths_cannot_escape_or_conflict_with_directories() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    for path in ["/../x", "../../x", "/a\0x", "/a\\x", "relative"] {
        let mut tx = store.begin().unwrap();
        assert_eq!(
            tx.tool("fs.write", json!({"namespace":"a","path":path,"text":"x"}))
                .unwrap_err()
                .code,
            state_store::ErrorCode::InvalidArgument
        );
    }
    run(
        &store,
        "fs.write",
        json!({"namespace":"a","path":"/dir/x","text":"ok"}),
    );
    let mut tx = store.begin().unwrap();
    assert!(
        tx.tool(
            "fs.write",
            json!({"namespace":"a","path":"/dir","text":"bad"})
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
    tx.tool(
        "fs.write",
        json!({"namespace":"a","path":"/pending","text":"x"}),
    )
    .unwrap();
    assert!(
        tx.tool(
            "db.execute",
            json!({"namespace":"b","database":"app","sql":"invalid sql"})
        )
        .is_err()
    );
    assert_eq!(
        tx.commit_result().unwrap_err().code,
        state_store::ErrorCode::TransactionAborted
    );
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
        run(&reopened, "fs.list", json!({"namespace":"a"}))["entries"],
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
        .tool("namespace.get", json!({"namespace":"b"}))
        .unwrap();
    second
        .tool("fs.write", json!({"namespace":"b","path":"/x","text":"2"}))
        .unwrap();
    second.commit_result().unwrap();
    first
        .tool("fs.write", json!({"namespace":"a","path":"/x","text":"1"}))
        .unwrap();
    assert_eq!(
        first.commit_result().unwrap_err().code,
        state_store::ErrorCode::Conflict
    );
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
            "db.migrate",
            json!({"namespace":"a","database":"app","migrations":migrations}),
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
            tx.tool(
                "db.migrate",
                json!({"namespace":"a","database":"app","migrations":invalid})
            )
            .unwrap_err()
            .code,
            state_store::ErrorCode::MigrationMismatch
        );
    }
    let mut tx = store.begin().unwrap();
    assert!(tx.tool("db.migrate",json!({"namespace":"a","database":"app","migrations":[{"id":"003","sql":"CREATE TABLE uncommitted(x); BAD SQL"}]})).is_err());
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
        "db.migrate",
        json!({"namespace":"a","database":"app","migrations":[{"id":"001","sql":"CREATE TABLE p(id INTEGER PRIMARY KEY); CREATE TABLE c(id INTEGER PRIMARY KEY, p INTEGER REFERENCES p(id), title TEXT NOT NULL DEFAULT 'hello'); CREATE INDEX c_p ON c(p); CREATE VIEW cv AS SELECT title FROM c; CREATE TRIGGER ct AFTER INSERT ON p BEGIN INSERT INTO c(p) VALUES(new.id); END;"}]}),
    );
    let info = run(
        &store,
        "db.inspect",
        json!({"namespace":"a","database":"app"}),
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
                tx.tool(
                    &format!("db.{action}"),
                    json!({"namespace":"a","database":"app","sql":statement})
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
            tx.tool(
                "db.query",
                json!({"namespace":"a","database":"app","sql":statement})
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
fn endpoint_source_is_pinned_but_databases_can_be_dropped() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    run(
        &store,
        "fs.write",
        json!({"namespace":"a","path":"/api.py","text":"def f():\n return 1\n"}),
    );
    let declaration = run(
        &store,
        "function.declare",
        json!({"namespace":"a","name":"f","file":"/api.py","symbol":"f"}),
    );
    run(
        &store,
        "fs.write",
        json!({"namespace":"a","path":"/api.py","text":"def f():\n return 2\n"}),
    );
    assert_eq!(
        run(&store, "function.get", json!({"namespace":"a","name":"f"}))["source"],
        "def f():\n return 1\n"
    );
    run(
        &store,
        "namespace.copy",
        json!({"namespace":"a","name":"b"}),
    );
    assert_eq!(
        run(&store, "function.get", json!({"namespace":"b","name":"f"}))["version"],
        declaration["version"]
    );
    run(&store, "db.drop", json!({"namespace":"a","database":"app"}));
    run(
        &store,
        "db.create",
        json!({"namespace":"a","database":"app"}),
    );
    assert_eq!(
        run(&store, "function.get", json!({"namespace":"a","name":"f"}))["version"],
        declaration["version"]
    );
}

#[test]
fn receipts_commit_with_state_and_resolve_concurrent_duplicate() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    let mut first = store.begin().unwrap();
    let mut duplicate = store.begin().unwrap();
    for tx in [&mut first, &mut duplicate] {
        tx.tool(
            "fs.write",
            json!({"namespace":"a","path":"/x","text":"once"}),
        )
        .unwrap();
        tx.set_receipt(
            "owner",
            "key",
            "hash",
            state_store::records::ReceiptResult::Function(json!({"ok":true})),
        )
        .unwrap();
    }
    first.commit_result().unwrap();
    let replay = duplicate.commit_result().unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["result"], json!({"ok":true}));
    assert_eq!(
        store.receipt("owner", "key").unwrap().unwrap().request_hash,
        "hash"
    );
    let mut wrong = store.begin().unwrap();
    wrong
        .set_receipt(
            "owner",
            "key",
            "other",
            state_store::records::ReceiptResult::Function(json!(1)),
        )
        .unwrap();
    assert_eq!(
        wrong.commit_result().unwrap_err().code,
        state_store::ErrorCode::IdempotencyMismatch
    );
    let mut dropped = store.begin().unwrap();
    dropped
        .set_receipt(
            "owner",
            "dropped",
            "hash",
            state_store::records::ReceiptResult::Function(json!(1)),
        )
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
        "db.execute",
        json!({"namespace":"a","database":"app","sql":"INSERT INTO n VALUES(?,?) RETURNING a AS x,b AS x","params":[{"$base64":"AAH/"},{"$integer":"9223372036854775807"}]}),
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
    let err=tx.tool("db.query",json!({"namespace":"a","database":"app","sql":"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<11000) SELECT x FROM n"})).unwrap_err();
    assert_eq!(err.code, state_store::ErrorCode::LimitExceeded);
    let start = std::time::Instant::now();
    let mut tx = store.begin().unwrap();
    let err=tx.tool("db.query",json!({"namespace":"a","database":"app","sql":"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n"})).unwrap_err();
    assert_eq!(err.code, state_store::ErrorCode::LimitExceeded);
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(sql(&store, "a", "query", "SELECT 1")["rows"], json!([[1]]));
}

#[test]
fn stale_revision_and_missing_database_are_explicit_errors() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.tool(
            "fs.write",
            json!({"namespace":"a","path":"/x","text":"x","expected_revision":"stale"})
        )
        .unwrap_err()
        .code,
        state_store::ErrorCode::Conflict
    );
    let mut tx = store.begin().unwrap();
    assert_eq!(
        tx.tool(
            "db.query",
            json!({"namespace":"a","database":"typo","sql":"SELECT 1"})
        )
        .unwrap_err()
        .code,
        state_store::ErrorCode::NotFound
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
        tx.tool(
            "db.query",
            json!({"namespace":"a","database":"app","sql":source}),
        )
        .unwrap()
    };
    assert_eq!(
        query(&mut tx, "SELECT last_insert_rowid()")["rows"],
        json!([[1]])
    );
    tx.tool("namespace.copy", json!({"namespace":"a","name":"b"}))
        .unwrap();
    assert_eq!(
        query(&mut tx, "SELECT v FROM scratch")["rows"],
        json!([[42]])
    );
    txsql(&mut tx, "a", "UPDATE n SET x=8");
    tx.commit_result().unwrap();
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
    let args = json!({"namespace":"a","database":"app","migrations":[{"id":"1","sql":"CREATE TABLE n(x)"}]});
    run(&store, "db.migrate", args.clone());
    let before = snapshot_count(&dir);
    let ns = run(&store, "namespace.get", json!({"namespace":"a"}));
    let mut tx = store.begin().unwrap();
    assert_eq!(tx.tool("db.migrate", args).unwrap()["applied"], 0);
    assert_eq!(tx.commit_result().unwrap()["changed"], false);
    assert_eq!(before, snapshot_count(&dir));
    assert_eq!(
        ns["revision"],
        run(&store, "namespace.get", json!({"namespace":"a"}))["revision"]
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
    tx.tool("namespace.copy", json!({"namespace":"a","name":"copy"}))
        .unwrap();
    tx.set_receipt(
        "owner",
        "crash-key",
        "request",
        state_store::records::ReceiptResult::Function(json!("committed")),
    )
    .unwrap();
    if std::env::var("STATE_STORE_CRASH_COMMIT").as_deref() == Ok("yes") {
        tx.commit_result().unwrap();
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
        "db.migrate",
        json!({"namespace":"a","database":"app","migrations":[{"id":"wide","sql":source}]}),
    );
    let mut tx = store.begin().unwrap();
    let error = tx
        .tool("db.inspect", json!({"namespace":"a","database":"app"}))
        .unwrap_err();
    assert_eq!(error.code, state_store::ErrorCode::LimitExceeded);
    assert!(error.message.contains("1 MiB"));
}

#[test]
fn explicit_temporary_tables_have_a_page_quota() {
    let (_dir, store) = setup();
    namespace(&store, "a");
    db(&store, "a");
    let mut tx = store.begin().unwrap();
    txsql(&mut tx, "a", "CREATE TEMP TABLE bounded(bytes BLOB)");
    let error = tx.tool("db.execute", json!({"namespace":"a","database":"app","sql":"WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<40) INSERT INTO bounded SELECT zeroblob(524288) FROM seq"})).unwrap_err();
    assert_eq!(error.code, state_store::ErrorCode::LimitExceeded);
    assert_eq!(
        tx.commit_result().unwrap_err().code,
        state_store::ErrorCode::TransactionAborted
    );
    assert_eq!(
        sql(&store, "a", "query", "SELECT name FROM sqlite_schema")["rows"],
        json!([])
    );
}

#[test]
fn typed_operations_abort_publication_after_a_caught_migration_failure() {
    use state_store::inputs::{DbExecuteInput, DbMigrateInput, DbQueryInput, MigrationInput};
    let (_directory, store) = setup();
    namespace(&store, "typed");
    db(&store, "typed");
    sql(
        &store,
        "typed",
        "execute",
        "CREATE TABLE entries(value INTEGER)",
    );
    let mut tx = store.begin().unwrap();
    tx.execute_database(&DbExecuteInput {
        namespace: "typed".into(),
        database: "app".into(),
        sql: "INSERT INTO entries VALUES (1)".into(),
        params: None,
        expected_revision: None,
    })
    .unwrap();
    let failure = tx
        .migrate_database(&DbMigrateInput {
            namespace: "typed".into(),
            database: "app".into(),
            expected_revision: None,
            migrations: vec![
                MigrationInput {
                    id: "first".into(),
                    sql: "INSERT INTO entries VALUES (2)".into(),
                },
                MigrationInput {
                    id: "broken".into(),
                    sql: "INSERT INTO missing VALUES (3)".into(),
                },
            ],
        })
        .unwrap_err();
    assert_ne!(failure.code, state_store::ErrorCode::TransactionAborted);
    let query = DbQueryInput {
        namespace: "typed".into(),
        database: "app".into(),
        sql: "SELECT COUNT(*) FROM entries".into(),
        params: None,
    };
    assert_eq!(
        tx.query_database(&query).unwrap_err().code,
        state_store::ErrorCode::TransactionAborted
    );
    assert_eq!(
        tx.commit_result().unwrap_err().code,
        state_store::ErrorCode::TransactionAborted
    );
    let mut fresh = store.begin().unwrap();
    assert_eq!(
        fresh.query_database(&query).unwrap().rows,
        vec![vec![state_store::records::SqlCell::Integer(0)]]
    );
    assert!(
        fresh
            .database_migrations(&state_store::inputs::DbMigrationsInput {
                namespace: "typed".into(),
                database: "app".into()
            })
            .unwrap()
            .migrations
            .is_empty()
    );
}

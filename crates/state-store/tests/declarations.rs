use serde_json::json;
use state_store::{FunctionDeclaration, Store};

#[test]
fn legacy_grants_are_discarded_without_republishing_code() {
    let wire = json!({
        "name":"f", "file":"/api.py", "symbol":"f", "source":"def f(): pass",
        "source_hash":"hash", "abi_version":1, "version":"existing-version",
        "input_schema":{"type":"object"}, "output_schema":true,
        "databases":{"app":"read"}, "database_ids":{"app":"db-id"},
        "files":{"/notes//./":"write"}, "calls":[{"namespace":"self","function":"notify"}]
    });
    let declaration: FunctionDeclaration = serde_json::from_value(wire).unwrap();
    assert_eq!(declaration.version, "existing-version");
    assert_eq!(declaration.source, "def f(): pass");
    let encoded = serde_json::to_value(&declaration).unwrap();
    for field in ["databases", "files", "calls", "database_ids"] {
        assert!(encoded.get(field).is_none());
    }
    let restarted: FunctionDeclaration = serde_json::from_value(encoded).unwrap();
    assert_eq!(restarted.version, declaration.version);
    assert_eq!(restarted.source, declaration.source);
}

#[test]
fn malformed_persisted_record_fields_are_rejected_at_read_boundary() {
    for wire in [
        json!({"name":"f", "source":42}),
        json!({"name":"f", "file":"/api.py", "symbol":"f", "source":"source", "source_hash":"hash", "abi_version":"1", "version":"v", "database_ids":{}}),
        json!({"name":"f", "file":"/api.py", "symbol":"f", "source":"source", "source_hash":"hash", "abi_version":1, "version":"v", "database_ids":{}, "source_hash":42}),
    ] {
        assert!(serde_json::from_value::<FunctionDeclaration>(wire).is_err());
    }
}

#[test]
fn published_versions_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut tx = store.begin().unwrap();
    tx.dispatch("state_namespace", json!({"action":"create", "name":"ns"}))
        .unwrap();
    tx.dispatch("state_fs", json!({"action":"write", "namespace":"ns", "path":"/api.py", "text":"def f():\n return 1\n"})).unwrap();
    let result = tx.dispatch("state_function", json!({"action":"declare", "namespace":"ns", "name":"f", "file":"/api.py", "symbol":"f"})).unwrap();
    let version = result["version"].as_str().unwrap().to_owned();
    tx.commit().unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let tx = store.begin().unwrap();
    let declaration = tx.function_declaration("ns", "f", Some(&version)).unwrap();
    assert_eq!(declaration.version, version);
    assert_eq!(declaration.source, "def f():\n return 1\n");
}

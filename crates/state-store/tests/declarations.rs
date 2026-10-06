mod support;
use serde_json::json;
use state_store::{FunctionDeclaration, Store};
use support::ToolCalls;

#[test]
fn unknown_declaration_fields_are_rejected() {
    let wire = json!({"name":"f", "file":"/api.py", "symbol":"f", "source":"def f(): pass", "source_hash":"hash", "abi_version":1, "version":"v", "unexpected":true});
    assert!(serde_json::from_value::<FunctionDeclaration>(wire).is_err());
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
    tx.tool("namespace.create", json!({ "name":"ns"})).unwrap();
    tx.tool(
        "fs.write",
        json!({ "namespace":"ns", "path":"/api.py", "text":"def f():\n return 1\n"}),
    )
    .unwrap();
    let result = tx
        .tool(
            "function.declare",
            json!({ "namespace":"ns", "name":"f", "file":"/api.py", "symbol":"f"}),
        )
        .unwrap();
    let version = result["version"].as_str().unwrap().to_owned();
    tx.commit_result().unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let tx = store.begin().unwrap();
    let declaration = tx.function_declaration("ns", "f", Some(&version)).unwrap();
    assert_eq!(declaration.version, version);
    assert_eq!(declaration.source, "def f():\n return 1\n");
}

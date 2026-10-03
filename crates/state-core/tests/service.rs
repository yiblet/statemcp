use serde_json::{Value, json};
use state_core::{CoreLimits, State};
use std::time::Duration;
use tempfile::TempDir;

fn service() -> (TempDir, State) {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(dir.path()).unwrap();
    (dir, state)
}
fn run(state: &State, tool: &str, args: Value) -> Value {
    state.dispatch(tool, args).unwrap()
}
fn namespace(state: &State, name: &str) {
    run(
        state,
        "state_namespace",
        json!({"action":"create","name":name}),
    );
}
fn write(state: &State, namespace: &str, path: &str, text: &str) {
    run(
        state,
        "state_fs",
        json!({"action":"write","namespace":namespace,"path":path,"text":text}),
    );
}
fn publish(state: &State, namespace: &str, name: &str, source: &str, extra: Value) -> Value {
    write(state, namespace, &format!("/{name}.py"), source);
    let mut declaration = json!({"action":"declare","namespace":namespace,"name":name,"file":format!("/{name}.py"),"symbol":"endpoint"});
    for (key, value) in extra.as_object().unwrap() {
        declaration[key] = value.clone();
    }
    run(state, "state_function", declaration)
}
fn call(state: &State, namespace: &str, function: &str, arguments: Value) -> Value {
    run(
        state,
        "state_call",
        json!({"namespace":namespace,"function":function,"arguments":arguments}),
    )
}
fn database(state: &State, namespace: &str) {
    run(
        state,
        "state_db",
        json!({"action":"create","namespace":namespace,"database":"app"}),
    );
    run(
        state,
        "state_db",
        json!({"action":"execute","namespace":namespace,"database":"app","sql":"CREATE TABLE items(id INTEGER PRIMARY KEY, text TEXT)"}),
    );
}
fn rows(state: &State, namespace: &str) -> Value {
    run(state, "state_db", json!({"action":"query","namespace":namespace,"database":"app","sql":"SELECT text FROM items ORDER BY id"}))["rows"].clone()
}

#[test]
fn create_migrate_publish_call_in_one_real_monty_script() {
    let (_dir, state) = service();
    let source = "def add(text):\n    return db_execute('app', 'INSERT INTO items(text) VALUES (?) RETURNING id, text', [text])\n";
    let result = run(
        &state,
        "state_execute",
        json!({"inputs":{"source":source},"script":r#"
mcp('state_namespace', {'action': 'create', 'name': 'notes'})
mcp('state_db', {'action': 'create', 'namespace': 'notes', 'database': 'app'})
mcp('state_db', {'action': 'migrate', 'namespace': 'notes', 'database': 'app', 'migrations': [{'id': 'initial', 'sql': 'CREATE TABLE items(id INTEGER PRIMARY KEY, text TEXT)'}]})
mcp('state_fs', {'action': 'write', 'namespace': 'notes', 'path': '/api.py', 'text': inputs['source']})
mcp('state_function', {'action': 'declare', 'namespace': 'notes', 'name': 'add', 'file': '/api.py', 'symbol': 'add', 'databases': {'app': 'write'}})
call('notes', 'add', {'text': 'bound; DROP TABLE items'})
"#}),
    );
    assert_eq!(
        result["value"]["rows"],
        json!([[1, "bound; DROP TABLE items"]])
    );
    assert_eq!(rows(&state, "notes"), json!([["bound; DROP TABLE items"]]));
    let schema = run(
        &state,
        "state_db",
        json!({"action":"inspect","namespace":"notes","database":"app"}),
    );
    assert_eq!(schema["tables"][0]["name"], "items");
    assert_eq!(schema["migrations"].as_array().unwrap().len(), 1);
}

#[test]
fn code_is_pinned_and_shared_db_bindings_follow_clones_and_renames() {
    let (dir, state) = service();
    namespace(&state, "a");
    database(&state, "a");
    publish(
        &state,
        "a",
        "add",
        "def endpoint(text):\n    return db_execute('app', 'INSERT INTO items(text) VALUES (?)', [text])\n",
        json!({"databases":{"app":"write"}}),
    );
    publish(
        &state,
        "a",
        "list",
        "def endpoint():\n    return db_query('app', 'SELECT text FROM items ORDER BY id')['rows']\n",
        json!({"databases":{"app":"read"}}),
    );
    call(&state, "a", "add", json!({"text":"original"}));
    write(
        &state,
        "a",
        "/list.py",
        "def endpoint():\n    return 'edited'\n",
    );
    assert_eq!(call(&state, "a", "list", json!({})), json!([["original"]]));
    let copied = run(
        &state,
        "state_namespace",
        json!({"action":"copy","namespace":"a","name":"b"}),
    );
    assert!(copied["revision"].is_string());
    call(&state, "b", "add", json!({"text":"fork"}));
    run(
        &state,
        "state_namespace",
        json!({"action":"update","namespace":"b","name":"renamed"}),
    );
    assert_eq!(
        call(&state, "renamed", "list", json!({})),
        json!([["original"], ["fork"]])
    );
    assert_eq!(call(&state, "a", "list", json!({})), json!([["original"]]));
    run(
        &state,
        "state_function",
        json!({"action":"update","namespace":"a","name":"list","file":"/list.py","symbol":"endpoint"}),
    );
    assert_eq!(call(&state, "a", "list", json!({})), "edited");
    drop(state);
    assert_eq!(
        rows(&State::open(dir.path()).unwrap(), "renamed"),
        json!([["original"], ["fork"]])
    );
}

#[test]
fn input_and_output_contracts_are_authoritative_before_publication() {
    let (_dir, state) = service();
    namespace(&state, "n");
    database(&state, "n");
    publish(
        &state,
        "n",
        "bad",
        "def endpoint(text):\n    db_execute('app', 'INSERT INTO items(text) VALUES (?)', [text])\n    return 123\n",
        json!({"databases":{"app":"write"},"input_schema":{"type":"object","required":["text"],"properties":{"text":{"type":"string"}},"additionalProperties":false},"output_schema":{"type":"string"}}),
    );
    for arguments in [json!({"text":3}), json!({"text":"writes then fails"})] {
        let error = state
            .dispatch(
                "state_call",
                json!({"namespace":"n","function":"bad","arguments":arguments}),
            )
            .unwrap_err();
        assert_eq!(error.code, "SCHEMA_VALIDATION");
        assert_eq!(rows(&state, "n"), json!([]));
    }
}

#[test]
fn module_initialization_is_effect_free_at_declare_and_every_invoke() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(
        &state,
        "n",
        "/bad.py",
        "write_text('/evil', 'x')\ndef endpoint():\n    return 1\n",
    );
    let error=state.dispatch("state_function",json!({"action":"declare","namespace":"n","name":"bad","file":"/bad.py","symbol":"endpoint","files":{"/":"write"}})).unwrap_err();
    assert_eq!(error.code, "PERMISSION_DENIED");
    assert!(
        state
            .dispatch(
                "state_fs",
                json!({"action":"read","namespace":"n","path":"/evil"})
            )
            .is_err()
    );
    let result = run(
        &state,
        "state_function",
        json!({"action":"list","namespace":"n"}),
    );
    assert_eq!(result["functions"], json!([]));
}

#[test]
fn cross_namespace_nested_failure_rolls_back_every_staged_effect() {
    let (_dir, state) = service();
    for n in ["a", "b"] {
        namespace(&state, n);
        database(&state, n);
    }
    publish(
        &state,
        "b",
        "bad",
        "def endpoint():\n    db_execute('app', \"INSERT INTO items(text) VALUES ('b')\")\n    return 1 / 0\n",
        json!({"databases":{"app":"write"}}),
    );
    publish(
        &state,
        "a",
        "outer",
        "def endpoint():\n    db_execute('app', \"INSERT INTO items(text) VALUES ('a')\")\n    return call('b', 'bad')\n",
        json!({"databases":{"app":"write"},"calls":[{"namespace":"b","function":"bad"}]}),
    );
    assert!(
        state
            .dispatch("state_call", json!({"namespace":"a","function":"outer"}))
            .is_err()
    );
    assert_eq!(rows(&state, "a"), json!([]));
    assert_eq!(rows(&state, "b"), json!([]));
    let error = state
        .dispatch(
            "state_execute",
            json!({"script":r#"
mcp('state_fs', {'action':'write', 'namespace':'a', 'path':'/staged', 'text':'x'})
try:
    call('a', 'outer')
except:
    pass
42
"#}),
        )
        .unwrap_err();
    assert!(!error.code.is_empty());
    assert!(
        state
            .dispatch(
                "state_fs",
                json!({"action":"read","namespace":"a","path":"/staged"})
            )
            .is_err()
    );
}

#[test]
fn invoke_does_not_grant_raw_access_and_mcp_cannot_bypass_helper_policy() {
    let (_dir, state) = service();
    namespace(&state, "n");
    database(&state, "n");
    publish(
        &state,
        "n",
        "writer",
        "def endpoint():\n    return db_execute('app', \"INSERT INTO items(text) VALUES ('x')\")\n",
        json!({"databases":{"app":"write"}}),
    );
    publish(
        &state,
        "n",
        "delegator",
        "def endpoint():\n    call('self', 'writer')\n    return mcp('state_db', {'action':'query', 'namespace':'self', 'database':'app', 'sql':'SELECT * FROM items'})\n",
        json!({"calls":[{"namespace":"self","function":"writer"}]}),
    );
    assert_eq!(
        state
            .dispatch(
                "state_call",
                json!({"namespace":"n","function":"delegator"})
            )
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert_eq!(rows(&state, "n"), json!([]));
    publish(
        &state,
        "n",
        "reader",
        "def endpoint():\n    return db_execute('app', \"INSERT INTO items(text) VALUES ('x')\")\n",
        json!({"databases":{"app":"read"}}),
    );
    assert_eq!(
        state
            .dispatch("state_call", json!({"namespace":"n","function":"reader"}))
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    publish(
        &state,
        "n",
        "escape",
        "def endpoint():\n    return mcp('state_execute', {'script':'42'})\n",
        json!({}),
    );
    assert_eq!(
        state
            .dispatch("state_call", json!({"namespace":"n","function":"escape"}))
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
}

#[test]
fn file_grants_normalize_paths_and_respect_component_boundaries() {
    let (_dir, state) = service();
    namespace(&state, "n");
    publish(
        &state,
        "n",
        "files",
        "def endpoint(path):\n    write_text(path, 'ok')\n    return read_text(path)\n",
        json!({"files":{"/allowed/./":"write"}}),
    );
    assert_eq!(
        call(&state, "n", "files", json!({"path":"/allowed//child"})),
        "ok"
    );
    for path in ["/allowedness/child", "/other", "/allowed/../other"] {
        assert!(
            state
                .dispatch(
                    "state_call",
                    json!({"namespace":"n","function":"files","arguments":{"path":path}})
                )
                .is_err()
        );
    }
}

#[test]
fn namespace_and_callee_bindings_survive_external_rename_and_self_clone() {
    let (_dir, state) = service();
    namespace(&state, "a");
    namespace(&state, "b");
    publish(
        &state,
        "b",
        "constant",
        "def endpoint():\n    return 8\n",
        json!({}),
    );
    let external_id = run(
        &state,
        "state_namespace",
        json!({"action":"get","namespace":"b"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    publish(
        &state,
        "a",
        "outer",
        &format!("def endpoint():\n    return call('{external_id}', 'constant')\n"),
        json!({"calls":[{"namespace":"b","function":"constant"}]}),
    );
    run(
        &state,
        "state_namespace",
        json!({"action":"update","namespace":"b","name":"c"}),
    );
    assert_eq!(call(&state, "a", "outer", json!({})), 8);
    publish(
        &state,
        "a",
        "selfcall",
        "def endpoint():\n    return call('self', 'outer')\n",
        json!({"calls":[{"namespace":"self","function":"outer"}]}),
    );
    run(
        &state,
        "state_namespace",
        json!({"action":"copy","namespace":"a","name":"fork"}),
    );
    assert_eq!(call(&state, "fork", "selfcall", json!({})), 8);
}

#[test]
fn schemas_reject_external_references_and_support_local_defs() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(
        &state,
        "n",
        "/api.py",
        "def endpoint(value):\n    return value\n",
    );
    for schema in [
        json!({"$ref":"file:///etc/passwd"}),
        json!({"$schema":"https://example.invalid/meta"}),
        json!({"const":{"$ref":"https://example.invalid/indirect"},"$ref":"#/const"}),
        json!({"$ref":"https://example.invalid/schema"}),
        json!({"$dynamicRef":"file:///tmp/schema"}),
        json!({"$recursiveRef":"remote.json"}),
        json!({"$id":"https://example.invalid/"}),
        json!({"type":123}),
    ] {
        assert_eq!(state.dispatch("state_function",json!({"action":"declare","namespace":"n","name":"test","file":"/api.py","symbol":"endpoint","input_schema":schema})).unwrap_err().code,"INVALID_ARGUMENT");
    }
    run(
        &state,
        "state_function",
        json!({"action":"declare","namespace":"n","name":"test","file":"/api.py","symbol":"endpoint","input_schema":{"type":"object","properties":{"value":{"$ref":"#/$defs/text"}},"$defs":{"text":{"type":"string"}}}}),
    );
    assert_eq!(call(&state, "n", "test", json!({"value":"good"})), "good");
    assert!(
        state
            .dispatch(
                "state_call",
                json!({"namespace":"n","function":"test","arguments":{"value":4}})
            )
            .is_err()
    );
}

#[test]
fn strict_tool_and_host_validation_and_discovery_share_the_same_surface() {
    let (_dir, state) = service();
    namespace(&state, "n");
    for (tool, args) in [
        (
            "state_namespace",
            json!({"action":"create","name":"oops","typo":true}),
        ),
        (
            "state_db",
            json!({"action":"delete","namespace":"n","database":"app"}),
        ),
        ("state_execute", json!({"source":"42"})),
        (
            "state_call",
            json!({"namespace":"n","function":"missing","arguments":[]}),
        ),
        (
            "state_fs",
            json!({"action":"write","namespace":"n","path":"/bad","text":"x","base64":"eA=="}),
        ),
    ] {
        assert_eq!(
            state.dispatch(tool, args).unwrap_err().code,
            "INVALID_ARGUMENT"
        );
    }
    for script in [
        "mcp('state_namespace', {'action':'list'}, extra=1)",
        "mcp('state_namespace', name='state_db')",
        "mcp('state_namespace', {'action':'list','extra':1})",
        "db_query('app', 'SELECT 1')",
    ] {
        assert_eq!(
            state
                .dispatch("state_execute", json!({"script":script}))
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
    }
    let result = run(
        &state,
        "state_execute",
        json!({"script":"mcp('state_describe')['tools']"}),
    );
    assert_eq!(result["value"].as_array().unwrap().len(), 7);
    assert_eq!(state_core::tool_definitions().len(), 7);
}

#[test]
fn shared_depth_and_operation_budgets_abort_recursion_and_loops() {
    let (_dir, state) = service();
    namespace(&state, "n");
    publish(
        &state,
        "n",
        "recursive",
        "def endpoint():\n    write_text('/staged', 'x')\n    return call('self', 'recursive')\n",
        json!({"files":{"/staged":"write"},"calls":[{"namespace":"self","function":"recursive"}]}),
    );
    let limited = state.clone().with_limits(CoreLimits {
        max_depth: 3,
        ..CoreLimits::default()
    });
    assert_eq!(
        limited
            .dispatch(
                "state_call",
                json!({"namespace":"n","function":"recursive"})
            )
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
    assert!(
        state
            .dispatch(
                "state_fs",
                json!({"action":"read","namespace":"n","path":"/staged"})
            )
            .is_err()
    );
    let mut limits = CoreLimits::default();
    limits.runtime.max_calls = 4;
    let limited = state.clone().with_limits(limits);
    assert_eq!(limited.dispatch("state_execute",json!({"namespace":"n","script":"for i in range(10):\n    write_text('/loop', 'x')\n"})).unwrap_err().code,"LIMIT_EXCEEDED");
    assert!(
        state
            .dispatch(
                "state_fs",
                json!({"action":"read","namespace":"n","path":"/loop"})
            )
            .is_err()
    );
    let mut limits = CoreLimits::default();
    limits.runtime.max_duration = Duration::from_millis(10);
    assert_eq!(
        state
            .with_limits(limits)
            .dispatch("state_execute", json!({"script":"while True:\n    pass"}))
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
}

#[test]
fn receipt_replays_exact_result_and_scopes_keys_by_trusted_principal() {
    let (_dir, state) = service();
    namespace(&state, "n");
    database(&state, "n");
    let args = json!({"namespace":"n","idempotency_key":"k","script":"db_execute('app', \"INSERT INTO items(text) VALUES ('once') RETURNING id\")"});
    let result = run(&state, "state_execute", args.clone());
    assert_eq!(run(&state, "state_execute", args.clone()), result);
    assert_eq!(rows(&state, "n"), json!([["once"]]));
    let mut changed = args.clone();
    changed["script"] = json!("42");
    assert_eq!(
        state.dispatch("state_execute", changed).unwrap_err().code,
        "IDEMPOTENCY_MISMATCH"
    );
    state.dispatch_as("other", "state_execute", args).unwrap();
    assert_eq!(rows(&state, "n"), json!([["once"], ["once"]]));
    assert_eq!(state.dispatch("state_execute",json!({"script":"mcp('state_execute', {'script':'42', 'idempotency_key':'nested'})"})).unwrap_err().code,"INVALID_ARGUMENT");
}

#[test]
fn direct_large_files_remain_readable_and_runtime_writes_respect_bridge_limits() {
    let (_dir, state) = service();
    namespace(&state, "n");
    let text = "x".repeat(2 * 1024 * 1024);
    write(&state, "n", "/large", &text);
    assert_eq!(
        run(
            &state,
            "state_fs",
            json!({"action":"read","namespace":"n","path":"/large"})
        )["text"],
        text
    );
    assert_eq!(
        state
            .dispatch(
                "state_execute",
                json!({"namespace":"n","script":"read_text('/large')"})
            )
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
    let mut limits = CoreLimits::default();
    limits.runtime.max_output_bytes = 1024;
    let limited = state.clone().with_limits(limits);
    assert_eq!(
        limited
            .dispatch(
                "state_execute",
                json!({"namespace":"n","script":"write_text('/escaped', '\\x00' * 200)"})
            )
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
    assert!(
        state
            .dispatch(
                "state_fs",
                json!({"action":"read","namespace":"n","path":"/escaped"})
            )
            .is_err()
    );
}

#[test]
fn migration_grants_introspection_and_version_checks_are_enforced() {
    let (_dir, state) = service();
    namespace(&state, "n");
    database(&state, "n");
    let declaration = publish(
        &state,
        "n",
        "migration",
        r#"def endpoint():
    mcp('state_db', {'action':'migrate', 'namespace':'self', 'database':'app', 'migrations':[{'id':'extend','sql':'CREATE TABLE more(value TEXT)'}]})
    return db_inspect('app')
"#,
        json!({"databases":{"app":"migrate"}}),
    );
    let schema = call(&state, "n", "migration", json!({}));
    assert_eq!(schema["tables"].as_array().unwrap().len(), 2);
    assert_eq!(schema["migrations"].as_array().unwrap().len(), 1);
    let stale = state
        .dispatch(
            "state_call",
            json!({"namespace":"n","function":"migration","expected_version":"stale"}),
        )
        .unwrap_err();
    assert_eq!(stale.code, "CONFLICT");
    run(
        &state,
        "state_call",
        json!({"namespace":"n","function":"migration","expected_version":declaration["version"]}),
    );
    publish(
        &state,
        "n",
        "denied",
        r#"def endpoint():
    return mcp('state_db', {'action':'migrate', 'namespace':'self', 'database':'app', 'migrations':[]})
"#,
        json!({"databases":{"app":"write"}}),
    );
    assert_eq!(
        state
            .dispatch("state_call", json!({"namespace":"n","function":"denied"}))
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
}

#[test]
fn schema_instance_data_and_property_names_are_not_reference_keywords() {
    let (_dir, state) = service();
    namespace(&state, "n");
    publish(
        &state,
        "n",
        "echo",
        "def endpoint(value):\n    return value\n",
        json!({"input_schema":{"type":"object","properties":{"value":{"type":"object","properties":{"$ref":{"type":"string"},"$id":{"type":"string"}},"const":{"$ref":"https://example.invalid/", "$id":"literal"}}}}}),
    );
    let literal = json!({"$ref":"https://example.invalid/", "$id":"literal"});
    assert_eq!(call(&state, "n", "echo", json!({"value":literal})), literal);
}

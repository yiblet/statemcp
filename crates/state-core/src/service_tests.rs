use crate::test_support as support;
use crate::{Controller, CoreLimits};
use serde_json::{Value, json};
use state_fs::MemoryFs;
use std::time::Duration;
use support::ToolCalls;

fn service() -> (MemoryFs, Controller) {
    let dir = MemoryFs::new();
    let state = support::memory_controller(&dir).unwrap();
    (dir, state)
}
fn run(state: &Controller, tool: &str, args: Value) -> Value {
    state.tool(tool, args).unwrap()
}
fn namespace(state: &Controller, name: &str) {
    run(state, "namespace.create", json!({"name":name}));
}
fn write(state: &Controller, namespace: &str, path: &str, text: &str) {
    run(
        state,
        "fs.write",
        json!({"namespace":namespace,"path":path,"text":text}),
    );
}
fn publish(state: &Controller, namespace: &str, name: &str, source: &str, extra: Value) -> Value {
    write(state, namespace, &format!("/{name}.py"), source);
    let mut declaration =
        json!({"namespace":namespace,"name":name,"file":format!("/{name}.py"),"symbol":"endpoint"});
    for (key, value) in extra.as_object().unwrap() {
        declaration[key] = value.clone();
    }
    run(state, "function.declare", declaration)
}
fn call(state: &Controller, namespace: &str, function: &str, arguments: Value) -> Value {
    run(
        state,
        "call",
        json!({"namespace":namespace,"function":function,"arguments":arguments}),
    )
}
fn database(state: &Controller, namespace: &str) {
    run(
        state,
        "db.create",
        json!({"namespace":namespace,"database":"app"}),
    );
    run(
        state,
        "db.execute",
        json!({"namespace":namespace,"database":"app","sql":"CREATE TABLE items(id INTEGER PRIMARY KEY, text TEXT)"}),
    );
}
fn rows(state: &Controller, namespace: &str) -> Value {
    run(
        state,
        "db.query",
        json!({"namespace":namespace,"database":"app","sql":"SELECT text FROM items ORDER BY id"}),
    )["rows"]
        .clone()
}

#[test]
fn create_migrate_publish_call_in_one_real_monty_script() {
    let (_dir, state) = service();
    let source = "def add(text):\n    return db_execute('app', 'INSERT INTO items(text) VALUES (?) RETURNING id, text', [text])\n";
    let result = run(
        &state,
        "execute",
        json!({"inputs":{"source":source},"script":r#"
mcp('namespace.create', { 'name': 'notes'})
mcp('db.create', { 'namespace': 'notes', 'database': 'app'})
mcp('db.migrate', { 'namespace': 'notes', 'database': 'app', 'migrations': [{'id': 'initial', 'sql': 'CREATE TABLE items(id INTEGER PRIMARY KEY, text TEXT)'}]})
mcp('fs.write', { 'namespace': 'notes', 'path': '/api.py', 'text': inputs['source']})
mcp('function.declare', { 'namespace': 'notes', 'name': 'add', 'file': '/api.py', 'symbol': 'add'})
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
        "db.inspect",
        json!({"namespace":"notes","database":"app"}),
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
        json!({}),
    );
    publish(
        &state,
        "a",
        "list",
        "def endpoint():\n    return db_query('app', 'SELECT text FROM items ORDER BY id')['rows']\n",
        json!({}),
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
        "namespace.copy",
        json!({"namespace":"a","name":"b"}),
    );
    assert!(copied["revision"].is_string());
    call(&state, "b", "add", json!({"text":"fork"}));
    run(
        &state,
        "namespace.update",
        json!({"namespace":"b","name":"renamed"}),
    );
    assert_eq!(
        call(&state, "renamed", "list", json!({})),
        json!([["original"], ["fork"]])
    );
    assert_eq!(call(&state, "a", "list", json!({})), json!([["original"]]));
    run(
        &state,
        "function.update",
        json!({"namespace":"a","name":"list","file":"/list.py","symbol":"endpoint"}),
    );
    assert_eq!(call(&state, "a", "list", json!({})), "edited");
    drop(state);
    assert_eq!(
        rows(&support::memory_controller(&dir).unwrap(), "renamed"),
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
        json!({"input_schema":{"type":"object","required":["text"],"properties":{"text":{"type":"string"}},"additionalProperties":false},"output_schema":{"type":"string"}}),
    );
    for arguments in [json!({"text":3}), json!({"text":"writes then fails"})] {
        let error = state
            .tool(
                "call",
                json!({"namespace":"n","function":"bad","arguments":arguments}),
            )
            .unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::SchemaValidation);
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
    let error = state
        .tool(
            "function.declare",
            json!({"namespace":"n","name":"bad","file":"/bad.py","symbol":"endpoint"}),
        )
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::PermissionDenied);
    assert!(
        state
            .tool("fs.read", json!({"namespace":"n","path":"/evil"}))
            .is_err()
    );
    let result = run(&state, "function.list", json!({"namespace":"n"}));
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
        json!({}),
    );
    publish(
        &state,
        "a",
        "outer",
        "def endpoint():\n    db_execute('app', \"INSERT INTO items(text) VALUES ('a')\")\n    return call('b', 'bad')\n",
        json!({}),
    );
    assert!(
        state
            .tool("call", json!({"namespace":"a","function":"outer"}))
            .is_err()
    );
    assert_eq!(rows(&state, "a"), json!([]));
    assert_eq!(rows(&state, "b"), json!([]));
    let error = state
        .tool(
            "execute",
            json!({"script":r#"
mcp('fs.write', { 'namespace':'a', 'path':'/staged', 'text':'x'})
try:
    call('a', 'outer')
except:
    pass
42
"#}),
        )
        .unwrap_err();
    assert!(!error.message.is_empty());
    assert!(
        state
            .tool("fs.read", json!({"namespace":"a","path":"/staged"}))
            .is_err()
    );
}

#[test]
fn same_namespace_calls_and_nested_scripts_have_full_access() {
    let (_dir, state) = service();
    namespace(&state, "n");
    database(&state, "n");
    publish(
        &state,
        "n",
        "writer",
        "def endpoint():\n    return db_execute('app', \"INSERT INTO items(text) VALUES ('x')\")\n",
        json!({}),
    );
    publish(
        &state,
        "n",
        "delegator",
        "def endpoint():\n    call('self', 'writer')\n    return mcp('execute', {'script':\"db_query('app', 'SELECT * FROM items')\"})\n",
        json!({}),
    );
    assert_eq!(
        call(&state, "n", "delegator", json!({}))["value"]["rows"],
        json!([[1, "x"]])
    );
}

#[test]
fn namespace_files_allow_all_paths_and_reject_traversal() {
    let (_dir, state) = service();
    namespace(&state, "n");
    publish(
        &state,
        "n",
        "files",
        "def endpoint(path):\n    write_text(path, 'ok')\n    return read_text(path)\n",
        json!({}),
    );
    for path in ["/allowed//child", "/allowedness/child", "/other"] {
        assert_eq!(call(&state, "n", "files", json!({"path":path})), "ok");
    }
    assert!(
        state
            .tool(
                "call",
                json!({"namespace":"n","function":"files","arguments":{"path":"/allowed/../other"}})
            )
            .is_err()
    );
}

#[test]
fn namespace_scope_follows_renames_and_clones_and_blocks_external_ids() {
    let (_dir, state) = service();
    namespace(&state, "a");
    namespace(&state, "b");
    publish(
        &state,
        "a",
        "constant",
        "def endpoint():\n    return 8\n",
        json!({}),
    );
    publish(
        &state,
        "b",
        "constant",
        "def endpoint():\n    return 9\n",
        json!({}),
    );
    let external_id = run(&state, "namespace.get", json!({"namespace":"b"}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    publish(
        &state,
        "a",
        "outer",
        "def endpoint():\n    return call('self', 'constant')\n",
        json!({}),
    );
    publish(
        &state,
        "a",
        "escape",
        &format!("def endpoint():\n    return call('{external_id}', 'constant')\n"),
        json!({}),
    );
    run(
        &state,
        "namespace.update",
        json!({"namespace":"a","name":"renamed"}),
    );
    run(
        &state,
        "namespace.update",
        json!({"namespace":"b","name":"external"}),
    );
    run(
        &state,
        "namespace.copy",
        json!({"namespace":"renamed","name":"fork"}),
    );
    for ns in ["renamed", "fork"] {
        assert_eq!(call(&state, ns, "outer", json!({})), 8);
        assert_eq!(
            state
                .tool("call", json!({"namespace":ns,"function":"escape"}))
                .unwrap_err()
                .code,
            crate::ErrorCode::PermissionDenied
        );
    }
}

#[test]
fn functions_cannot_escape_through_mcp_or_nested_scripts() {
    let (_dir, state) = service();
    namespace(&state, "a");
    namespace(&state, "b");
    write(&state, "b", "/secret", "secret");
    for (name, source) in [
        (
            "read",
            "def endpoint():\n    return mcp('fs.read', {'namespace':'b', 'path':'/secret'})\n",
        ),
        (
            "nested",
            "def endpoint():\n    return mcp('execute', {'script':\"mcp('fs.read', {'namespace':'b', 'path':'/secret'})\"})\n",
        ),
        (
            "explicit",
            "def endpoint():\n    return mcp('execute', {'namespace':'b', 'script':'42'})\n",
        ),
        (
            "list",
            "def endpoint():\n    return mcp('namespace.list', {})\n",
        ),
        (
            "create",
            "def endpoint():\n    return mcp('namespace.create', {'name':'escape'})\n",
        ),
        (
            "copy",
            "def endpoint():\n    return mcp('namespace.copy', {'namespace':'self', 'name':'escape'})\n",
        ),
    ] {
        publish(&state, "a", name, source, json!({}));
        assert_eq!(
            state
                .tool("call", json!({"namespace":"a","function":name}))
                .unwrap_err()
                .code,
            crate::ErrorCode::PermissionDenied,
            "{name}"
        );
    }
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
        assert_eq!(state.tool("function.declare",json!({"namespace":"n","name":"test","file":"/api.py","symbol":"endpoint","input_schema":schema})).unwrap_err().code,crate::ErrorCode::InvalidArgument);
    }
    run(
        &state,
        "function.declare",
        json!({"namespace":"n","name":"test","file":"/api.py","symbol":"endpoint","input_schema":{"type":"object","properties":{"value":{"$ref":"#/$defs/text"}},"$defs":{"text":{"type":"string"}}}}),
    );
    assert_eq!(call(&state, "n", "test", json!({"value":"good"})), "good");
    assert!(
        state
            .tool(
                "call",
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
        ("namespace.create", json!({"name":"oops","typo":true})),
        ("db.delete", json!({"namespace":"n","database":"app"})),
        ("execute", json!({"source":"42"})),
        (
            "call",
            json!({"namespace":"n","function":"missing","arguments":[]}),
        ),
        (
            "fs.write",
            json!({"namespace":"n","path":"/bad","text":"x","base64":"eA=="}),
        ),
    ] {
        assert_eq!(
            state.tool(tool, args).unwrap_err().code,
            crate::ErrorCode::InvalidArgument
        );
    }
    for script in [
        "mcp('namespace.list', {}, extra=1)",
        "mcp('namespace.create', name='db.query')",
        "mcp('namespace.list', {'extra':1})",
        "db_query('app', 'SELECT 1')",
    ] {
        assert_eq!(
            state
                .tool("execute", json!({"script":script}))
                .unwrap_err()
                .code,
            crate::ErrorCode::InvalidArgument
        );
    }
    let result = run(
        &state,
        "execute",
        json!({"script":"mcp('describe')['tools']"}),
    );
    assert_eq!(result["value"].as_array().unwrap().len(), 30);
    assert_eq!(crate::tool_definitions().len(), 30);
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
        json!({}),
    );
    let limited = state.clone().with_limits(CoreLimits {
        max_depth: 3,
        ..CoreLimits::default()
    });
    assert_eq!(
        limited
            .tool("call", json!({"namespace":"n","function":"recursive"}))
            .unwrap_err()
            .code,
        crate::ErrorCode::LimitExceeded
    );
    assert!(
        state
            .tool("fs.read", json!({"namespace":"n","path":"/staged"}))
            .is_err()
    );
    let mut limits = CoreLimits::default();
    limits.runtime.max_calls = 4;
    let limited = state.clone().with_limits(limits);
    assert_eq!(limited.tool("execute",json!({"namespace":"n","script":"for i in range(10):\n    write_text('/loop', 'x')\n"})).unwrap_err().code,crate::ErrorCode::LimitExceeded);
    assert!(
        state
            .tool("fs.read", json!({"namespace":"n","path":"/loop"}))
            .is_err()
    );
    let mut limits = CoreLimits::default();
    limits.runtime.max_duration = Duration::from_millis(10);
    assert_eq!(
        state
            .with_limits(limits)
            .tool("execute", json!({"script":"while True:\n    pass"}))
            .unwrap_err()
            .code,
        crate::ErrorCode::LimitExceeded
    );
}

#[test]
fn receipt_replays_exact_result_and_scopes_keys_by_trusted_principal() {
    let (_dir, state) = service();
    namespace(&state, "n");
    database(&state, "n");
    let args = json!({"namespace":"n","idempotency_key":"k","script":"db_execute('app', \"INSERT INTO items(text) VALUES ('once') RETURNING id\")"});
    let result = run(&state, "execute", args.clone());
    assert_eq!(run(&state, "execute", args.clone()), result);
    let input = serde_json::from_value(args.clone()).unwrap();
    let identity = crate::ReceiptIdentity::for_arguments("k", "execute", &args).unwrap();
    assert_eq!(
        serde_json::to_value(state.execute_script(input, Some(identity)).unwrap()).unwrap(),
        result
    );
    assert_eq!(rows(&state, "n"), json!([["once"]]));
    let mut changed = args.clone();
    changed["script"] = json!("42");
    assert_eq!(
        state.tool("execute", changed).unwrap_err().code,
        crate::ErrorCode::IdempotencyMismatch
    );
    state
        .clone()
        .with_principal("other")
        .unwrap()
        .tool("execute", args)
        .unwrap();
    assert_eq!(rows(&state, "n"), json!([["once"], ["once"]]));
    assert_eq!(
        state
            .tool(
                "execute",
                json!({"script":"mcp('execute', {'script':'42', 'idempotency_key':'nested'})"})
            )
            .unwrap_err()
            .code,
        crate::ErrorCode::InvalidArgument
    );
}

#[test]
fn direct_large_files_remain_readable_and_runtime_writes_respect_bridge_limits() {
    let (_dir, state) = service();
    namespace(&state, "n");
    let text = "x".repeat(2 * 1024 * 1024);
    write(&state, "n", "/large", &text);
    assert_eq!(
        run(&state, "fs.read", json!({"namespace":"n","path":"/large"}))["text"],
        text
    );
    assert_eq!(
        state
            .tool(
                "execute",
                json!({"namespace":"n","script":"read_text('/large')"})
            )
            .unwrap_err()
            .code,
        crate::ErrorCode::LimitExceeded
    );
    let mut limits = CoreLimits::default();
    limits.runtime.max_output_bytes = 1024;
    let limited = state.clone().with_limits(limits);
    assert_eq!(
        limited
            .tool(
                "execute",
                json!({"namespace":"n","script":"write_text('/escaped', '\\x00' * 200)"})
            )
            .unwrap_err()
            .code,
        crate::ErrorCode::LimitExceeded
    );
    assert!(
        state
            .tool("fs.read", json!({"namespace":"n","path":"/escaped"}))
            .is_err()
    );
}

#[test]
fn migration_introspection_and_version_checks_are_enforced() {
    let (_dir, state) = service();
    namespace(&state, "n");
    database(&state, "n");
    let declaration = publish(
        &state,
        "n",
        "migration",
        r#"def endpoint():
    mcp('db.migrate', { 'namespace':'self', 'database':'app', 'migrations':[{'id':'extend','sql':'CREATE TABLE more(value TEXT)'}]})
    return db_inspect('app')
"#,
        json!({}),
    );
    let schema = call(&state, "n", "migration", json!({}));
    assert_eq!(schema["tables"].as_array().unwrap().len(), 2);
    assert_eq!(schema["migrations"].as_array().unwrap().len(), 1);
    let stale = state
        .tool(
            "call",
            json!({"namespace":"n","function":"migration","expected_version":"stale"}),
        )
        .unwrap_err();
    assert_eq!(stale.code, crate::ErrorCode::Conflict);
    run(
        &state,
        "call",
        json!({"namespace":"n","function":"migration","expected_version":declaration["version"]}),
    );
    publish(
        &state,
        "n",
        "denied",
        r#"def endpoint():
    return mcp('db.migrate', { 'namespace':'self', 'database':'app', 'migrations':[]})
"#,
        json!({}),
    );
    assert_eq!(call(&state, "n", "denied", json!({}))["applied"], 0);
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

#[test]
fn discovery_guide_runs_and_acknowledgments_keep_actionable_tokens() {
    let (_dir, state) = service();
    let overview = run(&state, "describe", json!({}));
    assert_eq!(overview["tools"].as_array().unwrap().len(), 30);
    assert!(overview["tools"][0].get("inputSchema").is_none());
    let full = run(&state, "describe", json!({"mode":"full"}));
    assert!(full["tools"][0]["inputSchema"].is_object());
    let focused = run(&state, "describe", json!({"tool":"db.create"}));
    assert_eq!(focused["tool"]["name"], "db.create");
    for args in [
        json!({"mode":"unknown"}),
        json!({"action":"create"}),
        json!({"tool":"unknown"}),
        json!({"tool":"db.query","action":"unknown"}),
        json!({"function":"add"}),
    ] {
        assert_eq!(
            state.tool("describe", args).unwrap_err().code,
            crate::ErrorCode::InvalidArgument
        );
    }
    let guide = run(&state, "describe", json!({"mode":"runtime"}));
    let mut version = Value::Null;
    for step in guide["example"].as_array().unwrap() {
        let tool = step["tool"].as_str().unwrap();
        let result = run(&state, tool, step["arguments"].clone());
        if tool == "db.create" {
            assert!(result.get("snapshot").is_none());
            assert!(result.get("id").is_none());
            assert!(result["revision"].is_string());
        }
        if tool == "function.declare" {
            assert!(result.get("source").is_none());
            assert!(result.get("abi_version").is_none());
            assert!(result["revision"].is_string());
            version = result["version"].clone();
        }
        if tool == "call" {
            assert_eq!(result, json!([1, "hello"]));
        }
    }
    let detail = run(
        &state,
        "function.get",
        json!({"namespace":"demo","name":"add","expected_version":version}),
    );
    assert!(detail["source"].is_string());
    assert!(detail["source_hash"].is_string());
    assert_eq!(detail["abi_version"], 1);
    let listing = run(&state, "function.list", json!({"namespace":"demo"}));
    assert!(listing["functions"][0].get("source").is_none());
    let inspection = run(
        &state,
        "db.inspect",
        json!({"namespace":"demo","database":"app"}),
    );
    assert!(inspection["snapshot"].is_string());
    assert_eq!(inspection["staged"], false);
    let staged = run(
        &state,
        "execute",
        json!({"namespace":"demo","script":"db_execute('app', \"INSERT INTO notes(text) VALUES ('pending')\")\ndb_inspect('app')"}),
    );
    assert_eq!(staged["value"]["staged"], true);
    assert_eq!(staged["value"]["snapshot"], inspection["snapshot"]);
}

#[test]
fn dotted_tools_imply_actions_and_keep_namespace_scope() {
    let (_dir, state) = service();
    run(&state, "namespace.create", json!({"name":"n"}));
    run(
        &state,
        "db.create",
        json!({"namespace":"n","database":"app"}),
    );
    assert_eq!(
        state
            .tool(
                "db.query",
                json!({"namespace":"n","database":"app","sql":"SELECT 1","action":"execute"})
            )
            .unwrap_err()
            .code,
        crate::ErrorCode::InvalidArgument
    );
    let result = run(
        &state,
        "execute",
        json!({"namespace":"n","script":"mcp('db.query', {'namespace':'n', 'database':'app', 'sql':'SELECT 42'})"}),
    );
    assert_eq!(result["value"]["rows"], json!([[42]]));
    publish(
        &state,
        "n",
        "denied",
        "def endpoint():\n    return mcp('db.query', {'namespace':'self', 'database':'app', 'sql':'SELECT 1'})\n",
        json!({}),
    );
    assert_eq!(call(&state, "n", "denied", json!({}))["rows"], json!([[1]]));
}

#[test]
fn discovery_includes_bundled_readme_and_public_validation_errors() {
    let (_dir, state) = service();
    let overview = run(&state, "describe", json!({}));
    assert_eq!(overview["discovery"]["readme"], json!({"mode":"readme"}));
    let readme = run(&state, "describe", json!({"mode":"readme"}));
    assert_eq!(readme["format"], "markdown");
    assert_eq!(readme["text"], include_str!("../../../README.md"));
    let error = state
        .tool("db.query", json!({"namespace":"n"}))
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::InvalidArgument);
    assert!(error.message.contains("database"));
    assert!(!error.message.contains("db.query"));
    assert!(!error.message.contains("oneOf"));
    namespace(&state, "n");
    let written = run(
        &state,
        "fs.write",
        json!({"namespace":"n","path":"/text","text":"hello"}),
    );
    assert!(written.get("hash").is_none());
    let stat = run(&state, "fs.stat", json!({"namespace":"n","path":"/text"}));
    assert!(stat["hash"].is_string());
}

#[test]
fn rename_invalidates_namespace_revision() {
    let (_dir, state) = service();
    let created = run(&state, "namespace.create", json!({"name":"original"}));
    let renamed = run(
        &state,
        "namespace.update",
        json!({"namespace":created["id"],"name":"renamed","expected_revision":created["revision"]}),
    );
    assert_eq!(created["id"], renamed["id"]);
    assert_ne!(created["revision"], renamed["revision"]);
    assert_eq!(state.tool("namespace.update", json!({"namespace":created["id"],"name":"stale","expected_revision":created["revision"]})).unwrap_err().code, crate::ErrorCode::Conflict);
}

#[test]
fn obsolete_grant_fields_are_rejected_before_publication() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(&state, "n", "/api.py", "def endpoint():\n    return 1\n");
    for field in ["databases", "files", "calls"] {
        let mut args =
            json!({"namespace":"n","name":"invalid","file":"/api.py","symbol":"endpoint"});
        args[field] = json!([]);
        assert_eq!(
            state.tool("function.declare", args).unwrap_err().code,
            crate::ErrorCode::InvalidArgument
        );
    }
    assert_eq!(
        run(&state, "function.list", json!({"namespace":"n"}))["functions"],
        json!([])
    );
    publish(
        &state,
        "n",
        "valid",
        "def endpoint():\n    return 1\n",
        json!({}),
    );
    let detail = run(
        &state,
        "function.get",
        json!({"namespace":"n","name":"valid"}),
    );
    for field in ["databases", "files", "calls", "database_ids"] {
        assert!(detail.get(field).is_none());
    }
    assert_eq!(call(&state, "n", "valid", json!({})), 1);
}

#[test]
fn functions_can_build_tools_and_manage_databases_in_their_namespace() {
    let (_dir, state) = service();
    namespace(&state, "n");
    publish(
        &state,
        "n",
        "factory",
        r#"def endpoint():
    mcp('db.create', {'namespace':'self', 'database':'new'})
    db_execute('new', 'CREATE TABLE items(value INTEGER)')
    db_execute('new', 'INSERT INTO items VALUES (42)')
    write_text('/helper.py', "def helper():\n    return db_query('new', 'SELECT value FROM items')['rows']\n")
    mcp('function.declare', {'namespace':'self', 'name':'helper', 'file':'/helper.py', 'symbol':'helper'})
    result = call('self', 'helper')
    mcp('db.drop', {'namespace':'self', 'database':'new'})
    return result
"#,
        json!({}),
    );
    assert_eq!(call(&state, "n", "factory", json!({})), json!([[42]]));
    assert_eq!(
        run(&state, "db.list", json!({"namespace":"n"}))["databases"],
        json!([])
    );
    assert!(
        run(
            &state,
            "function.get",
            json!({"namespace":"n","name":"helper"})
        )["source"]
            .is_string()
    );
}

#[test]
fn sibling_imports_share_module_globals_and_pin_sources() {
    let (dir, state) = service();
    namespace(&state, "n");
    write(
        &state,
        "n",
        "/helpers.py",
        "count = 0\ndef increment():\n    global count\n    count += 1\n    return count\n",
    );
    let published = publish(
        &state,
        "n",
        "api",
        "import helpers\nimport helpers as h\nfrom helpers import increment as inc\ndef endpoint():\n    return [inc(), h.increment(), helpers.count, h is helpers]\n",
        json!({}),
    );
    assert_eq!(call(&state, "n", "api", json!({})), json!([1, 2, 2, true]));
    write(
        &state,
        "n",
        "/helpers.py",
        "count = 40\ndef increment():\n    global count\n    count += 1\n    return count\n",
    );
    assert_eq!(call(&state, "n", "api", json!({})), json!([1, 2, 2, true]));
    let updated = run(
        &state,
        "function.update",
        json!({"namespace":"n","name":"api","file":"/api.py","symbol":"endpoint"}),
    );
    assert_ne!(published["version"], updated["version"]);
    assert_eq!(
        call(&state, "n", "api", json!({})),
        json!([41, 42, 42, true])
    );
    drop(state);
    let state = support::memory_controller(&dir).unwrap();
    assert_eq!(
        call(&state, "n", "api", json!({})),
        json!([41, 42, 42, true])
    );
}

#[test]
fn package_imports_and_relative_imports_resolve_inside_the_namespace() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(
        &state,
        "n",
        "/pkg/__init__.py",
        "from . import helpers\nvalue = helpers.value\n",
    );
    write(&state, "n", "/pkg/helpers.py", "value = 42\n");
    write(&state, "n", "/pkg/nested/__init__.py", "");
    write(
        &state,
        "n",
        "/pkg/nested/helper.py",
        "from ..helpers import value\ndef answer():\n    return value\n",
    );
    publish(
        &state,
        "n",
        "api",
        "import pkg.helpers\nfrom pkg.nested.helper import answer as get_answer\ndef endpoint():\n    return [pkg.value, pkg.helpers.value, get_answer()]\n",
        json!({}),
    );
    assert_eq!(call(&state, "n", "api", json!({})), json!([42, 42, 42]));
    write(
        &state,
        "n",
        "/pkg/api.py",
        "import helpers\ndef endpoint():\n    return helpers.value\n",
    );
    run(
        &state,
        "function.declare",
        json!({"namespace":"n","name":"sibling","file":"/pkg/api.py","symbol":"endpoint"}),
    );
    assert_eq!(call(&state, "n", "sibling", json!({})), 42);
}

#[test]
fn imports_are_lazy_cached_and_isolated_and_failed_imports_can_retry() {
    let (_dir, state) = service();
    namespace(&state, "n");
    namespace(&state, "other");
    write(&state, "other", "/secret.py", "value = 99\n");
    write(&state, "n", "/bad.py", "raise ValueError('bad')\n");
    publish(
        &state,
        "n",
        "api",
        "def endpoint():\n    if False:\n        import unavailable\n    failures = 0\n    for i in range(2):\n        try:\n            import bad\n        except ValueError:\n            failures += 1\n    try:\n        import secret\n    except ModuleNotFoundError:\n        return failures\n",
        json!({}),
    );
    assert_eq!(call(&state, "n", "api", json!({})), 2);
    assert_eq!(
        state
            .tool("execute", json!({"namespace":"n","script":"import bad"}))
            .unwrap_err()
            .code,
        crate::ErrorCode::PythonError
    );
}

#[test]
fn imported_helpers_use_host_callbacks_and_import_initialization_remains_effect_free() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(
        &state,
        "n",
        "/helpers.py",
        "def update():\n    write_text('/result', 'ok')\n    return read_text('/result')\n",
    );
    publish(
        &state,
        "n",
        "api",
        "from helpers import update\ndef endpoint():\n    return update()\n",
        json!({}),
    );
    assert_eq!(call(&state, "n", "api", json!({})), "ok");
    write(&state, "n", "/effects.py", "write_text('/effect', 'bad')\n");
    write(
        &state,
        "n",
        "/bad_api.py",
        "import effects\ndef endpoint():\n    return 1\n",
    );
    assert_eq!(
        state
            .tool(
                "function.declare",
                json!({"namespace":"n","name":"bad","file":"/bad_api.py","symbol":"endpoint"})
            )
            .unwrap_err()
            .code,
        crate::ErrorCode::PermissionDenied
    );
    assert_eq!(
        state
            .tool("fs.read", json!({"namespace":"n","path":"/effect"}))
            .unwrap_err()
            .code,
        crate::ErrorCode::NotFound
    );
    assert_eq!(
        run(
            &state,
            "execute",
            json!({"namespace":"n","script":"import effects\nread_text('/effect')"})
        )["value"],
        "bad"
    );
}

#[test]
fn wildcard_imports_honor_all_and_circular_imports_share_module_identity() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(
        &state,
        "n",
        "/exports.py",
        "__all__ = ['value']\nvalue = 42\nhidden = 99\n",
    );
    write(
        &state,
        "n",
        "/a.py",
        "value = 1\nimport b\ndef answer():\n    return b.a.value + b.value\n",
    );
    write(&state, "n", "/b.py", "import a\nvalue = 2\n");
    publish(
        &state,
        "n",
        "api",
        "from exports import *\nimport a\ndef endpoint():\n    return [value, a.answer()]\n",
        json!({}),
    );
    assert_eq!(call(&state, "n", "api", json!({})), json!([42, 3]));
}

#[test]
fn package_from_import_and_namespace_packages_work_without_preloaded_children() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(&state, "n", "/pkg/__init__.py", "from . import helpers\n");
    write(&state, "n", "/pkg/helpers.py", "value = 42\n");
    write(&state, "n", "/space/helpers.py", "value = 43\n");
    publish(
        &state,
        "n",
        "api",
        "from pkg import helpers\nimport space.helpers as other\ndef endpoint():\n    return [helpers.value, other.value]\n",
        json!({}),
    );
    assert_eq!(call(&state, "n", "api", json!({})), json!([42, 43]));
}

#[test]
fn module_attributes_and_globals_share_one_namespace_without_leaking_to_the_entry() {
    let (_dir, state) = service();
    namespace(&state, "n");
    write(
        &state,
        "n",
        "/helper.py",
        "value = 1\ndef read():\n    return value\n",
    );
    publish(
        &state,
        "n",
        "api",
        "value = 100\nimport helper\ndef endpoint():\n    helper.value = 7\n    return [value, helper.read(), helper.__dict__['value'], helper.__name__, helper.__file__]\n",
        json!({}),
    );
    assert_eq!(
        call(&state, "n", "api", json!({})),
        json!([100, 7, 7, "helper", "/helper.py"])
    );
}

#[test]
fn namespace_descriptions_persist_copy_and_obey_revision_guards() {
    let (dir, state) = service();
    let old = run(&state, "namespace.create", json!({"name":"undescribed"}));
    assert_eq!(old["description"], Value::Null);
    let created = run(
        &state,
        "namespace.create",
        json!({"name":"notes", "description":"Shared notes"}),
    );
    assert_eq!(created["description"], "Shared notes");
    let renamed = run(
        &state,
        "namespace.update",
        json!({"namespace":"notes", "name":"memory"}),
    );
    assert_eq!(renamed["description"], "Shared notes");
    let updated = run(
        &state,
        "namespace.update",
        json!({"namespace":"memory", "description":"Shared memory", "expected_revision":renamed["revision"]}),
    );
    assert_eq!(updated["name"], "memory");
    assert_ne!(updated["revision"], renamed["revision"]);
    assert_eq!(state.tool("namespace.update", json!({"namespace":"memory", "description":null, "expected_revision":renamed["revision"]})).unwrap_err().code, crate::ErrorCode::Conflict);
    let copied = run(
        &state,
        "namespace.copy",
        json!({"namespace":"memory", "name":"backup"}),
    );
    assert_eq!(copied["description"], "Shared memory");
    let cleared_copy = run(
        &state,
        "namespace.copy",
        json!({"namespace":"memory", "name":"blank", "description":null}),
    );
    assert_eq!(cleared_copy["description"], Value::Null);
    assert!(state.tool("execute", json!({"script":"mcp('namespace.update', {'namespace':'memory', 'description':'rolled back'})\n1 / 0"})).is_err());
    drop(state);
    let state = support::memory_controller(&dir).unwrap();
    assert_eq!(
        run(&state, "namespace.get", json!({"namespace":"memory"}))["description"],
        "Shared memory"
    );
    let listed = run(&state, "namespace.list", json!({}));
    assert!(
        listed["namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|ns| ns["name"] == "memory" && ns["description"] == "Shared memory")
    );
    let cleared = run(
        &state,
        "namespace.update",
        json!({"namespace":"memory", "description":null}),
    );
    assert_eq!(cleared["description"], Value::Null);
    assert!(
        state
            .tool(
                "namespace.update",
                json!({"namespace":"memory", "description":42})
            )
            .is_err()
    );
}

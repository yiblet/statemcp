use serde_json::{Value, json};
fn tool_value(result: &Value) -> Value {
    assert!(result.get("structuredContent").is_none());
    result["content"].clone()
}

use statemcp::{Server, State};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn embedded_service_dispatches_through_mcp_and_rolls_back_failed_scripts() {
    let path = std::env::temp_dir().join(format!(
        "statemcp-wire-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let state = State::open(&path).unwrap();
    let mut server = Server::new(state.clone());
    server.handle(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"integration","version":"1"}}}));
    server.handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let response = server.handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"namespace.create","arguments":{"name":"app"}}})).unwrap();
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(tool_value(&response["result"])["name"], "app");
    let response = server.handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"execute","arguments":{"namespace":"app","script":"write_text('/failed.txt', 'rollback')\n1 / 0"}}})).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(
        state
            .dispatch("fs.read", json!({"namespace":"app","path":"/failed.txt"}))
            .is_err()
    );
    let result = state.dispatch("execute", json!({"namespace":"app","script":"write_text('/ok.txt', 'saved')\nread_text('/ok.txt')"})).unwrap();
    assert_eq!(result["value"], "saved");
    drop(server);
    drop(state);
    let reopened = State::open(&path).unwrap();
    let file: Value = reopened
        .dispatch("fs.read", json!({"namespace":"app","path":"/ok.txt"}))
        .unwrap();
    assert_eq!(file["text"], "saved");
    drop(reopened);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn executable_runs_real_worker_callbacks_and_explicit_maintenance() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let path = std::env::temp_dir().join(format!(
        "statemcp-cli-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_statemcp"))
        .arg("stdio")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute","arguments":{"script":"mcp('state_namespace', {'action': 'create', 'name': 'cli'})\nmcp('state_fs', {'action': 'write', 'namespace': 'cli', 'path': '/value.txt', 'text': 'persisted'})\n42"}}}),
    ];
    let mut input = child.stdin.take().unwrap();
    for request in requests {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    assert_eq!(tool_value(&responses[1]["result"])["value"], 42);
    let state = State::open(&path).unwrap();
    assert_eq!(
        state
            .dispatch("fs.read", json!({"namespace":"cli","path":"/value.txt"}))
            .unwrap()["text"],
        "persisted"
    );
    drop(state);
    let maintenance = Command::new(env!("CARGO_BIN_EXE_statemcp"))
        .args(["maintenance", "--retain-receipts", "0"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        maintenance.status.success(),
        "{}",
        String::from_utf8_lossy(&maintenance.stderr)
    );
    let report: Value = serde_json::from_slice(&maintenance.stdout).unwrap();
    assert!(report["revisions_removed"].is_number());
    fs::remove_dir_all(path).unwrap();
}

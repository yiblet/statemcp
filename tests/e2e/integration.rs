use serde_json::{Value, json};
use state_core::Controller;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn tool_value(result: &Value) -> Value {
    assert!(result.get("structuredContent").is_none());
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn executable_runs_real_worker_callbacks_and_explicit_maintenance() -> anyhow::Result<()> {
    let _serial = super::common::serial();
    use std::{
        io::{BufRead, BufReader, Write},
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
        .env("RUST_LOG", "debug")
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
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"execute","arguments":{"script":"mcp('namespace.create', { 'name': 'cli'})\nmcp('fs.write', { 'namespace': 'cli', 'path': '/value.txt', 'text': 'persisted'})\n42"}}}),
    ];
    let mut input = child.stdin.take().unwrap();
    for request in requests {
        writeln!(input, "{request}").unwrap();
    }
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut replies = Vec::new();
    for _ in 0..2 {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        replies.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("rmcp::service"));
    let responses = replies;
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    assert_eq!(tool_value(&responses[1]["result"])["value"], 42);
    let state = Controller::open(&path).unwrap();
    let file = state.read_file(state_contract::file::FsReadInput {
        namespace: "cli".into(),
        path: "/value.txt".into(),
    })?;
    assert!(
        matches!(file, state_contract::file::FileContents::Text { text, .. } if text == "persisted")
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
    fs::remove_dir_all(path)?;
    Ok(())
}

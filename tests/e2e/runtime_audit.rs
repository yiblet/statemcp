//! Adversarial tests across the actual statemcp executable's worker boundary.
use serde_json::{Map, Value, json};
use state_runtime::{Limits, RuntimeError, WorkerConfig};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

fn worker() -> WorkerConfig {
    WorkerConfig::new(env!("CARGO_BIN_EXE_statemcp"))
}
fn deny(_: &str, _: Vec<Value>, _: Map<String, Value>) -> Result<Value, RuntimeError> {
    panic!("unexpected host effect")
}

#[test]
fn escaped_callback_arguments_and_results_cannot_bypass_json_byte_budget() {
    let _serial = super::common::serial();
    let limits = Limits {
        max_output_bytes: 128,
        ..Limits::default()
    };
    // Raw string length fits, but its JSON representation exceeds the budget.
    let error = worker()
        .execute("mcp('\\x00' * 30)", Value::Null, &limits, &mut deny)
        .unwrap_err();
    assert_eq!(error.code, state_core::ErrorCode::LimitExceeded);
    let mut calls = 0;
    let error = worker()
        .execute(
            "try:\n    mcp('first')\nexcept Exception:\n    pass\nmcp('later')",
            Value::Null,
            &limits,
            &mut |_: &str,
                  _: Vec<serde_json::Value>,
                  _: serde_json::Map<String, serde_json::Value>| {
                calls += 1;
                Ok(json!("\0".repeat(30)))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, state_core::ErrorCode::LimitExceeded);
    assert_eq!(calls, 1, "invalid host results must not resume Python");
}

#[test]
fn worker_denies_files_network_processes_and_sleep_before_later_effects() {
    let _serial = super::common::serial();
    for source in [
        "from pathlib import Path\nPath('/etc/passwd').read_text()",
        "from pathlib import Path\nPath('/tmp/state-runtime-audit-denied').write_text('bad')",
        "import socket\nsocket.socket()",
        "import subprocess\nsubprocess.run(['true'])",
        "import os\nos.getenv('HOME')",
        "import time\ntime.sleep(60)",
    ] {
        let source = format!("{source}\nmcp('later')");
        let error = worker()
            .execute(&source, Value::Null, &Limits::default(), &mut deny)
            .unwrap_err();
        assert!(
            matches!(error.code.as_str(), "PERMISSION_DENIED" | "PYTHON_ERROR"),
            "{source}: {error}"
        );
    }
}

#[test]
fn module_default_arguments_cannot_hide_initialization_effects() {
    let _serial = super::common::serial();
    let source = "def endpoint(value=mcp('effect')):\n    return value";
    let error = worker()
        .invoke(
            source,
            "endpoint",
            json!({}).as_object().unwrap().clone(),
            &Limits::default(),
            &mut deny,
        )
        .unwrap_err();
    assert_eq!(error.code, state_core::ErrorCode::PermissionDenied);
}

fn write_frame(input: &mut impl Write, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    input.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
    input.write_all(&body).unwrap();
    input.flush().unwrap();
}

fn read_frame(output: &mut impl Read) -> Value {
    let mut header = [0; 4];
    output.read_exact(&mut header).unwrap();
    let length = u32::from_be_bytes(header) as usize;
    assert!(length <= 1024);
    let mut body = vec![0; length];
    output.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn malformed_host_reply_ends_worker_without_a_second_callback() {
    let _serial = super::common::serial();
    for reply in [
        json!({"type": "host_result", "value": null, "extra": true}),
        json!({"type": "complete", "result": {"value": null, "stdout": ""}}),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_statemcp"))
            .args(["--worker", "67108864", "1024"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        write_frame(
            &mut input,
            &json!({
                "operation": {"operation": "execute", "modules": {"entry_path":"/state.py", "files":{}}, "source": "try:\n    mcp('first')\nexcept Exception:\n    pass\nmcp('later')", "inputs": null},
                "limits": Limits::default(),
            }),
        );
        assert_eq!(read_frame(&mut output)["type"], "host_call");
        write_frame(&mut input, &reply);
        drop(input);
        let final_frame = read_frame(&mut output);
        assert_eq!(final_frame["type"], "error");
        assert_eq!(final_frame["error"]["code"], "WORKER_FAILED");
        let mut remaining = Vec::new();
        output.read_to_end(&mut remaining).unwrap();
        assert!(remaining.is_empty());
        assert!(child.wait().unwrap().success());
    }
}

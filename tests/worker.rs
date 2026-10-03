use serde_json::{Value, json};
use state_runtime::{Limits, RuntimeError, WorkerConfig};
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn worker() -> WorkerConfig {
    WorkerConfig::new(env!("CARGO_BIN_EXE_state-mcp"))
}
fn deny(_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>) -> Result<Value, RuntimeError> {
    panic!("unexpected host callback")
}
fn healthy(config: &WorkerConfig) {
    assert_eq!(
        config
            .execute("inputs + 1", json!(41), &Limits::default(), &mut deny)
            .unwrap()
            .value,
        json!(42)
    );
}

#[test]
fn isolated_execute_invoke_and_validate() {
    let config = worker();
    let result = config
        .execute(
            "print('captured')\ninputs['value']",
            json!({"value": [true, null, "quote '\n"]}),
            &Limits::default(),
            &mut deny,
        )
        .unwrap();
    assert_eq!(result.value, json!([true, null, "quote '\n"]));
    assert_eq!(result.stdout, "captured\n");
    let source = "def add(a, b):\n    return a + b";
    config
        .validate_module(source, "add", &Limits::default())
        .unwrap();
    assert_eq!(
        config
            .invoke(
                source,
                "add",
                json!({"a": 2, "b": 3}),
                &Limits::default(),
                &mut deny
            )
            .unwrap()
            .value,
        json!(5)
    );
    let bad = "mcp('state_fs', {})\ndef bad():\n    return 1";
    assert_eq!(
        config
            .validate_module(bad, "bad", &Limits::default())
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert_eq!(
        config
            .invoke(bad, "bad", json!({}), &Limits::default(), &mut deny)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
}

#[test]
fn callbacks_and_recursive_workers_have_no_pool_deadlock() {
    let config = worker();
    let mut names = Vec::new();
    let result = config
        .execute(
            "mcp('state_fs', {'path': '/a'})\ncall('other', 'add', {'x': inputs})",
            json!(7),
            &Limits::default(),
            &mut |name, args, kwargs| {
                assert!(kwargs.is_empty());
                names.push(name.to_owned());
                match name {
                    "mcp" => {
                        assert_eq!(args, vec![json!("state_fs"), json!({"path":"/a"})]);
                        Ok(Value::Null)
                    }
                    "call" => config
                        .invoke(
                            "def add(x):\n    return x + 1",
                            "add",
                            args[2].clone(),
                            &Limits::default(),
                            &mut deny,
                        )
                        .map(|r| r.value),
                    _ => panic!("unknown host name"),
                }
            },
        )
        .unwrap();
    assert_eq!(result.value, json!(8));
    assert_eq!(names, ["mcp", "call"]);
}

#[test]
fn timeout_memory_recursion_and_output_limits_recover() {
    let config = worker();
    let cases = [
        (
            "while True:\n    pass",
            Limits {
                max_duration: Duration::from_millis(30),
                ..Limits::default()
            },
        ),
        (
            "items = []\nfor i in range(1000000):\n    items.append(str(i) + 'x' * 100)\nlen(items)",
            Limits {
                max_memory: Some(4 * 1024 * 1024),
                ..Limits::default()
            },
        ),
        (
            "def recurse():\n    return recurse()\nrecurse()",
            Limits {
                max_recursion_depth: 8,
                ..Limits::default()
            },
        ),
        (
            "print('a' * 10000)",
            Limits {
                max_output_bytes: 1024,
                ..Limits::default()
            },
        ),
    ];
    for (source, limits) in cases {
        let started = Instant::now();
        let error = config
            .execute(source, Value::Null, &limits, &mut deny)
            .unwrap_err();
        assert_eq!(error.code, "LIMIT_EXCEEDED", "{error}");
        assert!(started.elapsed() < Duration::from_secs(6));
        healthy(&config);
    }
}

#[test]
fn callback_errors_and_panics_reap_workers() {
    let config = worker();
    let error = config
        .execute(
            "try:\n    mcp('a', {})\nexcept Exception:\n    mcp('b', {})",
            Value::Null,
            &Limits::default(),
            &mut |_, _, _| Err(RuntimeError::new("DENIED_TEST", "stop")),
        )
        .unwrap_err();
    assert_eq!(error.code, "DENIED_TEST");
    let caught = std::panic::catch_unwind(|| {
        let _ = config.execute(
            "mcp('a', {})",
            Value::Null,
            &Limits::default(),
            &mut |_, _, _| panic!("cancel callback"),
        );
    });
    assert!(caught.is_err());
    healthy(&config);
}

#[test]
fn callback_time_counts_toward_deadline() {
    let config = worker();
    let limits = Limits {
        max_duration: Duration::from_millis(150),
        ..Limits::default()
    };
    let mut called = false;
    let result = config.execute(
        "mcp('wait', {})\n42",
        Value::Null,
        &limits,
        &mut |_, _, _| {
            called = true;
            std::thread::sleep(Duration::from_millis(250));
            Ok(Value::Null)
        },
    );
    assert!(called);
    assert_eq!(result.unwrap_err().code, "LIMIT_EXCEEDED");
    healthy(&config);
}

#[test]
fn frames_are_bounded_and_malformed_workers_fail_closed() {
    let mut config = worker();
    config.max_frame_bytes = 1024;
    assert_eq!(
        config
            .execute(
                "inputs",
                json!("a".repeat(2048)),
                &Limits::default(),
                &mut deny
            )
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
    healthy(&config);
    for body in [
        vec![255, 255, 255, 255],
        vec![0, 0, 0, 1, b'{'],
        vec![0, 0, 0, 100, b'{'],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_state-mcp"))
            .args(["--worker", "67108864", "1024"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&body).unwrap();
        assert!(!child.wait_with_output().unwrap().status.success());
    }
}

#[test]
fn worker_does_not_need_python_or_modify_parent_allocator() {
    assert!(!state_runtime::memory_tracking_active());
    healthy(&worker());
    assert!(!state_runtime::memory_tracking_active());
    // The child executable is absolute; a completely empty PATH still works.
    let mut child = Command::new(env!("CARGO_BIN_EXE_state-mcp"))
        .args(["--worker", "67108864", "8388608"])
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let message = serde_json::to_vec(&json!({"operation": {"operation": "execute", "source": "21 * 2", "inputs": null}, "limits": Limits::default()})).unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(&(message.len() as u32).to_be_bytes())
        .unwrap();
    input.write_all(&message).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frame: Value = serde_json::from_slice(&output.stdout[4..]).unwrap();
    assert_eq!(frame["result"]["value"], json!(42));
}

#[test]
fn fatal_allocator_exit_during_frame_decode_recovers() {
    let config = worker();
    // Frame allocation precedes Monty initialization and exceeds the small
    // budget plus exception headroom, exercising actual child exit code 65.
    let limits = Limits {
        max_memory: Some(1024),
        ..Limits::default()
    };
    let error = config
        .execute(
            "inputs",
            json!("x".repeat(6 * 1024 * 1024)),
            &limits,
            &mut deny,
        )
        .unwrap_err();
    assert_eq!(error.code, "LIMIT_EXCEEDED", "{error}");
    assert!(error.message.contains("memory"), "{error}");
    healthy(&config);
}

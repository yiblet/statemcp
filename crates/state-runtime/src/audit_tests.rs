//! Adversarial interpreter and pre-spawn boundary checks without external I/O.
use crate::{Limits, RuntimeError, WorkerConfig};
use serde_json::{Map, Value, json};
fn deny(_: &str, _: Vec<Value>, _: Map<String, Value>) -> Result<Value, RuntimeError> {
    panic!("unexpected host effect")
}

#[test]
fn embedded_bindings_share_one_budget_including_names_and_escaping() {
    let limits = Limits {
        max_output_bytes: 128,
        ..Limits::default()
    };
    for bindings in [
        json!({"a": "x".repeat(60), "b": "x".repeat(60), "c": "x".repeat(60)}),
        json!({"a": "\0".repeat(30)}),
        json!({"a".repeat(128): null}),
    ] {
        assert_eq!(
            crate::execute_with_bindings(
                "mcp('effect', {})",
                bindings.as_object().unwrap().clone(),
                &limits,
                &mut deny,
            )
            .unwrap_err()
            .code,
            crate::ErrorCode::LimitExceeded
        );
    }
}

#[test]
fn escaped_callback_arguments_and_results_cannot_bypass_json_byte_budget() {
    let limits = Limits {
        max_output_bytes: 128,
        ..Limits::default()
    };
    // Raw string length fits, but its JSON representation exceeds the budget.
    let error = crate::execute("mcp('\\x00' * 30)", Value::Null, &limits, &mut deny).unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::LimitExceeded);
    let mut calls = 0;
    let error = crate::execute(
        "try:\n    mcp('first')\nexcept Exception:\n    pass\nmcp('later')",
        Value::Null,
        &limits,
        &mut |_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>| {
            calls += 1;
            Ok(json!("\0".repeat(30)))
        },
    )
    .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::LimitExceeded);
    assert_eq!(calls, 1, "invalid host results must not resume Python");
}

#[test]
fn runtime_denies_files_network_processes_and_sleep_before_later_effects() {
    for source in [
        "from pathlib import Path\nPath('/etc/passwd').read_text()",
        "from pathlib import Path\nPath('/tmp/state-runtime-audit-denied').write_text('bad')",
        "import socket\nsocket.socket()",
        "import subprocess\nsubprocess.run(['true'])",
        "import os\nos.getenv('HOME')",
        "import time\ntime.sleep(60)",
    ] {
        let source = format!("{source}\nmcp('later')");
        let error =
            crate::execute(&source, Value::Null, &Limits::default(), &mut deny).unwrap_err();
        assert!(
            matches!(error.code.as_str(), "PERMISSION_DENIED" | "PYTHON_ERROR"),
            "{source}: {error}"
        );
    }
}

#[test]
fn module_default_arguments_cannot_hide_initialization_effects() {
    let source = "def endpoint(value=mcp('effect')):\n    return value";
    let error = crate::invoke(
        source,
        "endpoint",
        json!({}).as_object().unwrap().clone(),
        &Limits::default(),
        &mut deny,
    )
    .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::PermissionDenied);
}

#[test]
fn source_limits_apply_before_spawning_and_nested_values_before_host_dispatch() {
    // A missing executable proves this rejection happens in the parent first.
    let error = WorkerConfig::new("/nonexistent/statemcp-runtime-audit")
        .execute(
            "12345",
            Value::Null,
            &Limits {
                max_source_bytes: 4,
                ..Limits::default()
            },
            &mut deny,
        )
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::LimitExceeded);
    let limits = Limits {
        max_json_depth: 2,
        ..Limits::default()
    };
    for (source, inputs) in [
        ("mcp('later')", json!([[[[null]]]])),
        ("mcp([[[[None]]]])", Value::Null),
    ] {
        assert_eq!(
            crate::execute(source, inputs, &limits, &mut deny)
                .unwrap_err()
                .code,
            crate::ErrorCode::LimitExceeded
        );
    }
}

#[test]
fn imported_sources_are_bounded_before_spawning_the_worker() {
    let modules = crate::ModuleSources {
        entry_path: "/api.py".into(),
        files: std::collections::BTreeMap::from([("/helper.py".into(), "x".repeat(100))]),
    };
    let limits = Limits {
        max_source_bytes: 64,
        ..Limits::default()
    };
    let error = WorkerConfig::new("/nonexistent/statemcp-import-audit")
        .execute_with_modules(
            "42",
            Value::Null,
            &modules,
            &limits,
            &mut |_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>| unreachable!(),
        )
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::LimitExceeded);
}

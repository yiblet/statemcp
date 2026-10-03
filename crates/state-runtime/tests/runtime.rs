use serde_json::{Value, json};
use state_runtime::{Limits, RuntimeError, execute, invoke, validate_module};
use std::time::Duration;

fn no_host(
    _: &str,
    _: Vec<Value>,
    _: serde_json::Map<String, Value>,
) -> Result<Value, RuntimeError> {
    panic!("unexpected host call")
}

#[test]
fn execute_returns_final_value_and_captures_prints() {
    let result = execute(
        "print('hello', inputs['name'])\n{'answer': inputs['value'] * 2}",
        json!({"name":"agent", "value":21}),
        &Limits::default(),
        &mut no_host,
    )
    .unwrap();
    assert_eq!(result.value, json!({"answer":42}));
    assert_eq!(result.stdout, "hello agent\n");
}

#[test]
fn all_host_callbacks_roundtrip_json_and_keywords() {
    for name in state_runtime::HOST_FUNCTIONS {
        let mut seen = false;
        let result = execute(
            &format!("{name}(inputs, mode='read')"),
            json!({"a":[1,true,null,"x"]}),
            &Limits::default(),
            &mut |called, args, kwargs| {
                assert_eq!(called, *name);
                assert_eq!(
                    args,
                    json!([{"a":[1,true,null,"x"]}]).as_array().unwrap().clone()
                );
                assert_eq!(kwargs, json!({"mode":"read"}).as_object().unwrap().clone());
                seen = true;
                Ok(json!({"ok": true}))
            },
        )
        .unwrap();
        assert!(seen);
        assert_eq!(result.value, json!({"ok":true}));
    }
}

#[test]
fn module_initialization_cannot_mutate_even_when_python_catches_error() {
    let source =
        "try:\n    write_text('/x', 'bad')\nexcept Exception:\n    pass\ndef run():\n    return 1";
    assert_eq!(
        validate_module(source, "run", &Limits::default())
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert_eq!(
        invoke(source, "run", json!({}), &Limits::default(), &mut no_host)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
}

#[test]
fn invoked_function_has_callbacks_and_bound_keyword_arguments() {
    let source =
        "factor = 2\ndef run(title, value=3):\n    return db_query('app', title, [value * factor])";
    validate_module(source, "run", &Limits::default()).unwrap();
    let attack = "'); write_text('/oops', 'oops') #";
    let result = invoke(
        source,
        "run",
        json!({"title":attack,"value":21}),
        &Limits::default(),
        &mut |name, args, _| {
            assert_eq!(name, "db_query");
            assert_eq!(
                args,
                json!(["app", attack, [42]]).as_array().unwrap().clone()
            );
            Ok(json!({"rows":[[42]]}))
        },
    )
    .unwrap();
    assert_eq!(result.value, json!({"rows":[[42]]}));
}

#[test]
fn host_errors_cannot_be_caught_and_followed_by_effects() {
    let mut calls = 0;
    let error = execute(
        "try:\n    db_query('app', 'bad')\nexcept Exception:\n    write_text('/x', 'bad')\n42",
        json!({}),
        &Limits::default(),
        &mut |_, _, _| {
            calls += 1;
            Err(RuntimeError::new("SQL_ERROR", "failed"))
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "SQL_ERROR");
    assert_eq!(calls, 1);
}

#[test]
fn host_call_budget_is_enforced_before_dispatch() {
    let mut calls = 0;
    let limits = Limits {
        max_calls: 2,
        ..Limits::default()
    };
    let error = execute(
        "for i in range(3):\n    read_text('/x')",
        Value::Null,
        &limits,
        &mut |_, _, _| {
            calls += 1;
            Ok(Value::Null)
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "LIMIT_EXCEEDED");
    assert_eq!(calls, 2);
}

#[test]
fn timeout_then_fresh_execution_succeeds() {
    let limits = Limits {
        max_duration: Duration::from_millis(20),
        ..Limits::default()
    };
    let error = execute("while True:\n    pass", Value::Null, &limits, &mut no_host).unwrap_err();
    assert_eq!(error.code, "LIMIT_EXCEEDED", "{error}");
    assert_eq!(
        execute("6 * 7", Value::Null, &Limits::default(), &mut no_host)
            .unwrap()
            .value,
        json!(42)
    );
}

#[test]
fn recursion_and_large_allocation_are_bounded() {
    let limits = Limits {
        max_recursion_depth: 10,
        max_allocation_bytes: 1024 * 1024,
        ..Limits::default()
    };
    for code in ["def f():\n    return f()\nf()", "'x' * 10000000"] {
        let error = execute(code, Value::Null, &limits, &mut no_host).unwrap_err();
        assert_eq!(error.code, "LIMIT_EXCEEDED", "{error}");
    }
}

#[test]
fn unarmed_aggregate_memory_limit_is_explicitly_rejected() {
    let limits = Limits {
        max_memory: Some(1024 * 1024),
        ..Limits::default()
    };
    assert_eq!(
        execute("1", Value::Null, &limits, &mut no_host)
            .unwrap_err()
            .code,
        "UNSUPPORTED_FEATURE"
    );
}

#[test]
fn host_filesystem_access_is_denied() {
    let error = execute(
        "from pathlib import Path\nPath('/etc/passwd').read_text()",
        Value::Null,
        &Limits::default(),
        &mut no_host,
    )
    .unwrap_err();
    assert_eq!(error.code, "PERMISSION_DENIED");
}

#[test]
fn unsupported_results_and_cycles_are_rejected() {
    for source in [
        "float('nan')",
        "{1: 'x'}",
        "{1, 2}",
        "2 ** 100",
        "x = []\nx.append(x)\nx",
    ] {
        assert!(
            execute(source, Value::Null, &Limits::default(), &mut no_host).is_err(),
            "{source}"
        );
    }
}

#[test]
fn output_and_input_depth_are_bounded() {
    let limits = Limits {
        max_output_bytes: 30,
        max_json_depth: 3,
        ..Limits::default()
    };
    assert_eq!(
        execute("print('x' * 31)", Value::Null, &limits, &mut no_host)
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
    assert_eq!(
        execute("inputs", json!([[[[[]]]]]), &limits, &mut no_host)
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
}

#[test]
fn endpoint_symbol_injection_and_missing_symbol_are_rejected() {
    for symbol in ["x(); print('oops')", "missing", "mcp", "__state_arguments"] {
        assert_eq!(
            validate_module("def run():\n    return 1", symbol, &Limits::default())
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
    }
}

#[test]
fn globals_do_not_persist_across_executions() {
    execute("secret = 42", Value::Null, &Limits::default(), &mut no_host).unwrap();
    assert_eq!(
        execute("secret", Value::Null, &Limits::default(), &mut no_host)
            .unwrap_err()
            .code,
        "PYTHON_ERROR"
    );
}

#[test]
fn caught_print_overflow_still_aborts_execution() {
    let limits = Limits {
        max_output_bytes: 30,
        ..Limits::default()
    };
    let error = execute(
        "try:\n    print('x' * 31)\nexcept MemoryError:\n    pass\n1",
        Value::Null,
        &limits,
        &mut no_host,
    )
    .unwrap_err();
    assert_eq!(error.code, "LIMIT_EXCEEDED");
}

use crate::{Limits, RuntimeError};
use serde_json::{Value, json};
fn deny(_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>) -> Result<Value, RuntimeError> {
    Err(RuntimeError::new(
        crate::ErrorCode::WorkerFailed,
        "unexpected host callback",
    ))
}
fn healthy() {
    assert_eq!(
        crate::execute("inputs + 1", json!(41), &Limits::default(), &mut deny)
            .unwrap()
            .value,
        json!(42)
    );
}
#[test]
fn callbacks_compose_recursive_runtime_invocations() {
    let mut names = Vec::new();
    let result = crate::execute(
        "mcp('fs.read', {'path': '/a'})\ncall('other', 'add', {'x': inputs})",
        json!(7),
        &Limits::default(),
        &mut |name, args, kwargs| {
            assert!(kwargs.is_empty());
            names.push(name.to_owned());
            match name {
                "mcp" => {
                    assert_eq!(args, vec![json!("fs.read"), json!({"path":"/a"})]);
                    Ok(Value::Null)
                }
                "call" => crate::invoke(
                    "def add(x):\n    return x + 1",
                    "add",
                    args[2].as_object().unwrap().clone(),
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
fn callback_errors_and_panics_leave_next_invocation_healthy() {
    let error = crate::execute(
        "try:\n    mcp('a', {})\nexcept Exception:\n    mcp('b', {})",
        Value::Null,
        &Limits::default(),
        &mut |_, _, _| {
            Err(RuntimeError::new(
                crate::ErrorCode::PermissionDenied,
                "stop",
            ))
        },
    )
    .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::PermissionDenied);
    let caught = std::panic::catch_unwind(|| {
        let _ = crate::execute(
            "mcp('a', {})",
            Value::Null,
            &Limits::default(),
            &mut |_, _, _| panic!("cancel callback"),
        );
    });
    assert!(caught.is_err());
    healthy();
}

#[test]
fn invocations_preserve_namespace_modules_and_shared_globals() {
    let modules = crate::ModuleSources {
        entry_path: "/api.py".into(),
        files: std::collections::BTreeMap::from([(
            "/helpers.py".into(),
            "value = 40\ndef answer():\n    global value\n    value += 1\n    return value\n"
                .into(),
        )]),
    };
    let source = "import helpers\nfrom helpers import answer\ndef endpoint():\n    return [answer(), helpers.answer(), helpers.value]\n";
    crate::validate_module_with_modules(source, "endpoint", &modules, &Limits::default()).unwrap();
    for _ in 0..2 {
        assert_eq!(
            crate::invoke_with_modules(
                source,
                "endpoint",
                json!({}).as_object().unwrap().clone(),
                &modules,
                &Limits::default(),
                &mut deny
            )
            .unwrap()
            .value,
            json!([41, 42, 42])
        );
    }
    assert_eq!(
        crate::execute_with_modules(
            "import helpers\nhelpers.answer()",
            Value::Null,
            &modules,
            &Limits::default(),
            &mut deny
        )
        .unwrap()
        .value,
        41
    );
}

//! Actual aggregate allocator limits are armed only in an isolated test child.
use state_runtime::{Limits, arm_worker_memory_limit, execute};

#[test]
fn cumulative_memory_is_bounded_in_an_armed_worker() {
    let _serial = super::common::serial();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "allocator::allocator_child",
            "--nocapture",
        ])
        .env("STATE_RUNTIME_ALLOCATOR_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        (output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("LIMIT_EXCEEDED"))
            || output.status.code() == Some(65),
        "status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "subprocess entry point; arms a process-global allocator limit"]
fn allocator_child() {
    let _serial = super::common::serial();
    if std::env::var_os("STATE_RUNTIME_ALLOCATOR_CHILD").is_none() {
        return;
    }
    let memory = 4 * 1024 * 1024;
    arm_worker_memory_limit(memory).unwrap();
    let limits = Limits {
        max_memory: Some(memory),
        ..Limits::default()
    };
    let error = execute(
        "items = []\nfor i in range(1000000):\n    items.append(str(i) + 'x' * 100)\nlen(items)",
        serde_json::Value::Null,
        &limits,
        &mut |_: &str, _: Vec<serde_json::Value>, _: serde_json::Map<String, serde_json::Value>| {
            panic!("unexpected host call")
        },
    )
    .unwrap_err();
    assert_eq!(error.code, state_runtime::ErrorCode::LimitExceeded);
    println!("LIMIT_EXCEEDED");
}

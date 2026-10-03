//! Allocator ceilings are process-global, so exercise them only in a child.
use state_runtime::{LimitedAllocator, Limits, arm_worker_memory_limit, execute};

#[global_allocator]
static ALLOCATOR: LimitedAllocator = LimitedAllocator;

#[test]
fn cumulative_memory_is_bounded_in_an_armed_worker() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "allocator_child", "--nocapture"])
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
fn allocator_child() {
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
        &mut |_, _, _| panic!("unexpected host call"),
    )
    .unwrap_err();
    assert_eq!(error.code, "LIMIT_EXCEEDED");
    println!("LIMIT_EXCEEDED");
}

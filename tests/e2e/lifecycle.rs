//! Observe real worker cleanup through Linux process state while callbacks run.
use anyhow::{Context, Result};
use serde_json::Value;
use state_runtime::{ErrorCode, Limits, RuntimeError, WorkerConfig};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct TrackedWorker {
    _directory: tempfile::TempDir,
    config: WorkerConfig,
    pid_file: PathBuf,
}
impl TrackedWorker {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let pid_file = directory.path().join("worker.pid");
        let wrapper = directory.path().join("worker");
        // The shell records its PID, then exec replaces it with the real worker.
        // Quote paths as shell arguments so spaces and apostrophes stay literal.
        let quote = |path: &Path| -> Result<String> {
            let text = path.to_str().context("worker path is not UTF-8")?;
            Ok(format!("'{}'", text.replace('\'', "'\"'\"'")))
        };
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > {}\nexec {} \"$@\"\n",
                quote(&pid_file)?,
                quote(Path::new(env!("CARGO_BIN_EXE_statemcp")))?,
            ),
        )?;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            config: WorkerConfig::new(wrapper),
            pid_file,
            _directory: directory,
        })
    }
    fn pid(&self) -> Result<u32> {
        fs::read_to_string(&self.pid_file)?
            .parse()
            .context("invalid worker PID")
    }
}

fn wait_for_reap(pid: u32) -> Result<()> {
    let process = PathBuf::from(format!("/proc/{pid}"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while process.try_exists()? {
        anyhow::ensure!(Instant::now() < deadline, "worker {pid} was not reaped");
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

#[test]
fn watchdog_kills_and_reaps_while_parent_callback_is_running() -> Result<()> {
    let _serial = super::common::serial();
    let worker = TrackedWorker::new()?;
    let mut observation = None;
    let result = worker.config.execute(
        "mcp('wait', {})\n42",
        Value::Null,
        &Limits {
            max_duration: Duration::from_secs(1),
            ..Limits::default()
        },
        &mut |_, _, _| {
            // This callback returns only after /proc confirms the watchdog has
            // reaped the child. Dispatch cannot perform that cleanup for it.
            observation = Some(worker.pid().and_then(wait_for_reap));
            Ok(Value::Null)
        },
    );
    observation.context("worker never called the host")??;
    assert_eq!(
        result.err().map(|error| error.code),
        Some(ErrorCode::LimitExceeded)
    );
    Ok(())
}

#[test]
fn host_error_drops_guard_and_reaps_real_worker() -> Result<()> {
    let _serial = super::common::serial();
    let worker = TrackedWorker::new()?;
    let result = worker.config.execute(
        "mcp('stop', {})",
        Value::Null,
        &Limits::default(),
        &mut |_, _, _| Err(RuntimeError::new(ErrorCode::PermissionDenied, "stop")),
    );
    assert_eq!(
        result.err().map(|error| error.code),
        Some(ErrorCode::PermissionDenied)
    );
    wait_for_reap(worker.pid()?)?;
    Ok(())
}

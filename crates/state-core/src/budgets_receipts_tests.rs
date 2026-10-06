use crate::test_support as support;
use crate::{CoreLimits, EmbeddedBackend, RuntimeBackend, State};
use serde_json::{Map, Value, json};
use state_runtime::{HostCallback, Limits, RunResult, RuntimeError};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use support::ToolCalls;

struct ObservingBackend {
    seen: Mutex<Vec<Duration>>,
}
impl RuntimeBackend for ObservingBackend {
    fn execute(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.seen.lock().unwrap().push(limits.max_duration);
        let nested = if self.seen.lock().unwrap().len() == 1 {
            std::thread::sleep(Duration::from_millis(20));
            host(
                "mcp",
                vec![json!("execute"), json!({"script":"nested"})],
                Map::new(),
            )?
        } else {
            json!(42)
        };
        Ok(RunResult {
            value: nested,
            stdout: String::new(),
        })
    }
    fn invoke(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: Map<String, Value>,
        _: &Limits,
        _: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        unreachable!()
    }
    fn validate_module(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: &Limits,
    ) -> Result<(), RuntimeError> {
        unreachable!()
    }
}

#[test]
fn nested_execution_gets_only_remaining_root_wall_budget() {
    let dir = state_fs::MemoryFs::new();
    let backend = Arc::new(ObservingBackend {
        seen: Mutex::new(Vec::new()),
    });
    let mut limits = CoreLimits::default();
    limits.runtime.max_duration = Duration::from_secs(1);
    let state = State::with_backend(support::memory_store(&dir).unwrap(), backend.clone())
        .with_limits(limits);
    state.tool("execute", json!({"script":"outer"})).unwrap();
    let seen = backend.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[1] + Duration::from_millis(15) < seen[0]);
}

struct CatchingBackend;
impl RuntimeBackend for CatchingBackend {
    fn execute(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: Value,
        _: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        host(
            "mcp",
            vec![json!("namespace.create"), json!({"name":"staged"})],
            Map::new(),
        )?;
        let _ignored = host("unknown_helper", vec![], Map::new());
        Ok(RunResult {
            value: json!("success"),
            stdout: String::new(),
        })
    }
    fn invoke(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: Map<String, Value>,
        _: &Limits,
        _: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        unreachable!()
    }
    fn validate_module(
        &self,
        _: &str,
        _: &state_runtime::ModuleSources,
        _: &str,
        _: &Limits,
    ) -> Result<(), RuntimeError> {
        unreachable!()
    }
}
#[test]
fn core_poisoning_survives_a_backend_that_swallows_host_errors() {
    let dir = state_fs::MemoryFs::new();
    let state = State::with_backend(
        support::memory_store(&dir).unwrap(),
        Arc::new(CatchingBackend),
    );
    assert_eq!(
        state
            .tool("execute", json!({"script":"ignored"}))
            .unwrap_err()
            .code,
        crate::ErrorCode::InvalidArgument
    );
    assert_eq!(
        state.tool("namespace.list", json!({})).unwrap()["namespaces"],
        json!([])
    );
}

struct RacingBackend {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl RuntimeBackend for RacingBackend {
    fn execute(
        &self,
        source: &str,
        modules: &state_runtime::ModuleSources,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        let failure =
            || RuntimeError::new(crate::ErrorCode::WorkerFailed, "test coordination failed");
        self.entered.send(()).map_err(|_| failure())?;
        self.release
            .lock()
            .map_err(|_| failure())?
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| failure())?;
        EmbeddedBackend.execute(source, modules, inputs, limits, host)
    }
    fn invoke(
        &self,
        source: &str,
        modules: &state_runtime::ModuleSources,
        symbol: &str,
        args: Map<String, Value>,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        EmbeddedBackend.invoke(source, modules, symbol, args, limits, host)
    }
    fn validate_module(
        &self,
        source: &str,
        modules: &state_runtime::ModuleSources,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        EmbeddedBackend.validate_module(source, modules, symbol, limits)
    }
}
#[test]
fn concurrent_receipts_conflict_then_retry_replays_the_committed_result()
-> Result<(), Box<dyn std::error::Error>> {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let dir = state_fs::MemoryFs::new();
    let state = State::with_backend(
        support::memory_store(&dir)?,
        Arc::new(RacingBackend {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        }),
    );
    state.tool("namespace.create", json!({"name":"n"}))?;
    state.tool("db.create", json!({"namespace":"n","database":"app"}))?;
    state.tool(
        "db.execute",
        json!({"namespace":"n","database":"app","sql":"CREATE TABLE writes(value TEXT)"}),
    )?;
    let args = json!({"namespace":"n","idempotency_key":"same","script":"db_execute('app', \"INSERT INTO writes VALUES ('one') RETURNING value\")"});
    let left = state.clone();
    let left_args = args.clone();
    let thread = std::thread::spawn(move || left.tool("execute", left_args));
    entered_rx.recv_timeout(std::time::Duration::from_secs(5))?;
    assert!(matches!(
        state.tool("execute", args.clone()),
        Err(crate::Error {
            code: crate::ErrorCode::Conflict,
            ..
        })
    ));
    release_tx.send(())?;
    let left = thread
        .join()
        .map_err(|_| "receipt test thread panicked")??;
    assert_eq!(state.tool("execute", args)?, left);
    assert_eq!(
        state.tool(
            "db.query",
            json!({"namespace":"n","database":"app","sql":"SELECT count(*) FROM writes"})
        )?["rows"],
        json!([[1]])
    );
    Ok(())
}

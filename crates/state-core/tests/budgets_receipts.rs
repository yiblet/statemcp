use serde_json::{Map, Value, json};
use state_core::{CoreLimits, EmbeddedBackend, RuntimeBackend, State, Store};
use state_runtime::{HostCallback, Limits, RunResult, RuntimeError};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

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
                vec![json!("state_execute"), json!({"script":"nested"})],
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
        _: Value,
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
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ObservingBackend {
        seen: Mutex::new(Vec::new()),
    });
    let mut limits = CoreLimits::default();
    limits.runtime.max_duration = Duration::from_secs(1);
    let state =
        State::with_backend(Store::open(dir.path()).unwrap(), backend.clone()).with_limits(limits);
    state
        .dispatch("state_execute", json!({"script":"outer"}))
        .unwrap();
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
            vec![
                json!("state_namespace"),
                json!({"action":"create","name":"staged"}),
            ],
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
        _: Value,
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
    let dir = tempfile::tempdir().unwrap();
    let state = State::with_backend(Store::open(dir.path()).unwrap(), Arc::new(CatchingBackend));
    assert_eq!(
        state
            .dispatch("state_execute", json!({"script":"ignored"}))
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        state
            .dispatch("state_namespace", json!({"action":"list"}))
            .unwrap()["namespaces"],
        json!([])
    );
}

struct RacingBackend {
    barrier: Barrier,
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
        // TODO(audit): replace unbounded rendezvous with a timeout so early peer failure cannot hang this test.
        self.barrier.wait();
        EmbeddedBackend.execute(source, modules, inputs, limits, host)
    }
    fn invoke(
        &self,
        source: &str,
        modules: &state_runtime::ModuleSources,
        symbol: &str,
        args: Value,
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
fn concurrent_identical_receipts_publish_once_and_return_the_winners_result() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::with_backend(
        Store::open(dir.path()).unwrap(),
        Arc::new(RacingBackend {
            barrier: Barrier::new(2),
        }),
    );
    state
        .dispatch("state_namespace", json!({"action":"create","name":"n"}))
        .unwrap();
    state
        .dispatch(
            "state_db",
            json!({"action":"create","namespace":"n","database":"app"}),
        )
        .unwrap();
    state.dispatch("state_db",json!({"action":"execute","namespace":"n","database":"app","sql":"CREATE TABLE writes(value TEXT)"})).unwrap();
    let args = json!({"namespace":"n","idempotency_key":"same","script":"db_execute('app', \"INSERT INTO writes VALUES ('one') RETURNING value\")"});
    let left = state.clone();
    let left_args = args.clone();
    let thread = std::thread::spawn(move || left.dispatch("state_execute", left_args));
    let right = state.dispatch("state_execute", args).unwrap();
    assert_eq!(thread.join().unwrap().unwrap(), right);
    assert_eq!(state.dispatch("state_db",json!({"action":"query","namespace":"n","database":"app","sql":"SELECT count(*) FROM writes"})).unwrap()["rows"],json!([[1]]));
}

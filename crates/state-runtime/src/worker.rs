//! Disposable same-executable workers. The parent alone owns host capabilities.
use crate::{HostCallback, HttpRunResult, Limits, ModuleSources, RunResult, RuntimeError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use state_contract::http::HttpRequest;
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Binary implementing `--worker MEMORY_BYTES MAX_FRAME_BYTES`.
    pub executable: PathBuf,
    pub max_frame_bytes: usize,
    /// Used when Limits::max_memory is None. Isolated execution is always capped.
    pub default_memory_bytes: usize,
}
impl WorkerConfig {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            max_frame_bytes: 8 * 1024 * 1024,
            default_memory_bytes: 64 * 1024 * 1024,
        }
    }
    pub fn execute(
        &self,
        source: &str,
        inputs: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.execute_with_modules(source, inputs, &ModuleSources::default(), limits, host)
    }
    pub fn execute_with_modules(
        &self,
        source: &str,
        inputs: Value,
        modules: &ModuleSources,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.run_json(
            Operation::Execute {
                source: source.into(),
                modules: modules.clone(),
                inputs,
            },
            limits,
            host,
        )
    }
    pub fn invoke(
        &self,
        source: &str,
        symbol: &str,
        arguments: serde_json::Map<String, Value>,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.invoke_with_modules(
            source,
            symbol,
            arguments,
            &ModuleSources::default(),
            limits,
            host,
        )
    }
    pub fn invoke_with_modules(
        &self,
        source: &str,
        symbol: &str,
        arguments: serde_json::Map<String, Value>,
        modules: &ModuleSources,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.run_json(
            Operation::Invoke {
                source: source.into(),
                modules: modules.clone(),
                symbol: symbol.into(),
                arguments,
            },
            limits,
            host,
        )
    }
    pub fn validate_module(
        &self,
        source: &str,
        symbol: &str,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        self.validate_module_with_modules(source, symbol, &ModuleSources::default(), limits)
    }
    pub fn validate_module_with_modules(
        &self,
        source: &str,
        symbol: &str,
        modules: &ModuleSources,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        self.run_json(
            Operation::Validate {
                source: source.into(),
                modules: modules.clone(),
                symbol: symbol.into(),
            },
            limits,
            &mut |_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>| {
                Err(RuntimeError::new(
                    crate::ErrorCode::PermissionDenied,
                    "module initialization cannot call host functions",
                ))
            },
        )
        .map(|_| ())
    }
    /// Invoke a pinned HTTP handler in a disposable worker with native request/response values.
    pub fn invoke_http(
        &self,
        source: &str,
        symbol: &str,
        modules: &ModuleSources,
        request: HttpRequest,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<HttpRunResult, RuntimeError> {
        match self.run(
            Operation::InvokeHttp {
                source: source.into(),
                symbol: symbol.into(),
                modules: modules.clone(),
                request,
            },
            limits,
            host,
        )? {
            Completion::Http(result) => Ok(result),
            Completion::Json(_) => Err(protocol("worker returned JSON for an HTTP invocation")),
        }
    }
    /// Validate an HTTP module with effects denied and the Response type available.
    pub fn validate_http(
        &self,
        source: &str,
        symbol: &str,
        modules: &ModuleSources,
        limits: &Limits,
    ) -> Result<(), RuntimeError> {
        self.run_json(
            Operation::ValidateHttp {
                source: source.into(),
                symbol: symbol.into(),
                modules: modules.clone(),
            },
            limits,
            &mut |_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>| {
                Err(protocol("unexpected initialization callback"))
            },
        )
        .map(|_| ())
    }
    fn run_json(
        &self,
        operation: Operation,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        match self.run(operation, limits, host)? {
            Completion::Json(result) => Ok(result),
            Completion::Http(_) => Err(protocol("worker returned HTTP for a JSON invocation")),
        }
    }
    fn run(
        &self,
        operation: Operation,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<Completion, RuntimeError> {
        let started = Instant::now();
        let symbol = match &operation {
            Operation::InvokeHttp { symbol, .. }
            | Operation::Invoke { symbol, .. }
            | Operation::ValidateHttp { symbol, .. }
            | Operation::Validate { symbol, .. } => symbol.clone(),
            Operation::Execute { .. } => "execute".to_owned(),
        };
        validate_frame_limit(self.max_frame_bytes)?;
        let deadline = Instant::now()
            .checked_add(limits.max_duration)
            .ok_or_else(|| {
                RuntimeError::new(
                    crate::ErrorCode::InvalidArgument,
                    "invalid execution duration",
                )
            })?;
        if operation.source().len() > limits.max_source_bytes {
            return Err(limit("source byte limit exceeded"));
        }
        operation.modules().check_limit(limits)?;
        let memory = limits.max_memory.unwrap_or(self.default_memory_bytes);
        if memory == 0 || memory > usize::MAX - MAX_FRAME_BYTES {
            return Err(RuntimeError::new(
                crate::ErrorCode::InvalidArgument,
                "invalid worker memory limit",
            ));
        }
        let mut limits = limits.clone();
        limits.max_memory = Some(memory);
        // Serialize before spawning so an invalid/oversized request cannot leave a child.
        let request = encode(
            &WorkerCommand {
                operation,
                limits: limits.clone(),
            },
            self.max_frame_bytes,
        )?;
        let encoded = Instant::now();
        if Instant::now() >= deadline {
            return Err(limit("execution deadline exceeded"));
        }
        let mut child = Command::new(&self.executable)
            .args([
                "--worker",
                &memory.to_string(),
                &self.max_frame_bytes.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(io_error)?;
        let mut input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        let guard = WorkerGuard::new(child, output, self.max_frame_bytes, deadline)?;
        if let Err(error) = write_bytes(&mut input, &request) {
            return Err(guard.failure(error));
        }
        let spawned = Instant::now();
        let mut host_elapsed = std::time::Duration::ZERO;
        let mut calls = 0;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(limit("execution deadline exceeded"));
            }
            let message = match guard
                .messages
                .as_ref()
                .expect("receiver")
                .recv_timeout(remaining)
            {
                Ok(Ok(message)) => message,
                Ok(Err(error)) => return Err(guard.failure(error)),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(limit("execution deadline exceeded"));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(guard.failure(protocol("worker channel closed")));
                }
            };
            if Instant::now() >= deadline {
                return Err(limit("execution deadline exceeded"));
            }
            match message {
                Message::HostCall { name, args, kwargs } => {
                    if crate::HostFunction::parse(&name)
                        .is_none_or(|function| matches!(function, crate::HostFunction::Config))
                    {
                        return Err(protocol("unknown worker host function"));
                    }
                    if calls >= limits.max_calls {
                        return Err(limit("host call limit exceeded"));
                    }
                    calls += 1;
                    // The watchdog kills the child even if this callback is blocked.
                    // Rust cannot preempt a borrowed synchronous callback; its owner
                    // must enforce deadlines on blocking DB or external operations.
                    let callback_started = Instant::now();
                    let result = host.call(&name, args, kwargs)?;
                    host_elapsed += callback_started.elapsed();
                    if Instant::now() >= deadline {
                        return Err(limit("execution deadline exceeded"));
                    }
                    let reply = encode(
                        &HostReply::HostResult { value: result },
                        self.max_frame_bytes,
                    )?;
                    if let Err(error) = write_bytes(&mut input, &reply) {
                        return Err(guard.failure(error));
                    }
                }
                Message::ConfigCall => {
                    if calls >= limits.max_calls {
                        return Err(limit("host call limit exceeded"));
                    }
                    calls += 1;
                    let config = host.config()?;
                    if Instant::now() >= deadline {
                        return Err(limit("execution deadline exceeded"));
                    }
                    let reply = encode(&HostReply::ConfigResult { config }, self.max_frame_bytes)?;
                    if let Err(error) = write_bytes(&mut input, &reply) {
                        return Err(guard.failure(error));
                    }
                }
                Message::Complete { result } => {
                    tracing::debug!(target: "statemcp::timing", phase = "worker", %symbol,
                        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                        encode_ms = encoded.duration_since(started).as_secs_f64() * 1000.0,
                        spawn_ms = spawned.duration_since(encoded).as_secs_f64() * 1000.0,
                        host_ms = host_elapsed.as_secs_f64() * 1000.0,
                        calls, frame_bytes = request.len(), "worker completed");
                    return Ok(result);
                }
                Message::Error { error } => return Err(error),
            }
        }
    }
}

pub fn execute_isolated(
    config: &WorkerConfig,
    source: &str,
    inputs: Value,
    limits: &Limits,
    host: &mut HostCallback<'_>,
) -> Result<RunResult, RuntimeError> {
    config.execute(source, inputs, limits, host)
}
pub fn invoke_isolated(
    config: &WorkerConfig,
    source: &str,
    symbol: &str,
    arguments: serde_json::Map<String, Value>,
    limits: &Limits,
    host: &mut HostCallback<'_>,
) -> Result<RunResult, RuntimeError> {
    config.invoke(source, symbol, arguments, limits, host)
}
pub fn validate_module_isolated(
    config: &WorkerConfig,
    source: &str,
    symbol: &str,
    limits: &Limits,
) -> Result<(), RuntimeError> {
    config.validate_module(source, symbol, limits)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    ValidateHttp {
        source: String,
        modules: ModuleSources,
        symbol: String,
    },
    InvokeHttp {
        source: String,
        modules: ModuleSources,
        symbol: String,
        request: HttpRequest,
    },
    Execute {
        source: String,
        modules: ModuleSources,
        inputs: Value,
    },
    Invoke {
        source: String,
        modules: ModuleSources,
        symbol: String,
        arguments: serde_json::Map<String, Value>,
    },
    Validate {
        source: String,
        modules: ModuleSources,
        symbol: String,
    },
}
impl Operation {
    fn modules(&self) -> &ModuleSources {
        match self {
            Self::ValidateHttp { modules, .. }
            | Self::InvokeHttp { modules, .. }
            | Self::Execute { modules, .. }
            | Self::Invoke { modules, .. }
            | Self::Validate { modules, .. } => modules,
        }
    }
    fn source(&self) -> &str {
        match self {
            Self::ValidateHttp { source, .. }
            | Self::InvokeHttp { source, .. }
            | Self::Execute { source, .. }
            | Self::Invoke { source, .. }
            | Self::Validate { source, .. } => source,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerCommand {
    operation: Operation,
    limits: Limits,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "result", rename_all = "snake_case")]
enum Completion {
    Json(RunResult),
    Http(HttpRunResult),
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Message {
    ConfigCall,
    HostCall {
        name: String,
        args: Vec<Value>,
        kwargs: Map<String, Value>,
    },
    Complete {
        result: Completion,
    },
    Error {
        error: RuntimeError,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HostReply {
    HostResult {
        value: Value,
    },
    ConfigResult {
        config: state_contract::execution::PublicConfig,
    },
}

/// Entry point for a fresh, dedicated worker with LimitedAllocator installed.
/// Never call this inside a long-lived parent: arming the allocator is global.
pub fn worker_main(memory_bytes: usize, max_frame_bytes: usize) -> Result<(), RuntimeError> {
    validate_frame_limit(max_frame_bytes)?;
    if memory_bytes == 0 || memory_bytes > usize::MAX - MAX_FRAME_BYTES {
        return Err(RuntimeError::new(
            crate::ErrorCode::InvalidArgument,
            "invalid worker memory limit",
        ));
    }
    crate::arm_worker_memory_limit(memory_bytes)?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut request: WorkerCommand = read_frame(&mut input, max_frame_bytes)?;
    // The command-line ceiling is authoritative, including for malformed clients.
    request.limits.max_memory = Some(memory_bytes);
    serve_command(request, &mut input, &mut output, max_frame_bytes)
}

// Translate only at the worker's external framed I/O boundary.
struct FramedHost<'a, R, W> {
    input: &'a mut R,
    output: &'a mut W,
    max_frame_bytes: usize,
}
impl<R: Read, W: Write> crate::HostServices for FramedHost<'_, R, W> {
    fn config(&mut self) -> Result<state_contract::execution::PublicConfig, RuntimeError> {
        write_frame(self.output, &Message::ConfigCall, self.max_frame_bytes)?;
        match read_frame(self.input, self.max_frame_bytes)? {
            HostReply::ConfigResult { config } => Ok(config),
            HostReply::HostResult { .. } => Err(protocol("expected configuration reply")),
        }
    }
    fn call(
        &mut self,
        name: &str,
        args: Vec<Value>,
        kwargs: Map<String, Value>,
    ) -> Result<Value, RuntimeError> {
        write_frame(
            self.output,
            &Message::HostCall {
                name: name.into(),
                args,
                kwargs,
            },
            self.max_frame_bytes,
        )?;
        match read_frame(self.input, self.max_frame_bytes)? {
            HostReply::HostResult { value } => Ok(value),
            HostReply::ConfigResult { .. } => Err(protocol("unexpected configuration reply")),
        }
    }
}

// The shell installs the process-global allocator and chooses the authoritative
// memory ceiling. This framed session owns no OS handles and can use byte buffers.
fn serve_command(
    request: WorkerCommand,
    input: &mut impl Read,
    output: &mut impl Write,
    max_frame_bytes: usize,
) -> Result<(), RuntimeError> {
    let mut host = FramedHost {
        input,
        output,
        max_frame_bytes,
    };
    let result = match request.operation {
        Operation::ValidateHttp {
            source,
            symbol,
            modules,
        } => crate::validate_http(&source, &symbol, &modules, &request.limits).map(|()| {
            Completion::Json(RunResult {
                value: Value::Null,
                stdout: String::new(),
            })
        }),
        Operation::InvokeHttp {
            source,
            symbol,
            modules,
            request: http_request,
        } => crate::invoke_http(
            &source,
            &symbol,
            &modules,
            http_request,
            &request.limits,
            &mut host,
        )
        .map(Completion::Http),
        Operation::Execute {
            source,
            inputs,
            modules,
        } => crate::execute_with_modules(&source, inputs, &modules, &request.limits, &mut host)
            .map(Completion::Json),
        Operation::Invoke {
            source,
            symbol,
            arguments,
            modules,
        } => crate::invoke_with_modules(
            &source,
            &symbol,
            arguments,
            &modules,
            &request.limits,
            &mut host,
        )
        .map(Completion::Json),
        Operation::Validate {
            source,
            symbol,
            modules,
        } => crate::validate_module_with_modules(&source, &symbol, &modules, &request.limits).map(
            |()| {
                Completion::Json(RunResult {
                    value: Value::Null,
                    stdout: String::new(),
                })
            },
        ),
    };
    let message = match result {
        Ok(result) => Message::Complete { result },
        Err(error) => Message::Error { error },
    };
    // An oversized final frame becomes a small structured error when possible.
    match encode(&message, max_frame_bytes) {
        Ok(bytes) => write_bytes(output, &bytes),
        Err(error) => write_frame(output, &Message::Error { error }, max_frame_bytes),
    }
}

/// Process lifecycle operations used by the watchdog and cleanup guard.
/// Keep exit-code interpretation at the OS boundary.
trait WorkerProcess: Send + 'static {
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<WorkerExit>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerExit {
    MemoryLimit,
    Other,
}
impl WorkerProcess for Child {
    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }
    fn wait(&mut self) -> io::Result<WorkerExit> {
        Child::wait(self).map(|status| {
            if status.code() == Some(65) {
                WorkerExit::MemoryLimit
            } else {
                WorkerExit::Other
            }
        })
    }
}

struct WorkerGuard<P: WorkerProcess = Child> {
    child: Arc<Mutex<P>>,
    expired: Arc<AtomicBool>,
    cancel: Option<mpsc::Sender<()>>,
    watchdog: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    messages: Option<mpsc::Receiver<Result<Message, RuntimeError>>>,
}
impl<P: WorkerProcess> WorkerGuard<P> {
    fn new(
        child: P,
        mut output: impl Read + Send + 'static,
        max: usize,
        deadline: Instant,
    ) -> Result<Self, RuntimeError> {
        let child = Arc::new(Mutex::new(child));
        let expired = Arc::new(AtomicBool::new(false));
        let (cancel, cancelled) = mpsc::channel();
        // Guard exists before thread creation, so spawn failures also reap.
        let mut guard = Self {
            child,
            expired,
            cancel: Some(cancel),
            watchdog: None,
            reader: None,
            messages: None,
        };
        let process = guard.child.clone();
        let expired = guard.expired.clone();
        guard.watchdog = Some(
            thread::Builder::new()
                .name("state-worker-deadline".into())
                .spawn(move || {
                    if matches!(
                        cancelled.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        expired.store(true, Ordering::Release);
                        reap(&process);
                    }
                })
                .map_err(io_error)?,
        );
        let (send, receive) = mpsc::sync_channel(1);
        guard.messages = Some(receive);
        guard.reader = Some(
            thread::Builder::new()
                .name("state-worker-frames".into())
                .spawn(move || {
                    loop {
                        let frame = read_frame(&mut output, max);
                        let done =
                            !matches!(&frame, Ok(Message::HostCall { .. } | Message::ConfigCall));
                        if send.send(frame).is_err() || done {
                            break;
                        }
                    }
                })
                .map_err(io_error)?,
        );
        Ok(guard)
    }
    fn failure(&self, original: RuntimeError) -> RuntimeError {
        let status = reap(&self.child);
        if self.expired.load(Ordering::Acquire) {
            limit("execution deadline exceeded")
        } else if status == Some(WorkerExit::MemoryLimit) {
            limit("worker memory limit exceeded")
        } else {
            original
        }
    }
}
impl<P: WorkerProcess> Drop for WorkerGuard<P> {
    fn drop(&mut self) {
        drop(self.messages.take());
        reap(&self.child);
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
    }
}
fn reap<P: WorkerProcess>(child: &Mutex<P>) -> Option<WorkerExit> {
    let mut child = child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Cleanup is best-effort: the process may already be gone. If reaping fails,
    // retain the original execution/protocol error rather than inventing an exit.
    let _ = child.kill();
    child.wait().ok()
}
fn validate_frame_limit(max: usize) -> Result<(), RuntimeError> {
    if !(1024..=MAX_FRAME_BYTES).contains(&max) {
        Err(RuntimeError::new(
            crate::ErrorCode::InvalidArgument,
            "worker frame limit must be between 1 KiB and 64 MiB",
        ))
    } else {
        Ok(())
    }
}
fn read_frame<T: DeserializeOwned>(input: &mut impl Read, max: usize) -> Result<T, RuntimeError> {
    let mut header = [0; 4];
    input.read_exact(&mut header).map_err(io_error)?;
    let len = u32::from_be_bytes(header) as usize;
    if len > max {
        return Err(limit("worker frame byte limit exceeded"));
    }
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes).map_err(io_error)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| protocol(format!("invalid worker frame: {error}")))
}
struct BoundedBytes {
    bytes: Vec<u8>,
    max: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.max.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("worker frame byte limit exceeded"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize, max: usize) -> Result<Vec<u8>, RuntimeError> {
    let mut writer = BoundedBytes {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut writer, value).map_err(|error| limit(error.to_string()))?;
    Ok(writer.bytes)
}
fn write_frame(
    output: &mut impl Write,
    value: &impl Serialize,
    max: usize,
) -> Result<(), RuntimeError> {
    write_bytes(output, &encode(value, max)?)
}
fn write_bytes(output: &mut impl Write, bytes: &[u8]) -> Result<(), RuntimeError> {
    output
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .map_err(io_error)?;
    output.write_all(bytes).map_err(io_error)?;
    output.flush().map_err(io_error)
}
fn protocol(message: impl Into<String>) -> RuntimeError {
    RuntimeError::new(crate::ErrorCode::WorkerFailed, message)
}
fn io_error(error: io::Error) -> RuntimeError {
    protocol(format!("worker I/O failed: {error}"))
}
fn limit(message: impl Into<String>) -> RuntimeError {
    RuntimeError::new(crate::ErrorCode::LimitExceeded, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn child_frame_decoder_rejects_malformed_truncated_and_oversized_messages() {
        for bytes in [
            vec![0, 0, 0, 0],
            vec![0, 0, 0, 1, b'{'],
            vec![0, 0, 0, 2, b'{'],
            vec![255, 255, 255, 255],
        ] {
            assert!(read_frame::<Message>(&mut bytes.as_slice(), 1024).is_err());
        }
        for body in [
            r#"{"type":"unknown"}"#,
            r#"{"type":"host_call","name":"mcp","args":[],"kwargs":{},"extra":true}"#,
            r#"{"type":"complete","result":{"value":null,"stdout":""}}trailing"#,
        ] {
            let mut frame = (body.len() as u32).to_be_bytes().to_vec();
            frame.extend_from_slice(body.as_bytes());
            assert_eq!(
                read_frame::<Message>(&mut frame.as_slice(), 1024)
                    .err()
                    .unwrap()
                    .code,
                crate::ErrorCode::WorkerFailed
            );
        }
    }

    #[derive(Default)]
    struct ProcessState {
        killed: bool,
        reaped: bool,
    }
    struct FakeProcess {
        state: Arc<Mutex<ProcessState>>,
        exit: WorkerExit,
    }
    impl WorkerProcess for FakeProcess {
        fn kill(&mut self) -> io::Result<()> {
            self.state
                .lock()
                .map_err(|_| io::Error::other("poisoned fake process"))?
                .killed = true;
            Ok(())
        }
        fn wait(&mut self) -> io::Result<WorkerExit> {
            self.state
                .lock()
                .map_err(|_| io::Error::other("poisoned fake process"))?
                .reaped = true;
            Ok(self.exit)
        }
    }
    fn fake(exit: WorkerExit) -> (FakeProcess, Arc<Mutex<ProcessState>>) {
        let state = Arc::new(Mutex::new(ProcessState::default()));
        (
            FakeProcess {
                state: state.clone(),
                exit,
            },
            state,
        )
    }
    #[test]
    fn watchdog_reaps_without_dispatch_or_callback_progress()
    -> Result<(), Box<dyn std::error::Error>> {
        let (child, state) = fake(WorkerExit::Other);
        let mut guard = WorkerGuard::new(child, io::Cursor::new(Vec::new()), 1024, Instant::now())?;
        let watchdog = guard.watchdog.take().ok_or("missing watchdog")?;
        watchdog.join().map_err(|_| "watchdog panicked")?;
        assert!(guard.expired.load(Ordering::Acquire));
        let state = state.lock().map_err(|_| "poisoned fake process")?;
        assert!(state.killed && state.reaped);
        drop(state);
        assert_eq!(
            guard.failure(protocol("original")).code,
            crate::ErrorCode::LimitExceeded
        );
        Ok(())
    }
    #[test]
    fn dropping_guard_kills_reaps_and_cancels_watchdog() -> Result<(), Box<dyn std::error::Error>> {
        let (child, state) = fake(WorkerExit::Other);
        let guard = WorkerGuard::new(
            child,
            io::Cursor::new(Vec::new()),
            1024,
            Instant::now() + Duration::from_secs(30),
        )?;
        drop(guard);
        let state = state.lock().map_err(|_| "poisoned fake process")?;
        assert!(state.killed && state.reaped);
        Ok(())
    }
    #[test]
    fn memory_exit_maps_to_limit_error_and_other_exits_preserve_original()
    -> Result<(), Box<dyn std::error::Error>> {
        for exit in [WorkerExit::MemoryLimit, WorkerExit::Other] {
            let (child, _) = fake(exit);
            let guard = WorkerGuard::new(
                child,
                io::Cursor::new(Vec::new()),
                1024,
                Instant::now() + Duration::from_secs(30),
            )?;
            let error = guard.failure(protocol("original"));
            assert_eq!(
                error.code,
                match exit {
                    WorkerExit::MemoryLimit => crate::ErrorCode::LimitExceeded,
                    WorkerExit::Other => crate::ErrorCode::WorkerFailed,
                }
            );
        }
        Ok(())
    }

    fn command(source: &str) -> WorkerCommand {
        WorkerCommand {
            operation: Operation::Execute {
                source: source.into(),
                inputs: Value::Null,
                modules: ModuleSources::default(),
            },
            limits: Limits::default(),
        }
    }
    #[test]
    fn framed_execute_invoke_and_validation_roundtrip() -> Result<(), RuntimeError> {
        let operations = [
            (
                command("print('captured')\n42").operation,
                serde_json::json!(42),
                "captured\n",
            ),
            (
                Operation::Invoke {
                    source: "def add(a, b):\n    return a + b".into(),
                    symbol: "add".into(),
                    arguments: Map::from_iter([
                        ("a".into(), Value::from(2)),
                        ("b".into(), Value::from(3)),
                    ]),
                    modules: ModuleSources::default(),
                },
                serde_json::json!(5),
                "",
            ),
            (
                Operation::Validate {
                    source: "def add():\n    return 42".into(),
                    symbol: "add".into(),
                    modules: ModuleSources::default(),
                },
                Value::Null,
                "",
            ),
        ];
        for (operation, expected, stdout) in operations {
            let request = WorkerCommand {
                operation,
                limits: Limits::default(),
            };
            let mut encoded = Vec::new();
            write_frame(&mut encoded, &request, 1024)?;
            let decoded = read_frame(&mut encoded.as_slice(), 1024)?;
            let mut output = Vec::new();
            serve_command(decoded, &mut io::empty(), &mut output, 1024)?;
            let Message::Complete {
                result: Completion::Json(result),
            } = read_frame(&mut output.as_slice(), 1024)?
            else {
                return Err(protocol("expected completion"));
            };
            assert_eq!(result.value, expected);
            assert_eq!(result.stdout, stdout);
        }
        Ok(())
    }
    #[test]
    fn http_worker_frames_preserve_binary_request_and_typed_response() -> Result<(), RuntimeError> {
        let request = state_contract::http::HttpRequest {
            method: state_contract::http::Method::parse("CUSTOM")
                .map_err(|e| protocol(e.message))?,
            path: "/web/echo".into(),
            query: String::new(),
            host: "localhost".into(),
            scheme: "http".into(),
            headers: Vec::new(),
            body: vec![0, 255, 10],
            view_args: Default::default(),
        };
        let command=WorkerCommand{operation:Operation::InvokeHttp{source:"def handle(request):\n    return request.body, 202, [('X-Method',request.method)]".into(),symbol:"handle".into(),modules:ModuleSources::default(),request},limits:Limits::default()};
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &command, 4096)?;
        let decoded = read_frame(&mut encoded.as_slice(), 4096)?;
        let mut output = Vec::new();
        serve_command(decoded, &mut io::empty(), &mut output, 4096)?;
        let Message::Complete {
            result: Completion::Http(result),
        } = read_frame(&mut output.as_slice(), 4096)?
        else {
            return Err(protocol("expected HTTP completion"));
        };
        assert_eq!(result.response.status(), 202);
        assert_eq!(result.response.body(), [0, 255, 10]);
        assert!(
            result
                .response
                .headers()
                .iter()
                .any(|(k, v)| k == "x-method" && v == "CUSTOM")
        );
        Ok(())
    }
    #[test]
    fn malformed_host_reply_ends_session_without_second_callback() -> Result<(), RuntimeError> {
        for reply in [
            serde_json::json!({"type":"host_result","value":null,"extra":true}),
            serde_json::json!({"type":"complete","result":{"value":null,"stdout":""}}),
        ] {
            let mut input = Vec::new();
            write_frame(&mut input, &reply, 1024)?;
            let mut output = Vec::new();
            serve_command(
                command("try:\n    mcp('first')\nexcept Exception:\n    pass\nmcp('later')"),
                &mut input.as_slice(),
                &mut output,
                1024,
            )?;
            let mut frames = output.as_slice();
            let Message::HostCall { name, args, .. } = read_frame(&mut frames, 1024)? else {
                return Err(protocol("expected host call"));
            };
            assert_eq!(name, "mcp");
            assert_eq!(args, vec![Value::from("first")]);
            let Message::Error { error } = read_frame(&mut frames, 1024)? else {
                return Err(protocol("expected error"));
            };
            assert_eq!(error.code, crate::ErrorCode::WorkerFailed);
            assert!(frames.is_empty());
        }
        Ok(())
    }
    #[test]
    fn framed_config_uses_typed_reply_and_rejects_json_reply() -> Result<(), RuntimeError> {
        for config in [
            state_contract::execution::PublicConfig::default(),
            state_contract::execution::PublicConfig {
                public_url: Some("https://example.com".into()),
                prefix: "/prefix".into(),
            },
        ] {
            let expected = serde_json::json!([config.public_url, config.prefix]);
            let mut input = Vec::new();
            write_frame(&mut input, &HostReply::ConfigResult { config }, 4096)?;
            let mut output = Vec::new();
            serve_command(
                command("c = config()\n[c.public_url, c.prefix]"),
                &mut input.as_slice(),
                &mut output,
                4096,
            )?;
            let mut frames = output.as_slice();
            assert!(matches!(
                read_frame::<Message>(&mut frames, 4096)?,
                Message::ConfigCall
            ));
            let Message::Complete {
                result: Completion::Json(result),
            } = read_frame(&mut frames, 4096)?
            else {
                return Err(protocol("expected config completion"));
            };
            assert_eq!(result.value, expected);
            assert!(frames.is_empty());
        }
        // A JSON-shaped record cannot masquerade as the typed configuration reply.
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &HostReply::HostResult {
                value: serde_json::json!({"public_url":null,"prefix":""}),
            },
            4096,
        )?;
        let mut output = Vec::new();
        serve_command(
            command("try:\n    config()\nexcept Exception:\n    pass\nmcp('later')"),
            &mut input.as_slice(),
            &mut output,
            4096,
        )?;
        let mut frames = output.as_slice();
        assert!(matches!(
            read_frame::<Message>(&mut frames, 4096)?,
            Message::ConfigCall
        ));
        let Message::Error { error } = read_frame(&mut frames, 4096)? else {
            return Err(protocol("expected configuration protocol error"));
        };
        assert_eq!(error.code, crate::ErrorCode::WorkerFailed);
        assert!(frames.is_empty());
        Ok(())
    }

    #[test]
    fn host_results_and_oversized_completion_are_framed() -> Result<(), RuntimeError> {
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &HostReply::HostResult {
                value: Value::from(41),
            },
            1024,
        )?;
        let mut output = Vec::new();
        serve_command(
            command("mcp('answer') + 1"),
            &mut input.as_slice(),
            &mut output,
            1024,
        )?;
        let mut frames = output.as_slice();
        assert!(matches!(
            read_frame::<Message>(&mut frames, 1024)?,
            Message::HostCall { .. }
        ));
        let Message::Complete {
            result: Completion::Json(result),
        } = read_frame(&mut frames, 1024)?
        else {
            return Err(protocol("expected completion"));
        };
        assert_eq!(result.value, Value::from(42));
        assert!(frames.is_empty());
        let mut output = Vec::new();
        serve_command(command("'x' * 2048"), &mut io::empty(), &mut output, 1024)?;
        let Message::Error { error } = read_frame(&mut output.as_slice(), 1024)? else {
            return Err(protocol("expected frame error"));
        };
        assert_eq!(error.code, crate::ErrorCode::LimitExceeded);
        Ok(())
    }
    #[test]
    fn invalid_and_oversized_requests_fail_before_process_spawn() {
        let config = WorkerConfig::new("/not-used");
        let no_host = &mut |_: &str, _: Vec<Value>, _: Map<String, Value>| {
            Err(protocol("unexpected callback"))
        };
        let error = config
            .execute(
                "12345",
                Value::Null,
                &Limits {
                    max_source_bytes: 4,
                    ..Limits::default()
                },
                no_host,
            )
            .unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::LimitExceeded);
        let mut config = config;
        config.max_frame_bytes = 1024;
        let error = config
            .execute(
                "inputs",
                Value::from("x".repeat(2048)),
                &Limits::default(),
                no_host,
            )
            .unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::LimitExceeded);
        config.default_memory_bytes = 0;
        let error = config
            .execute("42", Value::Null, &Limits::default(), no_host)
            .unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::InvalidArgument);
        assert!(validate_frame_limit(1023).is_err());
        assert!(validate_frame_limit(MAX_FRAME_BYTES + 1).is_err());
    }
}

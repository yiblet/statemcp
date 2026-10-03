//! Disposable same-executable workers. The parent alone owns host capabilities.
use crate::{HostCallback, Limits, RunResult, RuntimeError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
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
        self.run(
            Operation::Execute {
                source: source.into(),
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
        arguments: Value,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        self.run(
            Operation::Invoke {
                source: source.into(),
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
        self.run(
            Operation::Validate {
                source: source.into(),
                symbol: symbol.into(),
            },
            limits,
            &mut |_, _, _| {
                Err(RuntimeError::new(
                    "PERMISSION_DENIED",
                    "module initialization cannot call host functions",
                ))
            },
        )
        .map(|_| ())
    }
    fn run(
        &self,
        operation: Operation,
        limits: &Limits,
        host: &mut HostCallback<'_>,
    ) -> Result<RunResult, RuntimeError> {
        validate_frame_limit(self.max_frame_bytes)?;
        let deadline = Instant::now()
            .checked_add(limits.max_duration)
            .ok_or_else(|| RuntimeError::new("INVALID_ARGUMENT", "invalid execution duration"))?;
        if operation.source().len() > limits.max_source_bytes {
            return Err(limit("source byte limit exceeded"));
        }
        let memory = limits.max_memory.unwrap_or(self.default_memory_bytes);
        if memory == 0 || memory > usize::MAX - MAX_FRAME_BYTES {
            return Err(RuntimeError::new(
                "INVALID_ARGUMENT",
                "invalid worker memory limit",
            ));
        }
        let mut limits = limits.clone();
        limits.max_memory = Some(memory);
        // Serialize before spawning so an invalid/oversized request cannot leave a child.
        let request = encode(
            &Request {
                operation,
                limits: limits.clone(),
            },
            self.max_frame_bytes,
        )?;
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
                    if !crate::HOST_FUNCTIONS.contains(&name.as_str()) {
                        return Err(protocol("unknown worker host function"));
                    }
                    if calls >= limits.max_calls {
                        return Err(limit("host call limit exceeded"));
                    }
                    calls += 1;
                    // The watchdog kills the child even if this callback is blocked.
                    // Rust cannot preempt a borrowed synchronous callback; its owner
                    // must enforce deadlines on blocking DB or external operations.
                    let result = host(&name, args, kwargs)?;
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
                Message::Complete { result } => return Ok(result),
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
    arguments: Value,
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
    Execute {
        source: String,
        inputs: Value,
    },
    Invoke {
        source: String,
        symbol: String,
        arguments: Value,
    },
    Validate {
        source: String,
        symbol: String,
    },
}
impl Operation {
    fn source(&self) -> &str {
        match self {
            Self::Execute { source, .. }
            | Self::Invoke { source, .. }
            | Self::Validate { source, .. } => source,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: Operation,
    limits: Limits,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Message {
    HostCall {
        name: String,
        args: Vec<Value>,
        kwargs: Map<String, Value>,
    },
    Complete {
        result: RunResult,
    },
    Error {
        error: RuntimeError,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HostReply {
    HostResult { value: Value },
}

/// Entry point for a fresh, dedicated worker with LimitedAllocator installed.
/// Never call this inside a long-lived parent: arming the allocator is global.
pub fn worker_main(memory_bytes: usize, max_frame_bytes: usize) -> Result<(), RuntimeError> {
    validate_frame_limit(max_frame_bytes)?;
    if memory_bytes == 0 || memory_bytes > usize::MAX - MAX_FRAME_BYTES {
        return Err(RuntimeError::new(
            "INVALID_ARGUMENT",
            "invalid worker memory limit",
        ));
    }
    crate::arm_worker_memory_limit(memory_bytes)?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut request: Request = read_frame(&mut input, max_frame_bytes)?;
    // The command-line ceiling is authoritative, including for malformed clients.
    request.limits.max_memory = Some(memory_bytes);
    let mut host = |name: &str, args: Vec<Value>, kwargs: Map<String, Value>| {
        write_frame(
            &mut output,
            &Message::HostCall {
                name: name.into(),
                args,
                kwargs,
            },
            max_frame_bytes,
        )?;
        let HostReply::HostResult { value } = read_frame(&mut input, max_frame_bytes)?;
        Ok(value)
    };
    let result = match request.operation {
        Operation::Execute { source, inputs } => {
            crate::execute(&source, inputs, &request.limits, &mut host)
        }
        Operation::Invoke {
            source,
            symbol,
            arguments,
        } => crate::invoke(&source, &symbol, arguments, &request.limits, &mut host),
        Operation::Validate { source, symbol } => {
            crate::validate_module(&source, &symbol, &request.limits).map(|()| RunResult {
                value: Value::Null,
                stdout: String::new(),
            })
        }
    };
    let message = match result {
        Ok(result) => Message::Complete { result },
        Err(error) => Message::Error { error },
    };
    // An oversized final frame becomes a small structured error when possible.
    match encode(&message, max_frame_bytes) {
        Ok(bytes) => write_bytes(&mut output, &bytes),
        Err(error) => write_frame(&mut output, &Message::Error { error }, max_frame_bytes),
    }
}

struct WorkerGuard {
    child: Arc<Mutex<Child>>,
    expired: Arc<AtomicBool>,
    cancel: Option<mpsc::Sender<()>>,
    watchdog: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    messages: Option<mpsc::Receiver<Result<Message, RuntimeError>>>,
}
impl WorkerGuard {
    fn new(
        child: Child,
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
                        let done = !matches!(&frame, Ok(Message::HostCall { .. }));
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
        } else if status.is_some_and(|status| status.code() == Some(65)) {
            limit("worker memory limit exceeded")
        } else {
            original
        }
    }
}
impl Drop for WorkerGuard {
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
fn reap(child: &Mutex<Child>) -> Option<ExitStatus> {
    let mut child = child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = child.kill();
    child.wait().ok()
}
fn validate_frame_limit(max: usize) -> Result<(), RuntimeError> {
    if !(1024..=MAX_FRAME_BYTES).contains(&max) {
        Err(RuntimeError::new(
            "INVALID_ARGUMENT",
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
    RuntimeError::new("WORKER_FAILED", message)
}
fn io_error(error: io::Error) -> RuntimeError {
    protocol(format!("worker I/O failed: {error}"))
}
fn limit(message: impl Into<String>) -> RuntimeError {
    RuntimeError::new("LIMIT_EXCEEDED", message)
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
                "WORKER_FAILED"
            );
        }
    }

    fn stubborn_child() -> (Child, std::process::ChildStdout) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "worker::tests::stubborn_child_entry",
                "--nocapture",
            ])
            .env("STATE_RUNTIME_STUBBORN_CHILD", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        (child, output)
    }
    #[test]
    fn stubborn_child_entry() {
        if std::env::var_os("STATE_RUNTIME_STUBBORN_CHILD").is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }
    #[test]
    fn watchdog_kills_and_reaps_without_cooperative_vm_or_callback() {
        let (child, output) = stubborn_child();
        let guard = WorkerGuard::new(
            child,
            output,
            1024,
            Instant::now() + Duration::from_millis(30),
        )
        .unwrap();
        // Simulates a parent callback that has not returned control to dispatch.
        std::thread::sleep(Duration::from_millis(100));
        assert!(guard.expired.load(Ordering::Acquire));
        assert!(guard.child.lock().unwrap().try_wait().unwrap().is_some());
    }
    #[test]
    fn dropping_guard_reaps_stubborn_child_immediately() {
        let (child, output) = stubborn_child();
        let guard = WorkerGuard::new(
            child,
            output,
            1024,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        let child = guard.child.clone();
        let started = Instant::now();
        drop(guard);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(child.lock().unwrap().try_wait().unwrap().is_some());
    }
}

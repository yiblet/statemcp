//! Embedded, fresh-scope Monty execution with a narrow JSON host boundary.
//!
//! This library is not a process sandbox. See the crate README for the allocator
//! integration required for aggregate memory accounting and worker hard limits.

use monty::{MontyRepl, ReplProgress};
use monty_types::{
    CompileOptions, ExcType, MontyException, MontyObject, NamedValues, ObjectRef, PrintWriter,
    PrintWriterCallback, ResourceLimits, ResourceTracker,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    borrow::Cow,
    fmt,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

pub use monty_alloc::LimitedAllocator;

mod worker;
pub use worker::{
    WorkerConfig, execute_isolated, invoke_isolated, validate_module_isolated, worker_main,
};

pub const HOST_FUNCTIONS: &[&str] = &[
    "mcp",
    "call",
    "db_query",
    "db_execute",
    "db_inspect",
    "read_text",
    "write_text",
];

pub type HostCallback<'a> =
    dyn FnMut(&str, Vec<Value>, Map<String, Value>) -> Result<Value, RuntimeError> + 'a;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub max_duration: Duration,
    /// Aggregate memory cap. Requires an installed and armed LimitedAllocator.
    /// None explicitly means aggregate memory is not bounded by this library.
    pub max_memory: Option<usize>,
    /// Always applied to Monty's allocation preflight checks. Does not bound
    /// the sum of many small allocations without allocator integration.
    pub max_allocation_bytes: usize,
    pub max_calls: usize,
    pub max_recursion_depth: usize,
    pub max_source_bytes: usize,
    pub max_output_bytes: usize,
    pub max_json_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_duration: Duration::from_secs(5),
            max_memory: None,
            max_allocation_bytes: 64 * 1024 * 1024,
            max_calls: 256,
            max_recursion_depth: 100,
            max_source_bytes: 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            max_json_depth: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
}

impl RuntimeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for RuntimeError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub value: Value,
    /// Captured print output (stdout and stderr in emission order).
    pub stdout: String,
}

/// Arm allocator accounting and a hard memory ceiling in a dedicated worker.
///
/// The executable must install `LimitedAllocator` as its global allocator. This
/// ceiling is PROCESS GLOBAL and exceeding it terminates the process with code
/// 65. Never use this to isolate a session in a shared server process. Call once
/// on a fresh worker before parsing inputs; use one worker per root invocation.
pub fn arm_worker_memory_limit(bytes: usize) -> Result<(), RuntimeError> {
    monty_alloc::set_hard_limit(monty_types::memory_limit_with_headroom(Some(bytes), false))
        .map_err(|message| RuntimeError::new("UNSUPPORTED_FEATURE", message))
}

pub fn memory_tracking_active() -> bool {
    monty_types::LIVE_MEMORY.load(Ordering::Relaxed) > 0
        && monty_types::BASELINE_MEMORY.load(Ordering::Relaxed) != usize::MAX
}

/// Execute a script with the supplied JSON bound to the `inputs` global.
pub fn execute(
    source: &str,
    inputs: Value,
    limits: &Limits,
    host: &mut HostCallback<'_>,
) -> Result<RunResult, RuntimeError> {
    execute_with_bindings(
        source,
        Map::from_iter([("inputs".into(), inputs)]),
        limits,
        host,
    )
}

/// Execute with explicit globals. Host function names cannot be overridden by
/// bindings; JSON is converted directly, never interpolated into Python source.
pub fn execute_with_bindings(
    source: &str,
    bindings: Map<String, Value>,
    limits: &Limits,
    host: &mut HostCallback<'_>,
) -> Result<RunResult, RuntimeError> {
    let mut session = Session::new(source, limits)?;
    let mut inputs = host_inputs();
    for (name, value) in bindings {
        if !identifier(&name) || HOST_FUNCTIONS.contains(&name.as_str()) {
            return Err(RuntimeError::new(
                "INVALID_ARGUMENT",
                "invalid or reserved binding name",
            ));
        }
        inputs.push(name, session.to_monty(&value)?);
    }
    let repl = session.repl();
    let progress = repl
        .feed_start(source, inputs, session.print())
        .map_err(repl_error)?;
    let (_, value) = session.drive(progress, Some(host))?;
    session.finish(value)
}

/// Compile and initialize a module in a fresh VM, denying all host effects, and
/// check that `symbol` names a callable. No endpoint is invoked.
pub fn validate_module(source: &str, symbol: &str, limits: &Limits) -> Result<(), RuntimeError> {
    let mut session = Session::new(source, limits)?;
    session.module(source, symbol)?;
    session.check_deadline()
}

/// Initialize the pinned module with effects denied, then invoke its function
/// with JSON object fields as keyword arguments in a second feed. The phase
/// boundary is controlled in Rust and cannot be advanced by Python code.
pub fn invoke(
    source: &str,
    symbol: &str,
    arguments: Value,
    limits: &Limits,
    host: &mut HostCallback<'_>,
) -> Result<RunResult, RuntimeError> {
    if !arguments.is_object() {
        return Err(RuntimeError::new(
            "INVALID_ARGUMENT",
            "endpoint arguments must be an object",
        ));
    }
    let mut session = Session::new(source, limits)?;
    let repl = session.module(source, symbol)?;
    let mut inputs = NamedValues::new();
    inputs.push("__state_arguments", session.to_monty(&arguments)?);
    // Symbol syntax is restricted to one identifier before interpolation.
    let source = format!("{symbol}(**__state_arguments)");
    let progress = repl
        .feed_start(&source, inputs, session.print())
        .map_err(repl_error)?;
    let (_, value) = session.drive(progress, Some(host))?;
    session.finish(value)
}

fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn host_inputs() -> NamedValues {
    let mut inputs = NamedValues::new();
    for name in HOST_FUNCTIONS {
        inputs.push(*name, MontyObject::function(*name, None));
    }
    inputs
}

struct BoundedOutput {
    text: String,
    max: usize,
    exceeded: bool,
}
impl BoundedOutput {
    fn append(&mut self, text: &str) -> Result<(), MontyException> {
        if self.text.len().saturating_add(text.len()) > self.max {
            self.exceeded = true;
            return Err(MontyException::new(
                ExcType::MemoryError,
                Some("print byte limit exceeded".into()),
            ));
        }
        self.text.push_str(text);
        Ok(())
    }
}
impl PrintWriterCallback for BoundedOutput {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.append(&output)
    }
    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.append(end.encode_utf8(&mut [0; 4]))
    }
}

struct Session<'a> {
    limits: &'a Limits,
    started: Instant,
    stdout: BoundedOutput,
    calls: usize,
}

impl<'a> Session<'a> {
    fn new(source: &str, limits: &'a Limits) -> Result<Self, RuntimeError> {
        if source.len() > limits.max_source_bytes {
            return Err(limit_error("source byte limit exceeded"));
        }
        if limits.max_memory.is_some() && !memory_tracking_active() {
            return Err(RuntimeError::new(
                "UNSUPPORTED_FEATURE",
                "aggregate memory limits require an installed and armed LimitedAllocator in a dedicated worker",
            ));
        }
        Ok(Self {
            limits,
            started: Instant::now(),
            stdout: BoundedOutput {
                text: String::new(),
                max: limits.max_output_bytes,
                exceeded: false,
            },
            calls: 0,
        })
    }

    fn repl(&self) -> MontyRepl {
        let limits = ResourceLimits {
            max_feed_duration: Some(self.limits.max_duration),
            max_turn_duration: Some(self.limits.max_duration),
            max_memory: Some(
                self.limits
                    .max_memory
                    .unwrap_or(self.limits.max_allocation_bytes),
            ),
            max_recursion_depth: self.limits.max_recursion_depth,
            max_suspensions: self.limits.max_calls,
            ..ResourceLimits::default()
        };
        MontyRepl::new(
            "state.py",
            ResourceTracker::new(limits),
            CompileOptions::default(),
        )
    }

    fn print(&mut self) -> PrintWriter<'_> {
        PrintWriter::Callback(&mut self.stdout)
    }

    fn check_deadline(&self) -> Result<(), RuntimeError> {
        if self.stdout.exceeded {
            return Err(limit_error("print byte limit exceeded"));
        }
        if self.started.elapsed() > self.limits.max_duration {
            Err(limit_error("execution deadline exceeded"))
        } else {
            Ok(())
        }
    }

    fn module(&mut self, source: &str, symbol: &str) -> Result<MontyRepl, RuntimeError> {
        if !identifier(symbol) || symbol == "__state_arguments" || HOST_FUNCTIONS.contains(&symbol)
        {
            return Err(RuntimeError::new(
                "INVALID_ARGUMENT",
                "endpoint symbol must be a non-reserved Python identifier",
            ));
        }
        let progress = self
            .repl()
            .feed_start(source, host_inputs(), self.print())
            .map_err(repl_error)?;
        let (repl, _) = self.drive(progress, None)?;
        if !repl.has_function(symbol) {
            return Err(RuntimeError::new(
                "INVALID_ARGUMENT",
                format!("module has no callable {symbol}"),
            ));
        }
        Ok(repl)
    }

    fn drive(
        &mut self,
        mut progress: ReplProgress,
        mut host: Option<&mut HostCallback<'_>>,
    ) -> Result<(MontyRepl, MontyObject), RuntimeError> {
        loop {
            self.check_deadline()?;
            progress = match progress {
                ReplProgress::Complete { repl, value } => return Ok((repl, value)),
                ReplProgress::FunctionCall(call) => {
                    let Some(host) = host.as_mut() else {
                        return Err(RuntimeError::new(
                            "PERMISSION_DENIED",
                            "module initialization cannot call host functions",
                        ));
                    };
                    if !HOST_FUNCTIONS.contains(&call.function_name.as_str())
                        || call.object_id.is_some()
                    {
                        return Err(RuntimeError::new(
                            "PERMISSION_DENIED",
                            "unknown host function",
                        ));
                    }
                    if self.calls >= self.limits.max_calls {
                        return Err(limit_error("host call limit exceeded"));
                    }
                    self.calls += 1;
                    let mut budget = JsonBudget::new(self.limits);
                    let args = call
                        .args
                        .args()
                        .map(|v| budget.decode(v, 0))
                        .collect::<Result<Vec<_>, _>>()?;
                    let kwargs = call
                        .args
                        .kwargs()
                        .map(|(k, v)| {
                            let key = k
                                .as_str()
                                .ok_or_else(|| invalid_result("keyword must be a string"))?;
                            budget.charge(key.len())?;
                            Ok((key.to_owned(), budget.decode(v, 0)?))
                        })
                        .collect::<Result<Map<_, _>, RuntimeError>>()?;
                    // Host errors terminate Rust execution. They are deliberately
                    // never resumed as catchable Python exceptions.
                    let result = host(&call.function_name, args, kwargs)?;
                    self.check_deadline()?;
                    let value = self.to_monty(&result)?;
                    call.resume(value, self.print()).map_err(repl_error)?
                }
                ReplProgress::OsCall(_) => {
                    return Err(RuntimeError::new(
                        "PERMISSION_DENIED",
                        "host filesystem and OS operations are disabled",
                    ));
                }
                ReplProgress::NameLookup(call) => {
                    return Err(RuntimeError::new(
                        "PYTHON_ERROR",
                        format!("undefined name: {}", call.name),
                    ));
                }
                ReplProgress::ResolveFutures(_) => {
                    return Err(RuntimeError::new(
                        "UNSUPPORTED_FEATURE",
                        "unresolved asynchronous host futures are unsupported",
                    ));
                }
            };
        }
    }

    fn to_monty(&self, value: &Value) -> Result<MontyObject, RuntimeError> {
        JsonBudget::new(self.limits).encode(value, 0)
    }

    fn finish(self, value: MontyObject) -> Result<RunResult, RuntimeError> {
        self.check_deadline()?;
        let value = JsonBudget::new(self.limits).decode(value.as_ref(), 0)?;
        // The incremental budget bounds allocations; the final serialized check
        // additionally accounts for escaping of strings and object keys.
        let bytes = serde_json::to_vec(&value).map_err(|e| invalid_result(e.to_string()))?;
        if bytes.len().saturating_add(self.stdout.text.len()) > self.limits.max_output_bytes {
            return Err(limit_error("result and diagnostics byte limit exceeded"));
        }
        self.check_deadline()?;
        Ok(RunResult {
            value,
            stdout: self.stdout.text,
        })
    }
}

fn repl_error(error: Box<monty::ReplStartError>) -> RuntimeError {
    python_error(error.error)
}
fn python_error(error: MontyException) -> RuntimeError {
    let code = match error.exc_type() {
        ExcType::MemoryError | ExcType::TimeoutError | ExcType::RecursionError => "LIMIT_EXCEEDED",
        _ => "PYTHON_ERROR",
    };
    RuntimeError::new(code, error.to_string())
}
fn limit_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::new("LIMIT_EXCEEDED", message)
}
fn invalid_result(message: impl Into<String>) -> RuntimeError {
    RuntimeError::new("INVALID_ARGUMENT", message)
}

struct JsonBudget {
    remaining: usize,
    max_depth: usize,
}
impl JsonBudget {
    fn new(limits: &Limits) -> Self {
        Self {
            remaining: limits.max_output_bytes,
            max_depth: limits.max_json_depth,
        }
    }
    fn charge(&mut self, bytes: usize) -> Result<(), RuntimeError> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or_else(|| limit_error("JSON byte limit exceeded"))?;
        Ok(())
    }
    fn node(&mut self, depth: usize) -> Result<(), RuntimeError> {
        if depth > self.max_depth {
            return Err(limit_error("JSON depth limit exceeded"));
        }
        self.charge(1)
    }
    fn encode(&mut self, value: &Value, depth: usize) -> Result<MontyObject, RuntimeError> {
        self.node(depth)?;
        Ok(match value {
            Value::Null => MontyObject::none(),
            Value::Bool(v) => MontyObject::bool(*v),
            Value::Number(v) => {
                self.charge(20)?;
                if let Some(v) = v.as_i64() {
                    MontyObject::int(v)
                } else if v.is_u64() {
                    return Err(invalid_result(
                        "integers above i64::MAX must be represented as strings",
                    ));
                } else {
                    MontyObject::float(
                        v.as_f64()
                            .ok_or_else(|| invalid_result("unsupported number"))?,
                    )
                }
            }
            Value::String(v) => {
                self.charge(v.len())?;
                MontyObject::string(v.clone())
            }
            Value::Array(v) => MontyObject::list(
                v.iter()
                    .map(|v| self.encode(v, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Value::Object(v) => MontyObject::dict(
                v.iter()
                    .map(|(k, v)| {
                        self.charge(k.len())?;
                        Ok((MontyObject::string(k.clone()), self.encode(v, depth + 1)?))
                    })
                    .collect::<Result<Vec<_>, RuntimeError>>()?,
            ),
        })
    }
    fn decode(&mut self, value: ObjectRef<'_>, depth: usize) -> Result<Value, RuntimeError> {
        self.node(depth)?;
        match value.type_name() {
            "NoneType" => Ok(Value::Null),
            "bool" => Ok(Value::Bool(
                value
                    .as_bool()
                    .ok_or_else(|| invalid_result("invalid bool"))?,
            )),
            "int" => {
                self.charge(20)?;
                Ok(Value::from(value.as_int().ok_or_else(|| {
                    invalid_result("integers outside i64 must be represented as strings")
                })?))
            }
            "float" => {
                self.charge(24)?;
                let number = serde_json::Number::from_f64(
                    value
                        .as_float()
                        .ok_or_else(|| invalid_result("invalid float"))?,
                )
                .ok_or_else(|| invalid_result("non-finite results are not JSON"))?;
                Ok(Value::Number(number))
            }
            "str" => {
                let string = value
                    .as_str()
                    .ok_or_else(|| invalid_result("invalid string"))?;
                self.charge(string.len())?;
                Ok(Value::String(string.to_owned()))
            }
            "list" | "tuple" => Ok(Value::Array(
                value
                    .items()
                    .ok_or_else(|| invalid_result("invalid sequence"))?
                    .into_iter()
                    .map(|v| self.decode(v, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            "dict" => {
                let mut result = Map::new();
                for (k, v) in value
                    .pairs()
                    .ok_or_else(|| invalid_result("invalid dict"))?
                {
                    let key = k
                        .as_str()
                        .ok_or_else(|| invalid_result("JSON dictionary keys must be strings"))?;
                    self.charge(key.len())?;
                    result.insert(key.to_owned(), self.decode(v, depth + 1)?);
                }
                Ok(Value::Object(result))
            }
            kind => Err(invalid_result(format!(
                "unsupported JSON result type: {kind}"
            ))),
        }
    }
}

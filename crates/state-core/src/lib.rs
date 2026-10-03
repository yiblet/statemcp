//! One transactional service behind all fixed MCP tools and Monty host callbacks.
mod dispatch;
mod hosts;
mod policy;
mod runtime;
mod schemas;

pub use runtime::{EmbeddedBackend, RuntimeBackend};
pub use schemas::tool_definitions;
pub use state_runtime::{Limits, WorkerConfig};
pub use state_store::Store;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fmt, path::Path, sync::Arc, time::Instant};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
}
impl Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_ARGUMENT", message)
    }
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::new("LIMIT_EXCEEDED", message)
    }
    pub(crate) fn denied() -> Self {
        Self::new(
            "PERMISSION_DENIED",
            "operation is outside the endpoint's declared grants",
        )
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<state_store::Error> for Error {
    fn from(e: state_store::Error) -> Self {
        Self::new(e.code, e.message)
    }
}
impl From<state_runtime::RuntimeError> for Error {
    fn from(e: state_runtime::RuntimeError) -> Self {
        Self::new(e.code, e.message)
    }
}
impl From<Error> for state_runtime::RuntimeError {
    fn from(e: Error) -> Self {
        Self::new(e.code, e.message)
    }
}

#[derive(Clone, Debug)]
pub struct CoreLimits {
    pub runtime: Limits,
    /// Maximum simultaneously executing scripts/endpoints, including the root.
    pub max_depth: usize,
    /// Direct service results. Runtime JSON keeps its separate, smaller limit.
    pub max_result_bytes: usize,
}
impl Default for CoreLimits {
    fn default() -> Self {
        Self {
            runtime: Limits::default(),
            max_depth: 16,
            max_result_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone)]
pub struct State {
    store: Store,
    backend: Arc<dyn RuntimeBackend>,
    limits: CoreLimits,
}
impl State {
    /// Open an embedded service; does not launch another executable.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::with_backend(
            Store::open(path)?,
            Arc::new(EmbeddedBackend),
        ))
    }
    pub fn with_backend(store: Store, backend: Arc<dyn RuntimeBackend>) -> Self {
        Self {
            store,
            backend,
            limits: CoreLimits::default(),
        }
    }
    pub fn with_limits(mut self, limits: CoreLimits) -> Self {
        self.limits = limits;
        self
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn dispatch(&self, tool: &str, args: Value) -> Result<Value> {
        self.dispatch_as("owner", tool, args)
    }
    /// `principal` is a trusted embedding identity for receipt scoping, not authentication.
    /// Every root call has local owner authority; endpoint bodies have declared grants.
    pub fn dispatch_as(&self, principal: &str, tool: &str, args: Value) -> Result<Value> {
        if principal.is_empty() || principal.len() > 256 {
            return Err(Error::invalid("principal must contain 1..256 bytes"));
        }
        schemas::validate_operation(tool, &args)?;
        let key = args.get("idempotency_key").and_then(Value::as_str);
        let hash = format!(
            "{:x}",
            Sha256::digest(json!({"tool":tool,"arguments":canonical(&args)}).to_string())
        );
        if let Some(key) = key {
            if key.is_empty() || key.len() > 256 {
                return Err(Error::invalid("idempotency_key must contain 1..256 bytes"));
            }
            if let Some(receipt) = self.store.receipt(principal, key)? {
                return if receipt.request_hash == hash {
                    Ok(receipt.result)
                } else {
                    Err(Error::new(
                        "IDEMPOTENCY_MISMATCH",
                        "key was already used for a different request",
                    ))
                };
            }
        }
        let mut root = dispatch::Root::new(
            self.store.begin()?,
            self.backend.clone(),
            self.limits.clone(),
        );
        let mut result = root.dispatch(tool, args.clone(), &policy::Access::Owner, true)?;
        root.check()?;
        bounded(&result, self.limits.max_result_bytes)?;
        if let Some(error) = root.failure {
            return Err(error);
        }
        if let Some(key) = key {
            root.tx.set_receipt(principal, key, &hash, result.clone())?;
        }
        let committed = root.tx.commit()?;
        if committed["replayed"] == true {
            return Ok(committed["result"].clone());
        }
        // Storage mutations return staged revisions. Only advertise the actual committed head.
        if tool == "state_namespace"
            && matches!(args["action"].as_str(), Some("create" | "copy" | "update"))
            && let Some(id) = result["id"].as_str().map(str::to_owned)
        {
            result["revision"] = committed["revisions"][id].clone();
        }
        Ok(result)
    }
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let ordered: std::collections::BTreeMap<_, _> =
                map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            json!(ordered)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

pub(crate) fn bounded(value: &Value, max: usize) -> Result<()> {
    // serde_json::Value serialization cannot fail for values constructed by the service.
    if value.to_string().len() > max {
        return Err(Error::limit("JSON value exceeds output byte limit"));
    }
    Ok(())
}
pub(crate) fn remaining(start: Instant, limits: &Limits) -> Result<Limits> {
    let mut runtime = limits.clone();
    runtime.max_duration = runtime
        .max_duration
        .checked_sub(start.elapsed())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| Error::limit("root invocation deadline exceeded"))?;
    Ok(runtime)
}

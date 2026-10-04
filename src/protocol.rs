use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

const SERVER_INSTRUCTIONS: &str = "Namespaces contain virtual files, named SQLite databases, and published Python functions. Namespace selectors accept names or stable UUIDs. Use tools/list for tool names and argument schemas; function.list and function.get require a namespace. To publish an endpoint, create its namespace and databases, write a Python source file with fs.write, then use function.declare and call. Published functions have only their declared grants; root execute scripts have owner access. Use expected_revision and expected_version for optimistic concurrency. Tool results are returned directly in content, with isError indicating failure; this server uses a custom JSON-RPC result shape.";

/// Application errors are successful JSON-RPC responses with MCP `isError: true`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ToolError {}

/// Both MCP calls and embedded callers can use the same application dispatcher.
pub trait Dispatcher {
    fn dispatch(&mut self, tool: &str, args: Value) -> Result<Value, ToolError>;
}
impl<F> Dispatcher for F
where
    F: FnMut(&str, Value) -> Result<Value, ToolError>,
{
    fn dispatch(&mut self, tool: &str, args: Value) -> Result<Value, ToolError> {
        self(tool, args)
    }
}

/// Scaffold dispatcher; replace with the storage/runtime service when embedding.
pub struct UnsupportedDispatcher;
impl Dispatcher for UnsupportedDispatcher {
    fn dispatch(&mut self, tool: &str, _: Value) -> Result<Value, ToolError> {
        Err(ToolError::new(
            "NOT_IMPLEMENTED",
            format!("{tool} is not connected to a service"),
        ))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    New,
    Initializing,
    Ready,
}

/// One synchronous MCP session. Input closes to shut down; stdout contains only JSON.
#[derive(Clone)]
pub struct Server<D> {
    dispatcher: D,
    phase: Phase,
}
impl<D: Dispatcher> Server<D> {
    pub fn new(dispatcher: D) -> Self {
        Self {
            dispatcher,
            phase: Phase::New,
        }
    }

    /// Handle one parsed message. Notifications never execute tools or get responses.
    pub fn handle(&mut self, request: Value) -> Option<Value> {
        let Some(object) = request.as_object() else {
            return Some(rpc_error(Value::Null, -32600, "Expected a JSON-RPC object"));
        };
        let id = object.get("id").cloned();
        if object.get("jsonrpc") != Some(&json!("2.0")) {
            return Some(rpc_error(Value::Null, -32600, "Expected jsonrpc 2.0"));
        }
        if id
            .as_ref()
            .is_some_and(|v| !v.is_string() && !v.is_i64() && !v.is_u64())
        {
            return Some(rpc_error(
                Value::Null,
                -32600,
                "Request id must be a string or integer",
            ));
        }
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // This server sends no requests. Ignore response envelopes to avoid loops.
            if id.is_some() && (object.contains_key("result") || object.contains_key("error")) {
                return None;
            }
            return Some(rpc_error(
                id.unwrap_or(Value::Null),
                -32600,
                "Missing method",
            ));
        };
        let Some(id) = id else {
            if method == "notifications/initialized" && self.phase == Phase::Initializing {
                self.phase = Phase::Ready;
            }
            return None;
        };
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Some(rpc_error(id, -32602, "params must be an object"));
        }
        let result = match method {
            "ping" => json!({}),
            "initialize" => {
                if self.phase != Phase::New {
                    return Some(rpc_error(id, -32600, "Session is already initialized"));
                }
                if !params["protocolVersion"].is_string()
                    || !params["capabilities"].is_object()
                    || !params["clientInfo"]["name"].is_string()
                    || !params["clientInfo"]["version"].is_string()
                {
                    return Some(rpc_error(
                        id,
                        -32602,
                        "initialize requires protocolVersion, capabilities, and clientInfo",
                    ));
                }
                self.phase = Phase::Initializing;
                json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{"listChanged":false}},
                    "serverInfo":{"name":"statemcp","version":env!("CARGO_PKG_VERSION")},"instructions":SERVER_INSTRUCTIONS})
            }
            "tools/list" | "tools/call" if self.phase != Phase::Ready => {
                return Some(rpc_error(
                    id,
                    -32002,
                    "Complete initialize and notifications/initialized first",
                ));
            }
            "tools/list" => {
                if params.get("cursor").is_some() {
                    return Some(rpc_error(
                        id,
                        -32602,
                        "This server does not issue pagination cursors",
                    ));
                }
                json!({"tools":tool_definitions()})
            }
            "tools/call" => {
                let Some(name) = params["name"].as_str() else {
                    return Some(rpc_error(id, -32602, "tools/call requires name"));
                };
                if !tool_definitions().iter().any(|tool| tool["name"] == name) {
                    return Some(rpc_error(id, -32602, "Unknown tool"));
                }
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if !args.is_object() {
                    return Some(rpc_error(id, -32602, "arguments must be an object"));
                }
                match self.dispatcher.dispatch(name, args) {
                    Ok(value) => tool_result(value, false),
                    Err(error) => tool_result(json!({"error":error}), true),
                }
            }
            _ => return Some(rpc_error(id, -32601, "Method not found")),
        };
        Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }

    /// Read bounded newline-delimited frames, recover from bad frames, flush each reply.
    pub fn serve(&mut self, mut reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
        loop {
            let Some(frame) = read_frame(&mut reader)? else {
                return Ok(());
            };
            let response = match frame {
                Err(()) => Some(rpc_error(
                    Value::Null,
                    -32600,
                    "Request exceeds 8 MiB frame limit",
                )),
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(request) => self.handle(request),
                    Err(_) => Some(rpc_error(Value::Null, -32700, "Parse error")),
                },
            };
            if let Some(response) = response {
                serde_json::to_writer(&mut writer, &response)?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
    }
}

pub(crate) fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
pub(crate) fn tool_result(value: Value, is_error: bool) -> Value {
    json!({"content":value,"isError":is_error})
}
fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut frame = Vec::new();
    let mut too_large = false;
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok(if too_large {
                Some(Err(()))
            } else if frame.is_empty() {
                None
            } else {
                Some(Ok(frame))
            });
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(bytes.len(), |index| index + 1);
        if !too_large {
            if frame.len() + count > MAX_FRAME_BYTES {
                too_large = true;
                frame.clear();
            } else {
                frame.extend_from_slice(&bytes[..count]);
            }
        }
        reader.consume(count);
        if newline.is_some() {
            return Ok(Some(if too_large { Err(()) } else { Ok(frame) }));
        }
    }
}

/// Fixed discovery surface, shared with application validation.
pub fn tool_definitions() -> Vec<Value> {
    state_core::tool_definitions()
}

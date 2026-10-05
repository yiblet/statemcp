//! CLI arguments and translation to the canonical service operations.
mod input;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use input::{json_value, operation, request, source};
use serde::Serialize;
use serde_json::Value;
use statemcp::{Request, Tool};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "statemcp",
    version,
    about = "Stateful agent APIs over MCP and CLI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args)]
pub struct Storage {
    /// Managed state directory (required; there is no default)
    #[arg(value_name = "DATA_DIR")]
    pub data_dir: PathBuf,
}

// Parsed once at startup; keep the subcommand arguments inline.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Command {
    /// Internal isolated Monty worker
    #[command(hide = true, long_flag = "worker")]
    Worker {
        memory_bytes: usize,
        max_frame_bytes: usize,
    },
    /// Serve MCP over standard input/output
    Stdio {
        #[command(flatten)]
        storage: Storage,
    },
    /// Serve MCP over Streamable HTTP at /mcp
    Http {
        #[command(flatten)]
        storage: Storage,
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: SocketAddr,
        /// Require Authorization: Bearer <value> on every HTTP request
        #[arg(long, value_name = "X", value_parser = bearer_token)]
        auth_bearer: Option<String>,
    },
    /// Run explicit storage cleanup and exit
    Maintenance {
        #[command(flatten)]
        storage: Storage,
        #[arg(long, default_value_t = 10000)]
        retain_receipts: usize,
    },
    /// Invoke a state tool directly without running an MCP transport
    Cli {
        #[command(subcommand)]
        tool: ToolCommand,
    },
}

#[derive(Subcommand)]
pub enum ToolCommand {
    /// Create, inspect, rename, copy, or delete namespaces
    #[command(name = "state_namespace")]
    Namespace(Namespace),
    /// Read and mutate virtual files
    #[command(name = "state_fs")]
    Fs(Files),
    /// Manage named SQLite databases, SQL, migrations, and introspection
    #[command(name = "state_db")]
    Db(Database),
    /// Publish and manage pinned endpoint definitions
    #[command(name = "state_function")]
    Function(Function),
    /// Invoke a declared endpoint
    #[command(name = "state_call")]
    Call(Call),
    /// Execute transactional Python (inline, @FILE, or - for stdin)
    #[command(name = "state_execute")]
    Execute(Execute),
    /// Discover tools and endpoint contracts
    #[command(name = "state_describe")]
    Describe(Describe),
}

#[derive(Args)]
pub struct Operation {
    #[command(flatten)]
    pub storage: Storage,
    /// Operation action; may instead be supplied inside --json
    pub action: Option<String>,
    /// Additional operation fields as JSON, @FILE, or - for stdin
    #[arg(long)]
    pub json: Option<String>,
}

#[derive(Args, Serialize)]
pub struct Namespace {
    #[command(flatten)]
    #[serde(skip)]
    operation: Operation,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}

#[derive(Args, Serialize)]
pub struct Files {
    #[command(flatten)]
    #[serde(skip)]
    operation: Operation,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    recursive: Option<bool>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}

#[derive(Args, Serialize)]
pub struct Database {
    #[command(flatten)]
    #[serde(skip)]
    operation: Operation,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sql: Option<String>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    migrations: Option<Value>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}

#[derive(Args, Serialize)]
pub struct Function {
    #[command(flatten)]
    #[serde(skip)]
    operation: Operation,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Virtual source file to publish
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<Value>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    output_schema: Option<Value>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}

#[derive(Args, Serialize)]
pub struct Call {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<String>,
    /// Complete tool fields as JSON, @FILE, or - for stdin
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<Value>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
}

#[derive(Args)]
pub struct Execute {
    #[command(flatten)]
    storage: Storage,
    /// Python source, @FILE, or - for stdin
    script: Option<String>,
    /// Complete tool fields as JSON, @FILE, or - for stdin
    #[arg(long)]
    json: Option<String>,
    #[arg(long)]
    namespace: Option<String>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    inputs: Option<Value>,
    #[arg(long)]
    idempotency_key: Option<String>,
}

#[derive(Args, Serialize)]
pub struct Describe {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Complete tool fields as JSON, @FILE, or - for stdin
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<String>,
}

fn bearer_token(value: &str) -> Result<String> {
    statemcp::http::validate_bearer(Some(value))?;
    Ok(value.to_owned())
}

impl ToolCommand {
    pub fn storage(&self) -> &Storage {
        match self {
            Self::Namespace(args) => &args.operation.storage,
            Self::Fs(args) => &args.operation.storage,
            Self::Db(args) => &args.operation.storage,
            Self::Function(args) => &args.operation.storage,
            Self::Call(args) => &args.storage,
            Self::Execute(args) => &args.storage,
            Self::Describe(args) => &args.storage,
        }
    }
    pub fn request(&self) -> Result<Request> {
        match self {
            Self::Namespace(args) => operation(&args.operation, args, Tool::Namespace),
            Self::Fs(args) => operation(&args.operation, args, Tool::File),
            Self::Db(args) => operation(&args.operation, args, Tool::Database),
            Self::Function(args) => operation(&args.operation, args, Tool::Function),
            Self::Call(args) => request(Tool::Call, args.json.as_deref(), args),
            Self::Execute(args) => {
                #[derive(Serialize)]
                struct Fields<'a> {
                    #[serde(skip_serializing_if = "Option::is_none")]
                    script: Option<String>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    namespace: Option<&'a str>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    inputs: Option<&'a Value>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    idempotency_key: Option<&'a str>,
                }
                let fields = Fields {
                    script: args.script.as_deref().map(source).transpose()?,
                    namespace: args.namespace.as_deref(),
                    inputs: args.inputs.as_ref(),
                    idempotency_key: args.idempotency_key.as_deref(),
                };
                request(Tool::Execute, args.json.as_deref(), &fields)
            }
            Self::Describe(args) => request(Tool::Describe, args.json.as_deref(), args),
        }
    }
}

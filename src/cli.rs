//! CLI arguments and translation to the canonical service operations.
mod input;
use clap::{Args, Parser, Subcommand};
use input::{json_value, merge, operation, source};
use serde::Serialize;
use serde_json::{Value, json};
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

#[derive(Subcommand)]
pub enum Command {
    /// Serve MCP over standard input/output
    Stdio {
        #[command(flatten)]
        storage: Storage,
    },
    /// Serve the JSON-RPC tool API over HTTP at /mcp
    Http {
        #[command(flatten)]
        storage: Storage,
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: SocketAddr,
        /// Require Authorization: Bearer <value> on every HTTP request
        #[arg(long, value_name = "X")]
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
        tool: Box<ToolCommand>,
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
    /// Publish and manage pinned endpoint definitions and resource grants
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
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    /// JSON array of {database, access: read|write|migrate} grants
    databases: Option<Value>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    /// JSON array of {path, access: read|write} grants
    files: Option<Value>,
    #[arg(long, value_parser = json_value, value_name = "JSON")]
    #[serde(skip_serializing_if = "Option::is_none")]
    /// JSON array of {namespace, function} grants
    calls: Option<Value>,
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

impl Command {
    pub fn storage(&self) -> &Storage {
        match self {
            Self::Stdio { storage }
            | Self::Http { storage, .. }
            | Self::Maintenance { storage, .. } => storage,
            Self::Cli { tool } => tool.storage(),
        }
    }
    /// Prepare input before opening state. Core validation remains authoritative.
    pub fn request(&self) -> Result<Option<(String, Value)>, String> {
        match self {
            Self::Cli { tool } => tool.request().map(Some),
            _ => Ok(None),
        }
    }
}

impl ToolCommand {
    fn storage(&self) -> &Storage {
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
    fn request(&self) -> Result<(String, Value), String> {
        let (tool, arguments) = match self {
            Self::Namespace(args) => ("state_namespace", operation(&args.operation, args, true)?),
            Self::Fs(args) => ("state_fs", operation(&args.operation, args, false)?),
            Self::Db(args) => ("state_db", operation(&args.operation, args, false)?),
            Self::Function(args) => ("state_function", operation(&args.operation, args, false)?),
            Self::Call(args) => ("state_call", merge(args.json.as_deref(), args)?),
            Self::Execute(args) => {
                let mut value = json!({});
                if let Some(script) = &args.script {
                    value["script"] = json!(source(script)?);
                }
                if let Some(namespace) = &args.namespace {
                    value["namespace"] = json!(namespace);
                }
                if let Some(inputs) = &args.inputs {
                    value["inputs"] = inputs.clone();
                }
                if let Some(key) = &args.idempotency_key {
                    value["idempotency_key"] = json!(key);
                }
                ("state_execute", merge(args.json.as_deref(), &value)?)
            }
            Self::Describe(args) => ("state_describe", merge(args.json.as_deref(), args)?),
        };
        Ok((tool.to_owned(), arguments))
    }
}

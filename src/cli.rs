//! CLI arguments and translation to the canonical service operations.
mod input;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use serde::Serialize;
use serde_json::{Map, Value};
use state_contract::database::SqlCell;
use state_contract::http::{ApiGetInput, ApiListInput, ApiPublishInput, ApiRemoveInput};
use state_contract::{database::*, discovery::*, execution::*, file::*, function::*, namespace::*};
use state_core::ReceiptIdentity;
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
        /// Listening address; repeat to serve on multiple interfaces
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: Vec<SocketAddr>,
        /// Path prefix for MCP and health routes, for example /statemcp
        #[arg(long, default_value = "")]
        http_prefix: statemcp::http::HttpPrefix,
        /// Require Authorization: Bearer <value> on MCP requests
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
    #[command(name = "api.publish")]
    ApiPublish(CliApi),
    #[command(name = "api.get")]
    ApiGet(CliApi),
    #[command(name = "api.list")]
    ApiList(CliApi),
    #[command(name = "api.remove")]
    ApiRemove(CliApi),
    #[command(name = "namespace.create")]
    NamespaceCreate(CliNamespaceCreate),
    #[command(name = "namespace.list")]
    NamespaceList(CliNamespaceList),
    #[command(name = "namespace.get")]
    NamespaceGet(CliNamespaceGet),
    #[command(name = "namespace.update")]
    NamespaceUpdate(CliNamespaceUpdate),
    #[command(name = "namespace.copy")]
    NamespaceCopy(CliNamespaceCopy),
    #[command(name = "namespace.delete")]
    NamespaceDelete(CliNamespaceDelete),
    #[command(name = "fs.read")]
    FsRead(CliFsRead),
    #[command(name = "fs.stat")]
    FsStat(CliFsStat),
    #[command(name = "fs.list")]
    FsList(CliFsList),
    #[command(name = "fs.write")]
    FsWrite(CliFsWrite),
    #[command(name = "fs.append")]
    FsAppend(CliFsAppend),
    #[command(name = "fs.move")]
    FsMove(CliFsMove),
    #[command(name = "fs.copy")]
    FsCopy(CliFsCopy),
    #[command(name = "fs.delete")]
    FsDelete(CliFsDelete),
    #[command(name = "db.list")]
    DbList(CliDbList),
    #[command(name = "db.create")]
    DbCreate(CliDbCreate),
    #[command(name = "db.drop")]
    DbDrop(CliDbDrop),
    #[command(name = "db.query")]
    DbQuery(CliDbQuery),
    #[command(name = "db.execute")]
    DbExecute(CliDbExecute),
    #[command(name = "db.inspect")]
    DbInspect(CliDbInspect),
    #[command(name = "db.migrations")]
    DbMigrations(CliDbMigrations),
    #[command(name = "db.migrate")]
    DbMigrate(CliDbMigrate),
    #[command(name = "function.list")]
    FunctionList(CliFunctionList),
    #[command(name = "function.get")]
    FunctionGet(CliFunctionGet),
    #[command(name = "function.declare")]
    FunctionDeclare(CliFunctionDeclare),
    #[command(name = "function.update")]
    FunctionUpdate(CliFunctionUpdate),
    #[command(name = "function.remove")]
    FunctionRemove(CliFunctionRemove),
    #[command(name = "call")]
    Call(CliCall),
    #[command(name = "execute")]
    Execute(CliExecute),
    #[command(name = "describe")]
    Describe(CliDescribe),
}
/// HTTP registration uses typed --json contracts; routes are never decorator-scanned.
#[derive(Args, Serialize)]
pub struct CliApi {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceCreate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceGet {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceUpdate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceCopy {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceDelete {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsRead {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsStat {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsWrite {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsAppend {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsMove {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsCopy {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsDelete {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    recursive: Option<bool>,
}
#[derive(Args, Serialize)]
pub struct CliDbList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbCreate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbDrop {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbQuery {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long, value_parser = input::parse_json::<input::JsonArray<SqlCell>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<input::JsonArray<SqlCell>>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sql: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbExecute {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long, value_parser = input::parse_json::<input::JsonArray<SqlCell>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<input::JsonArray<SqlCell>>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sql: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbInspect {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbMigrations {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbMigrate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long, value_parser = input::parse_json::<input::JsonArray<MigrationInput>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    migrations: Option<input::JsonArray<MigrationInput>>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionGet {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionDeclare {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<EndpointSchema>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    output_schema: Option<EndpointSchema>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionUpdate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<EndpointSchema>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    output_schema: Option<EndpointSchema>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionRemove {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliCall {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long, value_parser = input::parse_json::<Map<String, Value>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<Map<String, Value>>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliExecute {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
    #[arg(long, value_parser = input::parse_json::<Value>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs: Option<Value>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Python source, @FILE, or - for stdin.
    #[arg(value_parser = input::source)]
    #[serde(skip_serializing_if = "Option::is_none")]
    script: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDescribe {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    #[arg(long, value_parser = input::discovery_mode)]
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<DiscoveryMode>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
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
            Self::ApiPublish(args)
            | Self::ApiGet(args)
            | Self::ApiList(args)
            | Self::ApiRemove(args) => &args.storage,
            Self::NamespaceCreate(args) => &args.storage,
            Self::NamespaceList(args) => &args.storage,
            Self::NamespaceGet(args) => &args.storage,
            Self::NamespaceUpdate(args) => &args.storage,
            Self::NamespaceCopy(args) => &args.storage,
            Self::NamespaceDelete(args) => &args.storage,
            Self::FsRead(args) => &args.storage,
            Self::FsStat(args) => &args.storage,
            Self::FsList(args) => &args.storage,
            Self::FsWrite(args) => &args.storage,
            Self::FsAppend(args) => &args.storage,
            Self::FsMove(args) => &args.storage,
            Self::FsCopy(args) => &args.storage,
            Self::FsDelete(args) => &args.storage,
            Self::DbList(args) => &args.storage,
            Self::DbCreate(args) => &args.storage,
            Self::DbDrop(args) => &args.storage,
            Self::DbQuery(args) => &args.storage,
            Self::DbExecute(args) => &args.storage,
            Self::DbInspect(args) => &args.storage,
            Self::DbMigrations(args) => &args.storage,
            Self::DbMigrate(args) => &args.storage,
            Self::FunctionList(args) => &args.storage,
            Self::FunctionGet(args) => &args.storage,
            Self::FunctionDeclare(args) => &args.storage,
            Self::FunctionUpdate(args) => &args.storage,
            Self::FunctionRemove(args) => &args.storage,
            Self::Call(args) => &args.storage,
            Self::Execute(args) => &args.storage,
            Self::Describe(args) => &args.storage,
        }
    }
    pub fn prepare(&self) -> Result<PreparedCommand> {
        match self {
            Self::ApiPublish(args) => Ok(PreparedCommand::ApiPublish(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::ApiGet(args) => Ok(PreparedCommand::ApiGet(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::ApiList(args) => Ok(PreparedCommand::ApiList(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::ApiRemove(args) => Ok(PreparedCommand::ApiRemove(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::NamespaceCreate(args) => Ok(PreparedCommand::NamespaceCreate(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::NamespaceList(args) => Ok(PreparedCommand::NamespaceList(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::NamespaceGet(args) => Ok(PreparedCommand::NamespaceGet(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::NamespaceUpdate(args) => Ok(PreparedCommand::NamespaceUpdate(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::NamespaceCopy(args) => Ok(PreparedCommand::NamespaceCopy(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::NamespaceDelete(args) => Ok(PreparedCommand::NamespaceDelete(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsRead(args) => Ok(PreparedCommand::FsRead(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsStat(args) => Ok(PreparedCommand::FsStat(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsList(args) => Ok(PreparedCommand::FsList(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsWrite(args) => Ok(PreparedCommand::FsWrite(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsAppend(args) => Ok(PreparedCommand::FsAppend(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsMove(args) => Ok(PreparedCommand::FsMove(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsCopy(args) => Ok(PreparedCommand::FsCopy(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FsDelete(args) => Ok(PreparedCommand::FsDelete(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbList(args) => Ok(PreparedCommand::DbList(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbCreate(args) => Ok(PreparedCommand::DbCreate(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbDrop(args) => Ok(PreparedCommand::DbDrop(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbQuery(args) => Ok(PreparedCommand::DbQuery(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbExecute(args) => Ok(PreparedCommand::DbExecute(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbInspect(args) => Ok(PreparedCommand::DbInspect(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbMigrations(args) => Ok(PreparedCommand::DbMigrations(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::DbMigrate(args) => Ok(PreparedCommand::DbMigrate(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FunctionList(args) => Ok(PreparedCommand::FunctionList(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FunctionGet(args) => Ok(PreparedCommand::FunctionGet(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FunctionDeclare(args) => Ok(PreparedCommand::FunctionDeclare(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FunctionUpdate(args) => Ok(PreparedCommand::FunctionUpdate(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::FunctionRemove(args) => Ok(PreparedCommand::FunctionRemove(input::parse(
                args.json.as_deref(),
                args,
            )?)),
            Self::Call(args) => {
                let (input, receipt) =
                    input::parse_receipted::<CallInput>(args.json.as_deref(), args, "call")?;
                Ok(PreparedCommand::Call(input, receipt))
            }
            Self::Execute(args) => {
                let (input, receipt) =
                    input::parse_receipted::<ExecuteInput>(args.json.as_deref(), args, "execute")?;
                Ok(PreparedCommand::Execute(input, receipt))
            }
            Self::Describe(args) => Ok(PreparedCommand::Describe(input::parse(
                args.json.as_deref(),
                args,
            )?)),
        }
    }
}

pub enum PreparedCommand {
    ApiPublish(ApiPublishInput),
    ApiGet(ApiGetInput),
    ApiList(ApiListInput),
    ApiRemove(ApiRemoveInput),
    NamespaceCreate(NamespaceCreateInput),
    NamespaceList(NamespaceListInput),
    NamespaceGet(NamespaceGetInput),
    NamespaceUpdate(NamespaceUpdateInput),
    NamespaceCopy(NamespaceCopyInput),
    NamespaceDelete(NamespaceDeleteInput),
    FsRead(FsReadInput),
    FsStat(FsStatInput),
    FsList(FsListInput),
    FsWrite(FsWriteInput),
    FsAppend(FsAppendInput),
    FsMove(FsMoveInput),
    FsCopy(FsCopyInput),
    FsDelete(FsDeleteInput),
    DbList(DbListInput),
    DbCreate(DbCreateInput),
    DbDrop(DbDropInput),
    DbQuery(DbQueryInput),
    DbExecute(DbExecuteInput),
    DbInspect(DbInspectInput),
    DbMigrations(DbMigrationsInput),
    DbMigrate(DbMigrateInput),
    FunctionList(FunctionListInput),
    FunctionGet(FunctionGetInput),
    FunctionDeclare(FunctionDeclareInput),
    FunctionUpdate(FunctionUpdateInput),
    FunctionRemove(FunctionRemoveInput),
    Call(CallInput, Option<ReceiptIdentity>),
    Execute(ExecuteInput, Option<ReceiptIdentity>),
    Describe(DescribeInput),
}
impl PreparedCommand {
    pub fn run(self, state: &state_core::Controller) -> state_core::Result<String> {
        fn encode(value: impl Serialize) -> state_core::Result<String> {
            serde_json::to_string(&value).map_err(|e| {
                state_core::Error::new(state_core::ErrorCode::InvalidArgument, e.to_string())
            })
        }
        match self {
            Self::ApiPublish(input) => encode(state.publish_api(input)?),
            Self::ApiGet(input) => encode(state.get_api(input)?),
            Self::ApiList(input) => encode(state.list_api(input)?),
            Self::ApiRemove(input) => encode(state.remove_api(input)?),
            Self::NamespaceCreate(input) => encode(state.create_namespace(input)?),
            Self::NamespaceList(_) => encode(state.list_namespaces()?),
            Self::NamespaceGet(input) => encode(state.get_namespace(input)?),
            Self::NamespaceUpdate(input) => encode(state.update_namespace(input)?),
            Self::NamespaceCopy(input) => encode(state.copy_namespace(input)?),
            Self::NamespaceDelete(input) => encode(state.delete_namespace(input)?),
            Self::FsRead(input) => encode(state.read_file(input)?),
            Self::FsStat(input) => encode(state.stat_file(input)?),
            Self::FsList(input) => encode(state.list_files(input)?),
            Self::FsWrite(input) => encode(state.write_file(input)?),
            Self::FsAppend(input) => encode(state.append_file(input)?),
            Self::FsMove(input) => encode(state.move_file(input)?),
            Self::FsCopy(input) => encode(state.copy_file(input)?),
            Self::FsDelete(input) => encode(state.delete_file(input)?),
            Self::DbList(input) => encode(state.list_databases(input)?),
            Self::DbCreate(input) => encode(state.create_database(input)?),
            Self::DbDrop(input) => encode(state.drop_database(input)?),
            Self::DbQuery(input) => encode(state.query_database(input)?),
            Self::DbExecute(input) => encode(state.execute_database(input)?),
            Self::DbInspect(input) => encode(state.inspect_database(input)?),
            Self::DbMigrations(input) => encode(state.database_migrations(input)?),
            Self::DbMigrate(input) => encode(state.migrate_database(input)?),
            Self::FunctionList(input) => encode(state.list_functions(input)?),
            Self::FunctionGet(input) => encode(state.get_function(input)?),
            Self::FunctionDeclare(input) => encode(state.declare_function(input)?),
            Self::FunctionUpdate(input) => encode(state.update_function(input)?),
            Self::FunctionRemove(input) => encode(state.remove_function(input)?),
            Self::Call(input, receipt) => encode(state.call_function(input, receipt)?),
            Self::Execute(input, receipt) => encode(state.execute_script(input, receipt)?),
            Self::Describe(input) => encode(state.describe(input)?),
        }
    }
}

#[cfg(test)]
mod tests;

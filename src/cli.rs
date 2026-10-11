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
    about = "Manage namespace files, SQLite databases and Python functions; serve MCP and HTTP APIs",
    after_help = "Getting started:\n  statemcp cli namespace.create ./state --name notes\n  statemcp cli namespace.list ./state\n  statemcp http ./state --bind 127.0.0.1:8000\n\nUse statemcp cli --help to browse state operations. Use <COMMAND> --help for arguments and examples."
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
    #[command(
        after_help = "Example:\n  statemcp stdio ./state\n\nConnect an MCP client to this process's stdin/stdout. Diagnostics go to stderr."
    )]
    Stdio {
        #[command(flatten)]
        storage: Storage,
    },
    /// Serve MCP at /mcp and namespace HTTP APIs and static files
    #[command(
        after_help = "Example:\n  statemcp http ./state --bind 127.0.0.1:8000\n\nMCP is served at /mcp, published routes at /<namespace>/..., and health at /health. Configure routes with cli api.publish or the MCP api.publish tool. --http-prefix applies to every route; namespace API/static routes and /health do not require a bearer token."
    )]
    Http {
        #[command(flatten)]
        storage: Storage,
        /// Listening address; repeat to serve on multiple interfaces
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: Vec<SocketAddr>,
        /// Path prefix for all HTTP routes, for example /statemcp
        #[arg(long, default_value = "", conflicts_with = "public_url")]
        http_prefix: statemcp::http::HttpPrefix,
        /// Public HTTP(S) URL including path, or only an absolute route prefix
        #[arg(long)]
        public_url: Option<statemcp::http::PublicUrl>,
        /// Require Authorization: Bearer <value> on MCP requests; API and static routes are public
        #[arg(long, value_name = "TOKEN", value_parser = bearer_token)]
        auth_bearer: Option<String>,
    },
    /// Run explicit storage cleanup and exit
    #[command(
        after_help = "Example:\n  statemcp maintenance ./state --retain-receipts 10000\n\nPrint the cleanup report as JSON. Old receipt removal means those idempotency keys can execute again."
    )]
    Maintenance {
        #[command(flatten)]
        storage: Storage,
        /// Number of completed idempotency receipts to keep during cleanup
        #[arg(long, default_value_t = 10000)]
        retain_receipts: usize,
    },
    /// Manage state and run functions directly; print results as JSON
    #[command(
        after_help = "Start with: statemcp cli namespace.create ./state --name notes\nThen inspect: statemcp cli namespace.list ./state\nUse statemcp cli <COMMAND> --help for arguments and examples."
    )]
    Cli {
        #[command(subcommand)]
        tool: ToolCommand,
    },
}

#[derive(Subcommand)]
pub enum ToolCommand {
    #[command(
        name = "api.publish",
        about = "Publish or replace a namespace HTTP handler or static mount",
        after_help = "Examples:\n  statemcp cli api.publish ./state --json '{\"namespace\":\"notes\",\"name\":\"item\",\"path\":\"/items/{id}\",\"methods\":[\"GET\"],\"target\":{\"kind\":\"function\",\"file\":\"/web.py\",\"symbol\":\"handle\"}}'\n  statemcp cli api.publish ./state --json '{\"namespace\":\"notes\",\"name\":\"site\",\"path\":\"/{*path}\",\"target\":{\"kind\":\"static\",\"directory\":\"/public\",\"index\":\"index.html\"}}'\n\nPaths are relative to /<namespace>. Omit methods to accept every method for a function, or GET/HEAD for static files. Function sources are pinned at publication; static files remain live. Optional expected_revision guards against concurrent changes."
    )]
    ApiPublish(CliApi),
    #[command(
        name = "api.get",
        about = "Show a named HTTP route in a namespace",
        after_help = "Example:\n  statemcp cli api.get ./state --json '{\"namespace\":\"notes\",\"name\":\"item\"}'"
    )]
    ApiGet(CliApi),
    #[command(
        name = "api.list",
        about = "List HTTP handlers and static mounts in a namespace",
        after_help = "Example:\n  statemcp cli api.list ./state --json '{\"namespace\":\"notes\"}'"
    )]
    ApiList(CliApi),
    #[command(
        name = "api.remove",
        about = "Remove a named HTTP route from a namespace",
        after_help = "Example:\n  statemcp cli api.remove ./state --json '{\"namespace\":\"notes\",\"name\":\"item\"}'\n\nOptional expected_revision guards against concurrent changes. Removing a route leaves its source and static files in place."
    )]
    ApiRemove(CliApi),
    #[command(
        name = "namespace.create",
        about = "Create an empty namespace for files, databases and functions",
        after_help = "Example:\n  statemcp cli namespace.create ./state --name notes --description \"Personal notes\"\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): name."
    )]
    NamespaceCreate(CliNamespaceCreate),
    #[command(
        name = "namespace.list",
        about = "List namespace names, IDs, revisions and resource counts",
        after_help = "Example:\n  statemcp cli namespace.list ./state\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON."
    )]
    NamespaceList(CliNamespaceList),
    #[command(
        name = "namespace.get",
        about = "Show a namespace and its current revision",
        after_help = "Example:\n  statemcp cli namespace.get ./state --namespace notes\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace."
    )]
    NamespaceGet(CliNamespaceGet),
    #[command(
        name = "namespace.update",
        about = "Rename a namespace or change its description",
        after_help = "Example:\n  statemcp cli namespace.update ./state --namespace notes --name archive\n\nOmitted description is preserved. To clear it, use --json '{\"namespace\":\"notes\",\"description\":null}'.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace."
    )]
    NamespaceUpdate(CliNamespaceUpdate),
    #[command(
        name = "namespace.copy",
        about = "Copy a namespace and its contents to an independent namespace",
        after_help = "Example:\n  statemcp cli namespace.copy ./state --namespace notes --name sandbox\n\nThe copy includes files, databases, published functions and HTTP routes. Later changes are independent.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, name."
    )]
    NamespaceCopy(CliNamespaceCopy),
    #[command(
        name = "namespace.delete",
        about = "Delete a namespace and its contents; its name remains reserved",
        after_help = "Example:\n  statemcp cli namespace.delete ./state --namespace sandbox\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace."
    )]
    NamespaceDelete(CliNamespaceDelete),
    #[command(
        name = "fs.read",
        about = "Read a namespace file as text or base64 for binary data",
        after_help = "Example:\n  statemcp cli fs.read ./state --namespace notes --path /hello.txt\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, path."
    )]
    FsRead(CliFsRead),
    #[command(
        name = "fs.stat",
        about = "Show file metadata or check whether a directory exists",
        after_help = "Example:\n  statemcp cli fs.stat ./state --namespace notes --path /hello.txt\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, path."
    )]
    FsStat(CliFsStat),
    #[command(
        name = "fs.list",
        about = "List files and directories directly beneath a virtual path",
        after_help = "Example:\n  statemcp cli fs.list ./state --namespace notes --path /\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace."
    )]
    FsList(CliFsList),
    #[command(
        name = "fs.write",
        about = "Create or replace a namespace file with text or binary bytes",
        after_help = "Example:\n  statemcp cli fs.write ./state --namespace notes --path /hello.txt --text \"Hello\"\n\nSupply exactly one of --text or --base64. Paths refer to the namespace filesystem, not host files.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, path, and one of text/base64."
    )]
    FsWrite(CliFsWrite),
    #[command(
        name = "fs.append",
        about = "Append text or binary bytes to a namespace file",
        after_help = "Example:\n  statemcp cli fs.append ./state --namespace notes --path /hello.txt --text \" world\"\n\nSupply exactly one of --text or --base64. Paths refer to the namespace filesystem, not host files.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, path, and one of text/base64."
    )]
    FsAppend(CliFsAppend),
    #[command(
        name = "fs.move",
        about = "Move a file to another path within its namespace",
        after_help = "Example:\n  statemcp cli fs.move ./state --namespace notes --path /hello.txt --destination /greeting.txt\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, path, destination."
    )]
    FsMove(CliFsMove),
    #[command(
        name = "fs.copy",
        about = "Copy a file to another path within its namespace",
        after_help = "Example:\n  statemcp cli fs.copy ./state --namespace notes --path /hello.txt --destination /copy.txt\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, path, destination."
    )]
    FsCopy(CliFsCopy),
    #[command(
        name = "fs.delete",
        about = "Delete a file or recursively delete a directory's contents",
        after_help = "Example:\n  statemcp cli fs.delete ./state --namespace notes --path /old --recursive\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, path."
    )]
    FsDelete(CliFsDelete),
    #[command(
        name = "db.list",
        about = "List the SQLite databases in a namespace",
        after_help = "Example:\n  statemcp cli db.list ./state --namespace notes\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace."
    )]
    DbList(CliDbList),
    #[command(
        name = "db.create",
        about = "Create an empty named SQLite database",
        after_help = "Example:\n  statemcp cli db.create ./state --namespace notes --database main\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, database."
    )]
    DbCreate(CliDbCreate),
    #[command(
        name = "db.drop",
        about = "Delete a named SQLite database and its data",
        after_help = "Example:\n  statemcp cli db.drop ./state --namespace notes --database scratch\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, database."
    )]
    DbDrop(CliDbDrop),
    #[command(
        name = "db.query",
        about = "Run one read-only SQL statement and return its rows",
        after_help = "Example:\n  statemcp cli db.query ./state --namespace notes --database main --sql \"SELECT * FROM items WHERE id = ?\" --params '[1]'\n\nUse ? placeholders with --params to bind values. Writes are rejected; use db.execute for mutations.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, database, sql."
    )]
    DbQuery(CliDbQuery),
    #[command(
        name = "db.execute",
        about = "Run one SQL statement, including writes and RETURNING",
        after_help = "Example:\n  statemcp cli db.execute ./state --namespace notes --database main --sql \"INSERT INTO items (title) VALUES (?)\" --params '[\"hello\"]'\n\nUse ? placeholders with --params to bind values. Use db.migrate for an ordered batch of statements.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, database, sql."
    )]
    DbExecute(CliDbExecute),
    #[command(
        name = "db.inspect",
        about = "Inspect database schema, indexes, foreign keys and storage metadata",
        after_help = "Example:\n  statemcp cli db.inspect ./state --namespace notes --database main\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, database."
    )]
    DbInspect(CliDbInspect),
    #[command(
        name = "db.migrations",
        about = "List applied migration IDs and checksums in order",
        after_help = "Example:\n  statemcp cli db.migrations ./state --namespace notes --database main\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, database."
    )]
    DbMigrations(CliDbMigrations),
    #[command(
        name = "db.migrate",
        about = "Apply an ordered batch of SQL migrations atomically",
        after_help = "Example:\n  statemcp cli db.migrate ./state --namespace notes --database main --migrations '[{\"id\":\"001\",\"sql\":\"CREATE TABLE items (id INTEGER PRIMARY KEY, title TEXT)\"}]'\n\nEach migration has an id and sql. Already applied IDs must match their stored checksum and order; failures roll back the entire batch.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, database, migrations."
    )]
    DbMigrate(CliDbMigrate),
    #[command(
        name = "function.list",
        about = "List published functions and their versions and schemas",
        after_help = "Example:\n  statemcp cli function.list ./state --namespace notes\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace."
    )]
    FunctionList(CliFunctionList),
    #[command(
        name = "function.get",
        about = "Show a published function, including its pinned source",
        after_help = "Example:\n  statemcp cli function.get ./state --namespace notes --name greet\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, name."
    )]
    FunctionGet(CliFunctionGet),
    #[command(
        name = "function.declare",
        about = "Publish a Python function from a namespace file",
        after_help = "Example:\n  statemcp cli function.declare ./state --namespace notes --name greet --file /functions.py --symbol greet\n\nWrite the Python file with fs.write first. Source and namespace imports are pinned at publication. Calls pass keyword arguments and require a JSON-compatible result.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, name, file, symbol."
    )]
    FunctionDeclare(CliFunctionDeclare),
    #[command(
        name = "function.update",
        about = "Replace a published function using the current source file",
        after_help = "Example:\n  statemcp cli function.update ./state --namespace notes --name greet --file /functions.py --symbol greet\n\nEditing a source file does not change a published function until it is published again. Supply file and symbol; source and imports are pinned again.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, name, file, symbol."
    )]
    FunctionUpdate(CliFunctionUpdate),
    #[command(
        name = "function.remove",
        about = "Remove a published function while keeping its source and data",
        after_help = "Example:\n  statemcp cli function.remove ./state --namespace notes --name greet\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, name."
    )]
    FunctionRemove(CliFunctionRemove),
    #[command(
        name = "call",
        about = "Call a published function with JSON keyword arguments",
        after_help = "Example:\n  statemcp cli call ./state notes greet --arguments '{\"name\":\"Ada\"}'\n\nNAMESPACE and FUNCTION select the published function. --arguments is a JSON object whose keys become Python keyword arguments.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): namespace, function."
    )]
    Call(CliCall),
    #[command(
        name = "execute",
        about = "Run a Python script with atomic namespace state changes",
        after_help = "Example:\n  statemcp cli execute ./state --namespace notes --inputs '{\"name\":\"Ada\"}' 'inputs[\"name\"]'\n\nSCRIPT accepts inline Python, @FILE or - for stdin. --inputs becomes the Python inputs value. With --namespace, scripts can access that namespace's files, databases and functions.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON.\nRequired fields (flags/positionals or --json): script."
    )]
    Execute(CliExecute),
    #[command(
        name = "describe",
        about = "Discover tools, schemas, namespace functions and runtime documentation",
        after_help = "Example:\n  statemcp cli describe ./state --tool api.publish\n\nChoose one: --mode overview|full|runtime|readme (default overview), --tool NAME, or --namespace NAME [--function NAME]. For example: statemcp cli describe ./state --mode runtime.\n\nArguments may be supplied as flags or in --json; do not supply the same field twice. JSON inputs accept inline JSON, @FILE or - for stdin. Results are printed as JSON."
    )]
    Describe(CliDescribe),
}
/// Shared storage and JSON arguments for the HTTP route commands.
#[derive(Args, Serialize)]
pub struct CliApi {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceCreate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Human-readable description
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Name for the new namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceGet {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceUpdate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Human-readable description
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// New namespace name; omit to keep the current name
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceCopy {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Human-readable description
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Name for the new independent namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliNamespaceDelete {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsRead {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Absolute virtual path within the namespace, such as /notes.txt
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsStat {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Absolute virtual path within the namespace, such as /notes.txt
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Virtual directory to list; defaults to /
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsWrite {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Absolute virtual path within the namespace, such as /notes.txt
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// UTF-8 text to write; supply this or --base64
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// Base64-encoded file bytes; supply this or --text
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsAppend {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Absolute virtual path within the namespace, such as /notes.txt
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// UTF-8 text to append; supply this or --base64
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// Base64-encoded file bytes; supply this or --text
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsMove {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Destination virtual file path in the same namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Absolute virtual path within the namespace, such as /notes.txt
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsCopy {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Destination virtual file path in the same namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Absolute virtual path within the namespace, such as /notes.txt
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFsDelete {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Absolute virtual path within the namespace, such as /notes.txt
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// Delete all files beneath a virtual directory; defaults to false
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    recursive: Option<bool>,
}
#[derive(Args, Serialize)]
pub struct CliDbList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbCreate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Database name within the namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbDrop {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Database name within the namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbQuery {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Database name within the namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// SQL parameters as a JSON array, such as [1,"hello"]; accepts @FILE or -
    #[arg(long, value_parser = input::parse_json::<input::JsonArray<SqlCell>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<input::JsonArray<SqlCell>>,
    /// One SQL statement; bind values with ? placeholders and --params
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sql: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbExecute {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Database name within the namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// SQL parameters as a JSON array, such as [1,"hello"]; accepts @FILE or -
    #[arg(long, value_parser = input::parse_json::<input::JsonArray<SqlCell>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<input::JsonArray<SqlCell>>,
    /// One SQL statement; bind values with ? placeholders and --params
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sql: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbInspect {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Database name within the namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbMigrations {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Database name within the namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliDbMigrate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Database name within the namespace
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Ordered JSON array of migration objects with id and sql; accepts @FILE or -
    #[arg(long, value_parser = input::parse_json::<input::JsonArray<MigrationInput>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    migrations: Option<input::JsonArray<MigrationInput>>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionList {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionGet {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Reject the operation unless the published function version matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    /// Published function name
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionDeclare {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Human-readable description
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Reject the operation unless the published function version matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    /// Python source file in the namespace, such as /functions.py
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    /// JSON Schema for keyword arguments: object or boolean, @FILE or -
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<EndpointSchema>,
    /// Published function name
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// JSON Schema for the return value: object or boolean, @FILE or -
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    output_schema: Option<EndpointSchema>,
    /// Python function name defined in the source file
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionUpdate {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Human-readable description
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Reject the operation unless the published function version matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    /// Python source file in the namespace, such as /functions.py
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    /// JSON Schema for keyword arguments: object or boolean, @FILE or -
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<EndpointSchema>,
    /// Published function name
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// JSON Schema for the return value: object or boolean, @FILE or -
    #[arg(long, value_parser = input::parse_json::<EndpointSchema>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    output_schema: Option<EndpointSchema>,
    /// Python function name defined in the source file
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliFunctionRemove {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Reject the operation unless the namespace revision matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    /// Reject the operation unless the published function version matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    /// Published function name
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliCall {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Keyword arguments as a JSON object, @FILE or -; defaults to {}
    #[arg(long, value_parser = input::parse_json::<Map<String, Value>>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<Map<String, Value>>,
    /// Reject the operation unless the published function version matches this value
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    /// Reuse a key with identical arguments to replay the result without repeating writes
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
    /// Namespace name or stable UUID
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Published function name
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<String>,
}
#[derive(Args, Serialize)]
pub struct CliExecute {
    #[command(flatten)]
    #[serde(skip)]
    storage: Storage,
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Reuse a key with identical arguments to replay the result without repeating writes
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
    /// JSON value available as inputs in the script, @FILE or -
    #[arg(long, value_parser = input::parse_json::<Value>)]
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs: Option<Value>,
    /// Namespace name or UUID for state access; omit for an unscoped script
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
    /// Command arguments as a JSON object, @FILE or - for stdin; duplicate fields are rejected
    #[arg(long)]
    #[serde(skip)]
    json: Option<String>,
    /// Documentation to show: overview (default), full, runtime or readme
    #[arg(long, value_parser = input::discovery_mode)]
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<DiscoveryMode>,
    /// Tool name whose description and JSON input schema to show, such as api.publish
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    /// Namespace name or stable UUID
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    /// Published function name
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

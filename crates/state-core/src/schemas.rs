use crate::{
    Error, Operation, Result,
    selectors::{SchemaDialect, SchemaKeyword},
};
use serde_json::{Map, Value};
use std::sync::OnceLock;

fn tool_description(operation: Operation) -> &'static str {
    match operation {
        Operation::Api(state_contract::operations::ApiAction::Publish) => {
            "Publish or replace an HTTP function route or static website mount beneath /namespace/. Handler receives one Request and returns Flask buffered response forms. Omitted methods means all methods for functions. Namespace-wide access and atomic writes; source/imports pinned at publication. Returns hosted URL patterns. For HTTP API or website work, first read MCP resource statemcp://guides/api-and-sites (small payloads, CDN imports, HTMX and Tailwind patterns)."
        }
        Operation::Api(state_contract::operations::ApiAction::Get) => {
            "Get one named HTTP route and its hosted URL patterns, without pinned source."
        }
        Operation::Api(state_contract::operations::ApiAction::List) => {
            "List HTTP function routes and static mounts in a namespace with hosted URL patterns."
        }
        Operation::Api(state_contract::operations::ApiAction::Remove) => {
            "Remove a named HTTP route, optionally guarding the namespace revision."
        }
        Operation::Namespace(crate::NamespaceAction::Create) => {
            "Create an empty namespace with an optional description for files, SQLite databases, and published functions."
        }
        Operation::Namespace(crate::NamespaceAction::List) => {
            "Primary entry point for discovering what is available in StateMCP. List namespaces with their names, descriptions, stable IDs, revisions, and resource counts. Then use function.list to discover published functions in the relevant namespace."
        }
        Operation::Namespace(crate::NamespaceAction::Get) => {
            "Get a namespace’s name, description, stable ID, revision, and resource counts."
        }
        Operation::Namespace(crate::NamespaceAction::Update) => {
            "Update a namespace name or description while preserving its stable ID and contents. Omitted description is preserved; null clears it."
        }
        Operation::Namespace(crate::NamespaceAction::Copy) => {
            "Copy a namespace and its contents to a new name. Subsequent changes are independent."
        }
        Operation::Namespace(crate::NamespaceAction::History) => {
            "List retained revisions newest first, including named pins."
        }
        Operation::Namespace(crate::NamespaceAction::Pin) => {
            "Pin a retained namespace revision with an optional unique name. At most seven pins; unpin one before adding an eighth."
        }
        Operation::Namespace(crate::NamespaceAction::Unpin) => {
            "Unpin a retained revision. Content outside recent history is collected at commit."
        }
        Operation::Namespace(crate::NamespaceAction::Delete) => {
            "Delete a namespace and its contents from active use. Its name remains reserved."
        }
        Operation::File(crate::FileAction::Read) => {
            "Read a virtual file as UTF-8 text or base64 for binary data."
        }
        Operation::File(crate::FileAction::Stat) => {
            "Get a virtual file’s metadata or check whether a virtual directory exists."
        }
        Operation::File(crate::FileAction::List) => {
            "List files and directories directly beneath a virtual path."
        }
        Operation::File(crate::FileAction::Write) => {
            "Create or replace a virtual file. Supply either text or base64."
        }
        Operation::File(crate::FileAction::Append) => {
            "Append text or base64-encoded bytes to a virtual file."
        }
        Operation::File(crate::FileAction::Move) => {
            "Move a virtual file to another path in the same namespace."
        }
        Operation::File(crate::FileAction::Copy) => {
            "Copy a virtual file to another path in the same namespace."
        }
        Operation::File(crate::FileAction::Delete) => {
            "Delete a virtual file, or delete a directory’s contents with recursive=true."
        }
        Operation::Database(crate::DatabaseAction::List) => {
            "List the named SQLite databases in a namespace."
        }
        Operation::Database(crate::DatabaseAction::Create) => {
            "Create an empty named SQLite database in a namespace."
        }
        Operation::Database(crate::DatabaseAction::Drop) => {
            "Drop a named SQLite database. Functions resolve database names within their namespace on each call."
        }
        Operation::Database(crate::DatabaseAction::Query) => {
            "Run one read-only SQL statement with optional bound parameters. Returns columns, rows, and rows_affected."
        }
        Operation::Database(crate::DatabaseAction::Execute) => {
            "Run one SQL statement with optional bound parameters, including writes and RETURNING. Returns columns, rows, and rows_affected."
        }
        Operation::Database(crate::DatabaseAction::Inspect) => {
            "Inspect a database’s tables, columns, indexes, foreign keys, schema, migration history, and storage metadata."
        }
        Operation::Database(crate::DatabaseAction::Migrations) => {
            "Get the ordered IDs and checksums of applied database migrations."
        }
        Operation::Database(crate::DatabaseAction::Migrate) => {
            "Apply an ordered batch of SQL migrations atomically. Previously applied IDs must match their stored checksums and order."
        }
        Operation::Function(crate::FunctionAction::List) => {
            "List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts, without source code. Use function.get with namespace and name for a full declaration."
        }
        Operation::Function(crate::FunctionAction::Get) => {
            "Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again."
        }
        Operation::Function(crate::FunctionAction::Remove) => {
            "Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected."
        }
        Operation::Function(crate::FunctionAction::Declare) => {
            "Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions can access all files, databases, and functions in their own namespace only. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls. Before writing Python, read MCP resource statemcp://guides/python (execution, helpers, scope, imports, transactions, and return values)."
        }
        Operation::Function(crate::FunctionAction::Update) => {
            "Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete declaration. Functions retain full access to their own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version. Before writing Python, read MCP resource statemcp://guides/python (execution, helpers, scope, imports, transactions, and return values)."
        }
        Operation::Call => {
            "Invoke a published function using namespace, function, and an arguments object matching its input schema. Returns the function's JSON value directly. The function runs with full access to its own namespace only; nested calls and writes share one transaction and failures roll back changes. expected_version guards against calling changed code. Reuse an idempotency_key only with the identical request to replay a completed result without repeating writes."
        }
        Operation::Execute => {
            "Run a Python script with owner access in one transaction. The inputs argument is available as the Python variable inputs; the final expression and printed output are returned as {value, stdout}. Set namespace to use db_query(database, sql, params=[]), db_execute(database, sql, params=[]), db_inspect(database), read_text(path), and write_text(path, text). SQL helpers return {columns, rows, rows_affected}. Use mcp(name, arguments={}) for any tool or call(namespace, function, arguments={}) for a published function. Failures roll back all nested writes. Imports load Python files and packages from the selected namespace, with per-run module caching. Published functions use pinned module sources. Reuse an idempotency_key only for an identical request. Before writing Python, read MCP resource statemcp://guides/python (execution, helpers, scope, imports, transactions, and return values)."
        }
        Operation::Describe => {
            "Get a compact API overview, mode=runtime for a Python authoring guide, or mode=full for all schemas, or mode=readme for the bundled documentation. Select tool for one tool’s schema, or namespace and optional function for endpoint contracts."
        }
    }
}

/// MCP tools have one operation per name and no action discriminator.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<schemars::Schema>,
}
pub(crate) fn typed_tool_definitions(full: bool) -> Vec<ToolDefinition> {
    use crate::{DatabaseAction, FileAction, FunctionAction, NamespaceAction};
    NamespaceAction::ALL
        .iter()
        .copied()
        .map(Operation::Namespace)
        .chain(FileAction::ALL.iter().copied().map(Operation::File))
        .chain(DatabaseAction::ALL.iter().copied().map(Operation::Database))
        .chain(FunctionAction::ALL.iter().copied().map(Operation::Function))
        .chain(
            state_contract::operations::ApiAction::ALL
                .iter()
                .copied()
                .map(Operation::Api),
        )
        .chain([Operation::Call, Operation::Execute, Operation::Describe])
        .map(|operation| ToolDefinition {
            name: operation.public_name(),
            description: tool_description(operation),
            input_schema: full.then(|| state_contract::schema::schema(operation)),
        })
        .collect()
}
pub fn tool_definitions() -> Vec<ToolDefinition> {
    typed_tool_definitions(true)
}

pub(crate) fn compile(
    schema: &state_contract::function::EndpointSchema,
) -> Result<jsonschema::Validator> {
    // The schema library consumes JSON; preserve the typed object/boolean contract until this boundary.
    let schema = serde_json::to_value(schema).map_err(|e| Error::invalid(e.to_string()))?;
    if schema.to_string().len() > 64 * 1024 {
        return Err(Error::limit("endpoint schema exceeds 64 KiB"));
    }
    check_references(&schema, 0)?;
    jsonschema::meta::validate(&schema)
        .map_err(|e| Error::invalid(format!("invalid JSON schema: {e}")))?;
    jsonschema::options()
        .with_retriever(DenyRetrieval)
        .build(&schema)
        .map_err(|e| Error::invalid(format!("invalid JSON schema: {e}")))
}

// Explicit denial also protects embedders that enable jsonschema's network/file
// features elsewhere in their dependency graph (Cargo features are additive).
struct DenyRetrieval;
impl jsonschema::Retrieve for DenyRetrieval {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema retrieval is forbidden".into())
    }
}
fn check_references(value: &Value, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(Error::limit("endpoint schema nesting exceeds 32"));
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                match SchemaKeyword::parse(key) {
                    Some(SchemaKeyword::Reference) => {
                        if !value
                            .as_str()
                            .is_some_and(|reference| reference.starts_with('#'))
                        {
                            return Err(Error::invalid(
                                "schema references must be local fragments; external retrieval is disabled",
                            ));
                        }
                        check_references(value, depth + 1)?;
                    }
                    Some(SchemaKeyword::Dialect) => {
                        if value.as_str().and_then(SchemaDialect::parse).is_none() {
                            return Err(Error::invalid(
                                "only built-in JSON Schema dialects are supported",
                            ));
                        }
                        check_references(value, depth + 1)?;
                    }
                    Some(SchemaKeyword::Id) => {
                        return Err(Error::invalid(
                            "schema $id resources are unsupported; use local $defs",
                        ));
                    }
                    Some(SchemaKeyword::InstanceData) => {}
                    Some(SchemaKeyword::SchemaMap) => {
                        if let Some(map) = value.as_object() {
                            for schema in map.values() {
                                check_references(schema, depth + 1)?;
                            }
                        }
                    }
                    None => check_references(value, depth + 1)?,
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                check_references(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Discoverable endpoint authoring contract; kept beside the fixed tool schemas.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RuntimeGuide {
    pub runtime: String,
    pub abi_version: u32,
    pub compatibility: String,
    pub execution: String,
    pub hosts: Vec<HostGuide>,
    pub scope: String,
    pub transactions: String,
    pub snapshot: String,
    pub example: Vec<ExampleCall>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HostGuide {
    pub signature: String,
    pub returns: String,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "tool", content = "arguments")]
pub enum ExampleCall {
    #[serde(rename = "namespace.create")]
    NamespaceCreate(state_contract::namespace::NamespaceCreateInput),
    #[serde(rename = "db.create")]
    DbCreate(state_contract::database::DbCreateInput),
    #[serde(rename = "db.execute")]
    DbExecute(state_contract::database::DbExecuteInput),
    #[serde(rename = "fs.write")]
    FsWrite(state_contract::file::FsWriteInput),
    #[serde(rename = "function.declare")]
    FunctionDeclare(state_contract::function::FunctionDeclareInput),
    #[serde(rename = "call")]
    Call(state_contract::execution::CallInput),
}
fn examples() -> Vec<ExampleCall> {
    use state_contract::{database::*, execution::*, file::*, function::*, namespace::*};
    let input_schema = EndpointSchema::Object(Map::from_iter([
        ("type".into(), Value::String("object".into())),
        (
            "properties".into(),
            Value::Object(Map::from_iter([(
                "text".into(),
                Value::Object(Map::from_iter([(
                    "type".into(),
                    Value::String("string".into()),
                )])),
            )])),
        ),
        (
            "required".into(),
            Value::Array(vec![Value::String("text".into())]),
        ),
        ("additionalProperties".into(), Value::Bool(false)),
    ]));
    vec![
        ExampleCall::NamespaceCreate(NamespaceCreateInput { name:"demo".into(), description:state_contract::namespace::DescriptionChange::Unchanged }),
        ExampleCall::DbCreate(DbCreateInput { namespace:"demo".into(),database:"app".into(),expected_revision:None }),
        ExampleCall::DbExecute(DbExecuteInput { namespace:"demo".into(),database:"app".into(),sql:"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT)".into(),params:None,expected_revision:None }),
        ExampleCall::FsWrite(FsWriteInput::Text(FileTextInput { namespace:"demo".into(),path:"/api.py".into(),text:"def add(text):\n    return db_execute('app', 'INSERT INTO notes(text) VALUES (?) RETURNING id, text', [text])['rows'][0]\n".into(),expected_revision:None })),
        ExampleCall::FunctionDeclare(FunctionDeclareInput { namespace:"demo".into(),name:"add".into(),file:"/api.py".into(),symbol:"add".into(),input_schema:Some(input_schema),output_schema:None,description:None,expected_version:None,expected_revision:None }),
        ExampleCall::Call(CallInput { namespace:"demo".into(),function:"add".into(),arguments:Some(Map::from_iter([("text".into(),Value::String("hello".into()))])),expected_version:None,idempotency_key:None }),
    ]
}
/// Build the static runtime documentation from typed records once.
pub(crate) fn runtime_guide() -> RuntimeGuide {
    static GUIDE: OnceLock<RuntimeGuide> = OnceLock::new();
    GUIDE.get_or_init(|| RuntimeGuide {
runtime: "Pydantic Monty".into(),
abi_version: 1,
compatibility: "Only ABI 1 is supported. Incompatible host signatures, invocation conventions, or result representations require an ABI increment. Unsupported declarations fail with ABI_MISMATCH and expected/actual versions; adapt source and redeclare. No automatic migration or older-ABI support is provided.".into(),
execution: "Endpoint arguments are keyword arguments to the pinned Python symbol. Return a JSON-compatible value. Scripts use inputs and return their final expression as {value, stdout}. Helpers use the endpoint namespace or execute.namespace. Imports load namespace Python files and packages with per-run caching; published functions use pinned sources. Module initialization cannot call host APIs.".into(),
hosts: vec![HostGuide { signature: "config()".into(), returns: "Config object with .public_url (origin or None) and .prefix; contains no credentials.".into() },HostGuide { signature: "mcp(name, arguments={})".into(), returns: "The selected state tool result; function namespace isolation still applies.".into() },HostGuide { signature: "call(namespace, function, arguments={})".into(), returns: "Endpoint JSON value.".into() },HostGuide { signature: "db_query(database, sql, params=[])".into(), returns: "{columns: [string], rows: [[value]], rows_affected: integer}; one read-only statement.".into() },HostGuide { signature: "db_execute(database, sql, params=[])".into(), returns: "{columns: [string], rows: [[value]], rows_affected: integer}; one statement, supports RETURNING.".into() },HostGuide { signature: "db_inspect(database)".into(), returns: "Database name, id, snapshot, staged flag, schema_fingerprint, migrations, schema query result, and tables with columns/indexes/foreign_keys.".into() },HostGuide { signature: "read_text(path)".into(), returns: "UTF-8 string.".into() },HostGuide { signature: "write_text(path, text)".into(), returns: "File write metadata.".into() }],
scope: "Published functions can access all files, databases, and functions in their own namespace, including creating databases, publishing functions, and running nested scripts. self resolves to the calling function namespace. Namespace names and UUIDs must resolve to that same namespace. Functions cannot access another namespace or create, copy, or list namespaces. Root scripts have owner access.".into(),
transactions: "Nested host calls share the root transaction. Failure rolls back staged effects. Use expected_revision for namespace concurrency and expected_version for endpoint concurrency.".into(),
snapshot: "An opaque backing SQLite database file identifier, not a content hash or namespace revision. staged=true reports pending writes in the current invocation. Copies use independent files. No historical read or restore API accepts this identifier.".into(),
example: examples()
    }).clone()
}

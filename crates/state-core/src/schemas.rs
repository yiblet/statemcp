use crate::{
    Error, Operation, Result, Tool,
    selectors::{Field, SchemaDialect, SchemaKeyword},
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

fn field(name: &str) -> Value {
    match Field::parse(name) {
        Some(Field::Inputs) => json!({}),
        Some(Field::Mode) => json!({"enum":["overview","full","runtime","readme"]}),
        Some(Field::Arguments) => json!({"type":"object"}),
        Some(Field::Schema) => json!({"type":["object","boolean"]}),
        Some(Field::Recursive) => json!({"type":"boolean"}),
        Some(Field::Params) => json!({"type":"array"}),
        Some(Field::Databases) => {
            json!({"type":"array","items":{"type":"object","properties":{"database":{"type":"string","minLength":1},"access":{"enum":["read","write","migrate"]}},"required":["database","access"],"additionalProperties":false}})
        }
        Some(Field::Files) => {
            json!({"type":"array","items":{"type":"object","properties":{"path":{"type":"string","pattern":"^/"},"access":{"enum":["read","write"]}},"required":["path","access"],"additionalProperties":false}})
        }
        Some(Field::Calls) => {
            json!({"type":"array","items":{"type":"object","properties":{"namespace":{"type":"string","minLength":1},"function":{"type":"string","minLength":1}},"required":["namespace","function"],"additionalProperties":false}})
        }
        Some(Field::Migrations) => {
            json!({"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"sql":{"type":"string"}},"required":["id","sql"],"additionalProperties":false}})
        }
        None => json!({"type":"string"}),
    }
}
fn variant(action: Option<&str>, required: &[&str], optional: &[&str]) -> Value {
    let mut properties = Map::new();
    let mut required: Vec<_> = required.iter().map(|s| json!(s)).collect();
    if let Some(action) = action {
        properties.insert("action".into(), json!({"const":action}));
        required.push(json!("action"));
    }
    for name in required
        .iter()
        .filter_map(Value::as_str)
        .chain(optional.iter().copied())
    {
        if name != "action" || action.is_none() {
            properties.insert(name.into(), field(name));
        }
    }
    let mut schema = json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
    if optional.contains(&"text") && optional.contains(&"base64") {
        schema["oneOf"] = json!([{"required":["text"]},{"required":["base64"]}]);
    }
    if action.is_none() && optional == ["namespace", "function"] {
        schema["dependentRequired"] = json!({"function":["namespace"]});
    }
    schema
}
/// Canonical fixed-tool schemas, also enforced for calls originating inside Monty.
fn operation_definitions() -> Vec<Value> {
    let tools = [
        (
            "state_namespace",
            "Create, inspect, list, rename, copy, or delete namespaces.",
            vec![
                variant(Some("create"), &["name"], &[]),
                variant(Some("list"), &[], &[]),
                variant(Some("get"), &["namespace"], &[]),
                variant(
                    Some("update"),
                    &["namespace", "name"],
                    &["expected_revision"],
                ),
                variant(Some("copy"), &["namespace", "name"], &["expected_revision"]),
                variant(Some("delete"), &["namespace"], &["expected_revision"]),
            ],
        ),
        (
            "state_fs",
            "Read and mutate namespace virtual files.",
            vec![
                variant(Some("read"), &["namespace", "path"], &[]),
                variant(Some("stat"), &["namespace", "path"], &[]),
                variant(Some("list"), &["namespace"], &["path"]),
                variant(
                    Some("write"),
                    &["namespace", "path"],
                    &["text", "base64", "expected_revision"],
                ),
                variant(
                    Some("append"),
                    &["namespace", "path"],
                    &["text", "base64", "expected_revision"],
                ),
                variant(
                    Some("move"),
                    &["namespace", "path", "destination"],
                    &["expected_revision"],
                ),
                variant(
                    Some("copy"),
                    &["namespace", "path", "destination"],
                    &["expected_revision"],
                ),
                variant(
                    Some("delete"),
                    &["namespace", "path"],
                    &["recursive", "expected_revision"],
                ),
            ],
        ),
        (
            "state_db",
            "Named SQLite databases, parameterized SQL, introspection and migrations.",
            vec![
                variant(Some("list"), &["namespace"], &[]),
                variant(
                    Some("create"),
                    &["namespace", "database"],
                    &["expected_revision"],
                ),
                variant(
                    Some("drop"),
                    &["namespace", "database"],
                    &["expected_revision"],
                ),
                variant(
                    Some("query"),
                    &["namespace", "database", "sql"],
                    &["params"],
                ),
                variant(
                    Some("execute"),
                    &["namespace", "database", "sql"],
                    &["params", "expected_revision"],
                ),
                variant(Some("inspect"), &["namespace", "database"], &[]),
                variant(Some("migrations"), &["namespace", "database"], &[]),
                variant(
                    Some("migrate"),
                    &["namespace", "database", "migrations"],
                    &["expected_revision"],
                ),
            ],
        ),
        (
            "state_function",
            "Publish pinned code and grants. Use describe mode=runtime for host APIs and an example; get retrieves full pinned source.",
            vec![
                variant(Some("list"), &["namespace"], &[]),
                variant(Some("get"), &["namespace", "name"], &["expected_version"]),
                variant(
                    Some("remove"),
                    &["namespace", "name"],
                    &["expected_version", "expected_revision"],
                ),
                variant(
                    Some("declare"),
                    &["namespace", "name", "file", "symbol"],
                    &[
                        "input_schema",
                        "output_schema",
                        "databases",
                        "files",
                        "calls",
                        "description",
                        "expected_version",
                        "expected_revision",
                    ],
                ),
                variant(
                    Some("update"),
                    &["namespace", "name", "file", "symbol"],
                    &[
                        "input_schema",
                        "output_schema",
                        "databases",
                        "files",
                        "calls",
                        "description",
                        "expected_version",
                        "expected_revision",
                    ],
                ),
            ],
        ),
        (
            "state_call",
            "Invoke an endpoint with nested operations in one root transaction.",
            vec![variant(
                None,
                &["namespace", "function"],
                &["arguments", "expected_version", "idempotency_key"],
            )],
        ),
        (
            "state_execute",
            "Execute a Monty script using transactional host APIs.",
            vec![variant(
                None,
                &["script"],
                &["namespace", "inputs", "idempotency_key"],
            )],
        ),
        (
            "state_describe",
            "Discover a compact overview (default), runtime guide (mode=runtime), full schemas (mode=full), one tool (tool), or endpoint contracts (namespace, function).",
            vec![
                variant(None, &[], &["mode"]),
                variant(None, &["tool"], &[]),
                variant(None, &["namespace"], &["function"]),
            ],
        ),
    ];
    tools
        .into_iter()
        .map(|(name, description, variants)| {
            let schema = if variants.len() == 1 {
                variants.into_iter().next().expect("one")
            } else {
                json!({"type":"object","oneOf":variants})
            };
            json!({"name":name,"description":description,"inputSchema":schema})
        })
        .collect()
}

fn tool_description(operation: Operation) -> &'static str {
    match operation {
        Operation::Namespace(crate::NamespaceAction::Create) => {
            "Create an empty namespace for files, SQLite databases, and published functions."
        }
        Operation::Namespace(crate::NamespaceAction::List) => {
            "List namespaces with their names, stable IDs, revisions, and resource counts."
        }
        Operation::Namespace(crate::NamespaceAction::Get) => {
            "Get a namespace’s name, stable ID, revision, and resource counts."
        }
        Operation::Namespace(crate::NamespaceAction::Update) => {
            "Rename a namespace while preserving its stable ID and contents."
        }
        Operation::Namespace(crate::NamespaceAction::Copy) => {
            "Copy a namespace and its contents to a new name. Subsequent changes are independent."
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
            "Drop a named SQLite database. Remove function declarations that reference it first."
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
            "List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts, and access grants without source code. Use function.get with namespace and name for a full declaration."
        }
        Operation::Function(crate::FunctionAction::Get) => {
            "Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, grants, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again."
        }
        Operation::Function(crate::FunctionAction::Remove) => {
            "Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected."
        }
        Operation::Function(crate::FunctionAction::Declare) => {
            "Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Explicit database, file, and callee grants control host access; omitted grants allow none. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls."
        }
        Operation::Function(crate::FunctionAction::Update) => {
            "Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete schemas and grants: omitted grants allow none, rather than preserving the previous grants. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version."
        }
        Operation::Call => {
            "Invoke a published function using namespace, function, and an arguments object matching its input schema. Returns the function's JSON value directly. The function runs with its declared grants; nested calls and writes share one transaction and failures roll back changes. expected_version guards against calling changed code. Reuse an idempotency_key only with the identical request to replay a completed result without repeating writes."
        }
        Operation::Execute => {
            "Run a Python script with owner access in one transaction. The inputs argument is available as the Python variable inputs; the final expression and printed output are returned as {value, stdout}. Set namespace to use db_query(database, sql, params=[]), db_execute(database, sql, params=[]), db_inspect(database), read_text(path), and write_text(path, text). SQL helpers return {columns, rows, rows_affected}. Use mcp(name, arguments={}) for any tool or call(namespace, function, arguments={}) for a published function. Failures roll back all nested writes. The runtime has no package loader or virtual-file imports. Reuse an idempotency_key only for an identical request."
        }
        Operation::Describe => {
            "Get a compact API overview, mode=runtime for a Python authoring guide, or mode=full for all schemas, or mode=readme for the bundled documentation. Select tool for one tool’s schema, or namespace and optional function for endpoint contracts."
        }
    }
}

/// MCP tools have one operation per name and no action discriminator.
pub fn tool_definitions() -> Vec<Value> {
    operation_definitions().into_iter().flat_map(|tool| {
        let family: Tool = tool["name"].as_str().expect("tool name").parse().expect("canonical tool");
        if matches!(family, Tool::Call | Tool::Execute | Tool::Describe) {
            let operation = family.with_action(None).expect("no action");
            let mut tool = tool;
            tool["name"] = json!(family.public_name());
            tool["description"] = json!(tool_description(operation));
            return vec![tool];
        }
        tool["inputSchema"]["oneOf"].as_array().expect("actions").iter().map(|schema| {
            let action = schema["properties"]["action"]["const"].as_str().expect("action");
            let operation = family.with_action(Some(action)).expect("canonical action");
            let mut schema = schema.clone();
            schema["properties"].as_object_mut().unwrap().remove("action");
            schema["required"].as_array_mut().unwrap().retain(|key| key != "action");
            json!({"name":operation.public_name(),"description":tool_description(operation),"inputSchema":schema})
        }).collect()
    }).collect()
}

// Preserve existing embedded/CLI calls and persisted Python modules. MCP itself
// advertises and accepts only the public names from tool_definitions().
pub(crate) fn normalize_call(name: &str, mut args: Value) -> Result<(Tool, Value)> {
    if let Ok(tool) = name.parse::<Tool>() {
        return Ok((tool, args));
    }
    let operation =
        Operation::from_public_name(name).map_err(|_| Error::invalid("unknown statemcp tool"))?;
    if let Some(action) = operation.action_name() {
        let object = args
            .as_object_mut()
            .ok_or_else(|| Error::invalid("arguments must be an object"))?;
        if object.contains_key("action") {
            return Err(Error::invalid(
                "action is implied by the tool name; omit the action argument",
            ));
        }
        validate_operation(name, &args)?;
        args.as_object_mut()
            .expect("object")
            .insert("action".into(), json!(action));
    } else {
        validate_operation(name, &args)?;
    }
    Ok((operation.tool(), args))
}

pub(crate) fn validate_operation(tool: &str, args: &Value) -> Result<()> {
    static VALIDATORS: OnceLock<BTreeMap<String, jsonschema::Validator>> = OnceLock::new();
    let validators = VALIDATORS.get_or_init(|| {
        operation_definitions()
            .into_iter()
            .chain(tool_definitions())
            .map(|tool| {
                (
                    tool["name"].as_str().expect("tool name").into(),
                    jsonschema::validator_for(&tool["inputSchema"]).expect("static schema"),
                )
            })
            .collect()
    });
    let validator = validators
        .get(tool)
        .ok_or_else(|| Error::invalid("unknown statemcp tool"))?;
    validator
        .validate(args)
        .map_err(|e| Error::invalid(format!("{tool}: {e}")))?;
    let grouped = tool.parse::<Tool>().ok();
    let operation = grouped.map(|tool| tool.operation(args)).transpose()?;
    if matches!(
        operation,
        Some(Operation::File(
            crate::FileAction::Write | crate::FileAction::Append
        ))
    ) && (args.get("text").is_some() == args.get("base64").is_some())
    {
        return Err(Error::invalid(
            "file writes require exactly one of text or base64",
        ));
    }
    if grouped == Some(Tool::Describe)
        && args.get("function").is_some()
        && args.get("namespace").is_none()
    {
        return Err(Error::invalid("function discovery requires a namespace"));
    }
    Ok(())
}

pub(crate) fn compile(schema: &Value) -> Result<jsonschema::Validator> {
    if schema.to_string().len() > 64 * 1024 {
        return Err(Error::limit("endpoint schema exceeds 64 KiB"));
    }
    check_references(schema, 0)?;
    jsonschema::meta::validate(schema)
        .map_err(|e| Error::invalid(format!("invalid JSON schema: {e}")))?;
    jsonschema::options()
        .with_retriever(DenyRetrieval)
        .build(schema)
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
pub(crate) fn runtime_guide() -> Value {
    let mut guide = json!({
        "runtime":"Pydantic Monty",
        "abi_version":1,
        "compatibility":"Only ABI 1 is supported. Incompatible host signatures, invocation conventions, or result representations require an ABI increment. Unsupported declarations fail with ABI_MISMATCH and expected/actual versions; adapt source and redeclare. No automatic migration or older-ABI support is provided.",
        "execution":"Endpoint arguments are keyword arguments to the pinned Python symbol. Return a JSON-compatible value. Scripts use inputs and return their final expression as {value, stdout}. Helpers use the endpoint namespace or execute.namespace. No package loader or virtual-file imports. Module initialization cannot call host APIs.",
        "hosts":[
            {"signature":"mcp(name, arguments={})","returns":"The selected state tool result; endpoint grants still apply."},
            {"signature":"call(namespace, function, arguments={})","returns":"Endpoint JSON value."},
            {"signature":"db_query(database, sql, params=[])","returns":"{columns: [string], rows: [[value]], rows_affected: integer}; one read-only statement."},
            {"signature":"db_execute(database, sql, params=[])","returns":"{columns: [string], rows: [[value]], rows_affected: integer}; one statement, supports RETURNING."},
            {"signature":"db_inspect(database)","returns":"Database name, id, snapshot, staged flag, schema_fingerprint, migrations, schema query result, and tables with columns/indexes/foreign_keys."},
            {"signature":"read_text(path)","returns":"UTF-8 string."},
            {"signature":"write_text(path, text)","returns":"File write metadata."}
        ],
        "grants":{
            "databases":"[{database, access: read|write|migrate}]; write includes read, migrate includes write. Grants pin database identities.",
            "files":"[{path, access: read|write}]; write includes read.",
            "calls":"[{namespace, function}]; self follows namespace copies. External grants pin namespace UUIDs; use that UUID in call().",
            "default":"No grants. Published endpoints cannot manage namespaces or declare functions. Root scripts can compose all tools."
        },
        "transactions":"Nested host calls share the root transaction. Failure rolls back staged effects. Use expected_revision for namespace concurrency and expected_version for endpoint concurrency.",
        "snapshot":"An opaque immutable SQLite file identifier, not a content hash or namespace revision. inspect reports the referenced snapshot; staged=true means pending writes are not represented by that snapshot. Unreferenced snapshots can be removed by maintenance. No historical read or restore API accepts this identifier.",
        "example":[
            {"tool":"state_namespace","arguments":{"action":"create","name":"demo"}},
            {"tool":"state_db","arguments":{"action":"create","namespace":"demo","database":"app"}},
            {"tool":"state_db","arguments":{"action":"execute","namespace":"demo","database":"app","sql":"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT)"}},
            {"tool":"state_fs","arguments":{"action":"write","namespace":"demo","path":"/api.py","text":"def add(text):\n    return db_execute('app', 'INSERT INTO notes(text) VALUES (?) RETURNING id, text', [text])['rows'][0]\n"}},
            {"tool":"state_function","arguments":{"action":"declare","namespace":"demo","name":"add","file":"/api.py","symbol":"add","databases": [{"database":"app","access":"write"}],"input_schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}}},
            {"tool":"state_call","arguments":{"namespace":"demo","function":"add","arguments":{"text":"hello"}}}
        ]
    });
    for step in guide["example"].as_array_mut().expect("example") {
        let tool: Tool = step["tool"]
            .as_str()
            .unwrap()
            .parse()
            .expect("example tool");
        let operation = tool
            .operation(&step["arguments"])
            .expect("example operation");
        step["arguments"].as_object_mut().unwrap().remove("action");
        step["tool"] = json!(operation.public_name());
    }
    guide
}

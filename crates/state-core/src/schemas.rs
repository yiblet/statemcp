use crate::{Error, Result};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

fn field(name: &str) -> Value {
    match name {
        "inputs" => json!({}),
        "arguments" => json!({"type":"object"}),
        "input_schema" | "output_schema" => json!({"type":["object","boolean"]}),
        "recursive" => json!({"type":"boolean"}),
        "params" => json!({"type":"array"}),
        "databases" => {
            json!({"type":"object","additionalProperties":{"enum":["read","write","migrate"]}})
        }
        "files" => json!({"type":"object","additionalProperties":{"enum":["read","write"]}}),
        "calls" => {
            json!({"type":"array","items":{"type":"object","properties":{"namespace":{"type":"string"},"function":{"type":"string"}},"required":["namespace","function"],"additionalProperties":false}})
        }
        "migrations" => {
            json!({"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"sql":{"type":"string"}},"required":["id","sql"],"additionalProperties":false}})
        }
        _ => json!({"type":"string"}),
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
        if name != "action" {
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
pub fn tool_definitions() -> Vec<Value> {
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
            "Publish pinned code, JSON contracts, database/file/callee grants.",
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
            "Discover fixed operations or endpoint contracts without source code.",
            vec![variant(None, &[], &["namespace", "function"])],
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

pub(crate) fn validate_operation(tool: &str, args: &Value) -> Result<()> {
    static VALIDATORS: OnceLock<BTreeMap<String, jsonschema::Validator>> = OnceLock::new();
    let validators = VALIDATORS.get_or_init(|| {
        tool_definitions()
            .into_iter()
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
        .ok_or_else(|| Error::invalid("unknown State MCP tool"))?;
    validator
        .validate(args)
        .map_err(|e| Error::invalid(format!("{tool}: {e}")))?;
    if tool == "state_fs"
        && matches!(args["action"].as_str(), Some("write" | "append"))
        && (args.get("text").is_some() == args.get("base64").is_some())
    {
        return Err(Error::invalid(
            "file writes require exactly one of text or base64",
        ));
    }
    if tool == "state_describe" && args.get("function").is_some() && args.get("namespace").is_none()
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
                if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef")
                    && !value.as_str().is_some_and(|r| r.starts_with('#'))
                {
                    return Err(Error::invalid(
                        "schema references must be local fragments; external retrieval is disabled",
                    ));
                }
                if key == "$schema"
                    && !matches!(
                        value.as_str(),
                        Some(
                            "http://json-schema.org/draft-04/schema#"
                                | "http://json-schema.org/draft-06/schema#"
                                | "http://json-schema.org/draft-07/schema#"
                                | "https://json-schema.org/draft/2019-09/schema"
                                | "https://json-schema.org/draft/2020-12/schema"
                        )
                    )
                {
                    return Err(Error::invalid(
                        "only built-in JSON Schema dialects are supported",
                    ));
                }
                // Resource IDs complicate local reference resolution; local anchors/$defs suffice.
                if key == "$id" {
                    return Err(Error::invalid(
                        "schema $id resources are unsupported; use local $defs",
                    ));
                }
                match key.as_str() {
                    // These hold instance data, not schemas.
                    "const" | "enum" | "default" | "examples" => {}
                    // Names in these maps are user property/resource names, not keywords.
                    "properties" | "patternProperties" | "$defs" | "definitions"
                    | "dependentSchemas" | "dependencies" => {
                        if let Some(map) = value.as_object() {
                            for schema in map.values() {
                                check_references(schema, depth + 1)?;
                            }
                        }
                    }
                    _ => check_references(value, depth + 1)?,
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

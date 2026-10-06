mod common;
use common::{Directory, connect, request, tool, value};
use rmcp::{ServiceExt, transport::TokioChildProcess};
use serde_json::{Value, json};
use statemcp::State;
fn tool_definitions() -> Vec<Value> {
    statemcp::tool_definitions()
        .into_iter()
        .map(|tool| serde_json::to_value(tool).unwrap())
        .collect()
}
use std::process::Command;

#[tokio::test]
async fn official_sdk_client_discovers_and_calls_tools() {
    let directory = Directory::new();
    let client = connect(State::open(&directory.0).unwrap()).await;
    assert_eq!(
        client
            .peer_info()
            .unwrap()
            .server_info
            .as_ref()
            .unwrap()
            .name,
        "statemcp"
    );
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 30);
    for (actual, expected) in tools.iter().zip(tool_definitions()) {
        assert_eq!(actual.name, expected["name"].as_str().unwrap());
        assert_eq!(
            serde_json::to_value(&actual.input_schema).unwrap(),
            expected["inputSchema"]
        );
    }
    assert_eq!(
        tool(
            &client,
            "execute",
            json!({"script":"inputs + 1", "inputs":41})
        )
        .await["value"],
        42
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn wire_results_preserve_json_values_and_application_errors() {
    let directory = Directory::new();
    let client = connect(State::open(&directory.0).unwrap()).await;
    let created = tool(
        &client,
        "namespace.create",
        json!({"name":"values", "description":"Protocol values"}),
    )
    .await;
    assert_eq!(created["description"], "Protocol values");
    let updated = tool(
        &client,
        "namespace.update",
        json!({"namespace":"values", "description":null}),
    )
    .await;
    assert_eq!(updated["description"], Value::Null);

    tool(
        &client,
        "fs.write",
        json!({"namespace":"values","path":"/api.py","text":"def echo(value):\n    return value"}),
    )
    .await;
    tool(
        &client,
        "function.declare",
        json!({"namespace":"values","name":"echo","file":"/api.py","symbol":"echo"}),
    )
    .await;
    for expected in [
        json!({"nested":[true,null,"quoted text\n"]}),
        json!([1, 2]),
        json!(42),
        json!("text"),
        json!(false),
        Value::Null,
    ] {
        assert_eq!(
            tool(
                &client,
                "call",
                json!({"namespace":"values","function":"echo","arguments":{"value":expected}})
            )
            .await,
            expected
        );
    }
    let failure = client
        .call_tool(request(
            "fs.read",
            json!({"namespace":"values","path":"/missing"}),
        ))
        .await
        .unwrap();
    assert_eq!(failure.is_error, Some(true));
    assert_eq!(value(&failure)["error"]["code"], "NOT_FOUND");
    let invalid = client
        .call_tool(request(
            "namespace.create",
            json!({"name":"invalid","unexpected":true}),
        ))
        .await
        .unwrap();
    assert_eq!(invalid.is_error, Some(true));
    assert!(invalid.content.iter().any(|block| matches!(block, rmcp::model::ContentBlock::Text(text) if text.text.contains("unexpected"))));
    assert!(
        client
            .call_tool(request("unknown", json!({})))
            .await
            .is_err()
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn official_sdk_client_launches_binary_over_stdio() {
    let directory = Directory::new();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_statemcp"));
    command.arg("stdio").arg(&directory.0);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = common::config().serve(transport).await.unwrap();
    assert_eq!(client.list_all_tools().await.unwrap().len(), 30);
    assert_eq!(
        tool(&client, "execute", json!({"script":"40 + 2"})).await["value"],
        42
    );
    client.cancel().await.unwrap();
}

#[test]
fn reference_schemas_match_the_advertised_tools() {
    let reference = include_str!("../reference.md");
    for tool in tool_definitions() {
        let heading = format!("### `{}`\n", tool["name"].as_str().unwrap());
        let section = reference.split_once(&heading).expect("documented tool").1;
        let schema = section
            .split_once("```json\n")
            .unwrap()
            .1
            .split_once("\n```")
            .unwrap()
            .0;
        assert_eq!(
            serde_json::from_str::<Value>(schema).unwrap(),
            tool["inputSchema"]
        );
    }
}

#[test]
fn schemas_advertise_exact_fixed_surface_and_required_properties() {
    let definitions = tool_definitions();
    let names: Vec<_> = definitions
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 30);
    assert!(names.contains(&"db.create"));
    assert!(names.contains(&"db.query"));
    assert!(names.contains(&"namespace.copy"));
    assert!(names.contains(&"execute"));
    assert!(names.iter().all(|name| !name.starts_with("state_")));
    for tool in &definitions {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object");
        let variants = if schema.get("anyOf").is_none() {
            vec![schema]
        } else {
            schema["anyOf"].as_array().unwrap().iter().collect()
        };
        for variant in variants {
            let variant = if let Some(reference) = variant["$ref"].as_str() {
                &schema["$defs"][reference.strip_prefix("#/$defs/").unwrap()]
            } else {
                variant
            };
            assert_eq!(variant["additionalProperties"], false);
            assert!(variant["properties"].get("action").is_none());
            if let Some(required) = variant["required"].as_array() {
                for key in required {
                    assert!(variant["properties"].get(key.as_str().unwrap()).is_some());
                }
            }
        }
    }
}

#[test]
fn binary_cli_help_version_and_argument_errors() {
    let binary = env!("CARGO_BIN_EXE_statemcp");
    for argument in ["--help", "--version"] {
        let output = Command::new(binary).arg(argument).output().unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("statemcp")
        );
    }
    for arguments in [
        vec![],
        vec!["stdio"],
        vec!["http"],
        vec!["--unknown"],
        vec!["--data-dir"],
        vec!["stdio", "/unused", "--auth-bearer", "x"],
        vec!["http", "/unused", "--auth-key", "x"],
        vec!["http", "/unused", "--auth-bearer", ""],
        vec!["http", "/unused", "--http-prefix", "/bad//prefix"],
        vec!["--worker"],
        vec!["--worker", "invalid", "1024"],
        vec!["--worker", "1024", "-1"],
        vec!["--worker", "1024", "1024", "unexpected"],
    ] {
        let output = Command::new(binary).args(arguments).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn sdk_macros_and_core_validate_the_same_tool_contracts() {
    let sdk = tool_definitions();
    let core: Vec<Value> = state_core::tool_definitions()
        .into_iter()
        .map(|tool| serde_json::to_value(tool).unwrap())
        .collect();
    for actual in sdk {
        let expected = core
            .iter()
            .find(|tool| tool["name"] == actual["name"])
            .unwrap();
        assert_eq!(actual["inputSchema"], expected["inputSchema"]);
        assert_eq!(actual["description"], expected["description"]);
    }
}

#[tokio::test]
async fn sdk_routing_preserves_original_idempotency_inputs() {
    let directory = Directory::new();
    let client = connect(State::open(&directory.0).unwrap()).await;
    let args = json!({"script":"inputs", "idempotency_key":"original-wire-input"});
    assert_eq!(
        tool(&client, "execute", args.clone()).await,
        tool(&client, "execute", args).await
    );
    let changed = client
        .call_tool(request(
            "execute",
            json!({"script":"inputs", "inputs":null, "idempotency_key":"original-wire-input"}),
        ))
        .await
        .unwrap();
    assert_eq!(value(&changed)["error"]["code"], "IDEMPOTENCY_MISMATCH");
    client.cancel().await.unwrap();
}

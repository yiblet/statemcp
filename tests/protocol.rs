mod common;
use common::{Directory, connect, request, tool, value};
use rmcp::{ServiceExt, transport::TokioChildProcess};
use serde_json::{Value, json};
use statemcp::{State, tool_definitions};
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
    tool(&client, "namespace.create", json!({"name":"values"})).await;
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
    assert_eq!(value(&invalid)["error"]["code"], "INVALID_ARGUMENT");
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
        let variants = if schema.get("properties").is_some() {
            vec![schema]
        } else {
            schema["oneOf"].as_array().unwrap().iter().collect()
        };
        for variant in variants {
            assert!(variant["properties"].get("action").is_none());
            for key in variant["required"].as_array().unwrap() {
                assert!(variant["properties"].get(key.as_str().unwrap()).is_some());
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

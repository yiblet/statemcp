use crate::test_support as common;
use common::{Fixture, connect, request, tool, value};
use serde_json::{Value, json};
fn tool_definitions() -> Vec<Value> {
    crate::tool_definitions()
        .into_iter()
        .map(|tool| serde_json::to_value(tool).unwrap())
        .collect()
}

#[tokio::test]
async fn official_sdk_client_discovers_and_calls_tools() {
    let directory = Fixture::new();
    let client = connect(directory.state_async().await.unwrap()).await;
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
    assert_eq!(tools.len(), 34);
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
    let directory = Fixture::new();
    let client = connect(directory.state_async().await.unwrap()).await;
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
    assert_eq!(names.len(), 34);
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
    let directory = Fixture::new();
    let client = connect(directory.state_async().await.unwrap()).await;
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

#[derive(Default)]
struct Gate {
    released: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
}
impl Gate {
    fn wait(&self) -> state_core::Result<()> {
        let mut released = self.released.lock().map_err(|_| {
            state_core::Error::new(state_core::ErrorCode::WorkerFailed, "poisoned gate")
        })?;
        while !*released {
            released = self.wake.wait(released).map_err(|_| {
                state_core::Error::new(state_core::ErrorCode::WorkerFailed, "poisoned gate")
            })?;
        }
        Ok(())
    }
}
struct ReleaseGate(std::sync::Arc<Gate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        let mut released = self
            .0
            .released
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *released = true;
        self.0.wake.notify_all();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn invocation_limit_is_shared_by_session_clones_and_recovers()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new();
    let server = super::Server::new(state_http::BlockingExecutor::new(
        fixture.state_async().await?,
    ));
    let other_session = server.clone();
    let gate = std::sync::Arc::new(Gate::default());
    let release = ReleaseGate(gate.clone());
    let (started, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let mut calls = Vec::new();
    for _ in 0..16 {
        let server = server.clone();
        let gate = gate.clone();
        let started = started.clone();
        calls.push(tokio::spawn(async move {
            server
                .run_blocking(move |_| {
                    started.send(()).map_err(|_| {
                        state_core::Error::new(
                            state_core::ErrorCode::WorkerFailed,
                            "closed start channel",
                        )
                    })?;
                    gate.wait()?;
                    Ok(42)
                })
                .await
        }));
    }
    // Every arrival is retained by the channel, even if several jobs start
    // before the async test polls again. No sleep or polling race is needed.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        for _ in 0..16 {
            entered.recv().await.ok_or("closed start channel")?;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await??;
    let busy = other_session.run_blocking(|_| Ok(0)).await?;
    assert_eq!(busy.is_error, Some(true));
    assert_eq!(common::value(&busy)["error"]["code"], "BUSY");
    drop(release);
    for call in calls {
        assert_eq!(common::value(&call.await??), 42);
    }
    assert_eq!(
        common::value(&other_session.run_blocking(|_| Ok(42)).await?),
        42
    );
    Ok(())
}

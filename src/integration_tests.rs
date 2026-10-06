use crate::test_support as common;
use serde_json::json;
use state_core::inputs::{ExecuteInput, FsReadInput};

#[tokio::test]
async fn embedded_service_dispatches_through_mcp_and_rolls_back_failed_scripts()
-> anyhow::Result<()> {
    let fixture = common::Fixture::new();
    let state = fixture.state_async().await?;
    let client = common::connect(state.clone()).await;
    let response = client
        .call_tool(common::request("namespace.create", json!({"name":"app"})))
        .await?;
    assert_eq!(common::value(&response)["name"], "app");
    let response = client
        .call_tool(common::request(
            "execute",
            json!({"namespace":"app","script":"write_text('/failed.txt', 'rollback')\n1 / 0"}),
        ))
        .await?;
    assert_eq!(response.is_error, Some(true));
    let check_state = state.clone();
    let file = tokio::task::spawn_blocking(move || {
        check_state.read_file(FsReadInput {
            namespace: "app".into(),
            path: "/failed.txt".into(),
        })
    })
    .await?;
    assert_eq!(
        file.err().map(|error| error.code),
        Some(state_core::ErrorCode::NotFound)
    );
    let result = common::tool(
        &client,
        "execute",
        json!({"namespace":"app","script":"write_text('/ok.txt', 'saved')\nread_text('/ok.txt')"}),
    )
    .await;
    assert_eq!(result["value"], "saved");
    client.cancel().await?;
    drop(state);
    let reopened = fixture.state_async().await?;
    let file = tokio::task::spawn_blocking(move || {
        reopened.read_file(FsReadInput {
            namespace: "app".into(),
            path: "/ok.txt".into(),
        })
    })
    .await??;
    assert!(
        matches!(file, state_core::records::FileContents::Text { text, .. } if text == "saved")
    );
    Ok(())
}

#[test]
fn callbacks_publish_and_maintenance_runs_in_memory() -> anyhow::Result<()> {
    let fixture = common::Fixture::new();
    let state = fixture.state()?;
    let input: ExecuteInput = serde_json::from_value(
        json!({"script":"mcp('namespace.create', {'name': 'cli'})\nmcp('fs.write', {'namespace': 'cli', 'path': '/value.txt', 'text': 'persisted'})\n42"}),
    )?;
    let result = state.execute_script(input, None)?;
    assert_eq!(result.value, 42);
    drop(state);
    let reopened = fixture.state()?;
    let file = reopened.read_file(FsReadInput {
        namespace: "cli".into(),
        path: "/value.txt".into(),
    })?;
    assert!(
        matches!(file, state_core::records::FileContents::Text { text, .. } if text == "persisted")
    );
    let report = fixture.store()?.maintenance(0)?;
    assert!(serde_json::to_value(report)?["revisions_removed"].is_number());
    Ok(())
}

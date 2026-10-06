use super::common::{self, Directory, tool};
use rmcp::{ServiceExt, transport::TokioChildProcess};
use serde_json::json;
use std::process::Command;

#[tokio::test]
async fn official_sdk_client_launches_binary_over_stdio() {
    let _serial = super::common::serial_async().await;
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
fn binary_cli_help_version_and_argument_errors() {
    let _serial = super::common::serial();
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

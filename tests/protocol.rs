use serde_json::{Value, json};
fn tool_value(result: &Value) -> Value {
    assert!(result.get("structuredContent").is_none());
    result["content"].clone()
}

use statemcp::{
    Server, ToolError, UnsupportedDispatcher, protocol::MAX_FRAME_BYTES, tool_definitions,
};
use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

fn request(id: impl Into<Value>, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id.into(),"method":method,"params":params})
}
fn initialize<D: statemcp::Dispatcher>(server: &mut Server<D>) {
    let response = server.handle(request(1, "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"tests","version":"1"}}))).unwrap();
    assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
    assert!(
        server
            .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none()
    );
}

#[test]
fn lifecycle_negotiates_supported_version_and_exact_capabilities() {
    let mut server = Server::new(UnsupportedDispatcher);
    assert_eq!(
        server.handle(request(0, "tools/list", json!({}))).unwrap()["error"]["code"],
        -32002
    );
    assert_eq!(
        server.handle(request(0, "ping", json!({}))).unwrap()["result"],
        json!({})
    );
    let response = server.handle(request("init","initialize",json!({"protocolVersion":"future-version","capabilities":{},"clientInfo":{"name":"tests","version":"1"}}))).unwrap();
    assert_eq!(response["id"], "init");
    assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        response["result"]["capabilities"],
        json!({"tools":{"listChanged":false}})
    );
    assert_eq!(
        server.handle(request(2, "tools/list", json!({}))).unwrap()["error"]["code"],
        -32002
    );
    server.handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    assert_eq!(
        server.handle(request(3, "tools/list", json!({}))).unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        30
    );
    assert_eq!(
        server.handle(request(4, "initialize", json!({}))).unwrap()["error"]["code"],
        -32600
    );
}

#[test]
fn malformed_initialization_does_not_advance_session() {
    let mut server = Server::new(UnsupportedDispatcher);
    assert_eq!(
        server
            .handle(request(
                1,
                "initialize",
                json!({"protocolVersion":"2025-06-18"})
            ))
            .unwrap()["error"]["code"],
        -32602
    );
    initialize(&mut server);
}

#[test]
fn tool_result_preserves_structure_text_and_service_errors() {
    let mut observed = Vec::new();
    let mut server = Server::new(|tool: &str, args: Value| {
        observed.push((tool.to_owned(), args.clone()));
        if tool == "namespace.delete" {
            Err(ToolError::new("NOT_FOUND", "namespace does not exist"))
        } else {
            Ok(json!({"name":"notes","revision":42}))
        }
    });
    initialize(&mut server);
    let result = server
        .handle(request(
            "call",
            "tools/call",
            json!({"name":"namespace.create","arguments":{"name":"notes"}}),
        ))
        .unwrap();
    assert_eq!(result["result"]["isError"], false);
    assert_eq!(tool_value(&result["result"])["revision"], 42);
    assert_eq!(
        result["result"]["content"],
        json!({"name":"notes","revision":42})
    );
    let error = server
        .handle(request(
            3,
            "tools/call",
            json!({"name":"namespace.delete","arguments":{}}),
        ))
        .unwrap();
    assert_eq!(error["result"]["isError"], true);
    assert_eq!(tool_value(&error["result"])["error"]["code"], "NOT_FOUND");
    assert!(error.get("error").is_none());
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0].0, "namespace.create");
    assert_eq!(observed[0].1["name"], "notes");
}

#[test]
fn content_preserves_all_json_values() {
    for expected in [
        json!([1, 2]),
        json!(42),
        json!(false),
        json!(null),
        json!({"value":"user data"}),
    ] {
        let mut server = Server::new(|_: &str, _: Value| Ok(expected.clone()));
        initialize(&mut server);
        let response = server
            .handle(request(1, "tools/call", json!({"name":"describe"})))
            .unwrap();
        assert_eq!(tool_value(&response["result"]), expected);
    }
}

#[test]
fn notifications_and_invalid_calls_never_dispatch() {
    let mut server = Server::new(|_: &str, _: Value| -> Result<Value, ToolError> {
        panic!("must not dispatch")
    });
    initialize(&mut server);
    assert!(
        server
            .handle(
                json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"state_namespace"}})
            )
            .is_none()
    );
    assert!(
        server
            .handle(
                json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}})
            )
            .is_none()
    );
    for params in [
        json!({"name":"unknown"}),
        json!({"name":"state_db","arguments":{"action":"query"}}),
        json!({"name":"execute","arguments":[]}),
        json!({}),
        json!([]),
    ] {
        assert_eq!(
            server.handle(request(2, "tools/call", params)).unwrap()["error"]["code"],
            -32602
        );
    }
    assert_eq!(
        server
            .handle(request(3, "not-a-method", json!({})))
            .unwrap()["error"]["code"],
        -32601
    );
    assert_eq!(
        server
            .handle(request(4, "tools/list", json!({"cursor":"bogus"})))
            .unwrap()["error"]["code"],
        -32602
    );
}

#[test]
fn invalid_envelopes_have_stable_errors() {
    let mut server = Server::new(UnsupportedDispatcher);
    for invalid in [
        json!([]),
        json!(42),
        json!({}),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":false,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1}),
    ] {
        assert_eq!(server.handle(invalid).unwrap()["error"]["code"], -32600);
    }
    assert!(
        server
            .handle(json!({"jsonrpc":"2.0","id":1,"result":{}}))
            .is_none()
    );
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
fn transport_recovers_after_malformed_json_and_oversize_frames() {
    let mut input = b"not-json\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', MAX_FRAME_BYTES + 64));
    input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/unknown\"}\n{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"ping\"}\n");
    let mut output = Vec::new();
    Server::new(UnsupportedDispatcher)
        .serve(Cursor::new(input), &mut output)
        .unwrap();
    let messages: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["error"]["code"], -32700);
    assert_eq!(messages[1]["error"]["code"], -32600);
    assert_eq!(messages[2]["id"], 99);
    assert_eq!(messages[2]["result"], json!({}));
}

#[test]
fn binary_stdio_handshake_and_shutdown() {
    let data_dir =
        std::env::temp_dir().join(format!("statemcp-protocol-test-{}", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_statemcp"))
        .arg("stdio")
        .arg(&data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for message in [
        request(
            1,
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"integration","version":"1"}}),
        ),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        request(2, "tools/list", json!({})),
    ] {
        writeln!(input, "{message}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let messages: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    if data_dir.exists() {
        std::fs::remove_dir_all(data_dir).unwrap();
    }
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["result"]["tools"].as_array().unwrap().len(), 30);
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
    ] {
        let output = Command::new(binary).args(arguments).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

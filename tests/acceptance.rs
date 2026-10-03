use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("state-mcp-demo-{}-{suffix}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    id: u64,
}
impl Client {
    fn open(path: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_state-mcp"))
            .arg("--data-dir")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            input,
            output,
            id: 0,
        };
        client.request("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"acceptance","version":"1"}}));
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }
    fn send(&mut self, message: Value) {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{message}").unwrap();
        input.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params}));
        let mut line = String::new();
        assert!(
            self.output.read_line(&mut line).unwrap() > 0,
            "server exited"
        );
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], self.id);
        assert!(reply.get("error").is_none(), "{reply}");
        reply["result"].clone()
    }
    fn tool(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.request("tools/call", json!({"name":tool,"arguments":arguments}));
        assert_ne!(result["isError"], true, "{result}");
        result["structuredContent"].clone()
    }
    fn call(&mut self, namespace: &str, function: &str, arguments: Value) -> Value {
        let result = self.tool(
            "state_call",
            json!({"namespace":namespace,"function":function,"arguments":arguments}),
        );
        result.get("value").cloned().unwrap_or(result)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        if std::thread::panicking() {
            let _ = self.child.kill();
        }
        let status = self.child.wait().unwrap();
        if !std::thread::panicking() {
            assert!(status.success());
        }
    }
}

fn install(client: &mut Client) {
    let fixtures: Value = serde_json::from_str(include_str!("../examples/setup.json")).unwrap();
    for fixture in fixtures.as_array().unwrap() {
        let namespace = &fixture["namespace"];
        client.tool(
            "state_namespace",
            json!({"action":"create","name":namespace}),
        );
        client.tool(
            "state_db",
            json!({"action":"create","namespace":namespace,"database":"app"}),
        );
        client.tool("state_db", json!({"action":"migrate","namespace":namespace,"database":"app","migrations":[{"id":"initial","sql":fixture["sql"]}]}));
        let source = match fixture["file"].as_str().unwrap() {
            "notes.py" => include_str!("../examples/notes.py"),
            "todos.py" => include_str!("../examples/todos.py"),
            "chat.py" => include_str!("../examples/chat.py"),
            other => panic!("unknown example {other}"),
        };
        client.tool(
            "state_fs",
            json!({"action":"write","namespace":namespace,"path":"/api.py","text":source}),
        );
        for endpoint in fixture["endpoints"].as_array().unwrap() {
            let mut declaration = endpoint.clone();
            declaration["action"] = json!("declare");
            declaration["namespace"] = namespace.clone();
            declaration["file"] = json!("/api.py");
            client.tool("state_function", declaration);
        }
    }
}

#[test]
fn real_stdio_notes_todos_chat_persist_and_compose() {
    let directory = Directory::new();
    let mut first = Client::open(&directory.0);
    install(&mut first);
    assert_eq!(
        first.call("notes", "add", json!({"text":"persistent note"})),
        json!([1, "persistent note"])
    );
    assert_eq!(
        first.call("todos", "add", json!({"text":"ship MVP"})),
        json!([1, "ship MVP", 0])
    );
    assert_eq!(
        first.call("todos", "complete", json!({"id":1})),
        json!([[1, "ship MVP", 1]])
    );
    let schema = first.tool(
        "state_db",
        json!({"action":"inspect","namespace":"chat","database":"app"}),
    );
    assert_eq!(schema["tables"][0]["name"], "messages");
    let mut second = Client::open(&directory.0);
    let arguments =
        json!({"room":"shared","sender":"model-a","text":"hello","request_id":"unique-message"});
    assert_eq!(
        first.call("chat", "post", arguments.clone()),
        json!([1, "shared", "model-a", "hello"])
    );
    assert_eq!(
        first.call("chat", "post", arguments),
        json!([1, "shared", "model-a", "hello"])
    );
    assert_eq!(
        second.call("chat", "poll", json!({"room":"shared","after":0})),
        json!([[1, "shared", "model-a", "hello"]])
    );
    assert_eq!(
        second.call("chat", "poll", json!({"room":"shared","after":1})),
        json!([])
    );
    let composed = first.tool("state_execute", json!({"script":"call('notes', 'add', {'text': inputs})\ncall('notes', 'list')","inputs":"composed note","idempotency_key":"compose-once"}));
    assert_eq!(
        composed["value"],
        json!([[1, "persistent note"], [2, "composed note"]])
    );
    drop(second);
    drop(first);
    let mut restarted = Client::open(&directory.0);
    assert_eq!(
        restarted.call("notes", "list", json!({})),
        composed["value"]
    );
    assert_eq!(
        restarted.call("todos", "list", json!({})),
        json!([[1, "ship MVP", 1]])
    );
    assert_eq!(
        restarted.call("chat", "poll", json!({"room":"shared"})),
        json!([[1, "shared", "model-a", "hello"]])
    );
    // TODO(acceptance): exercise acknowledgements and lease fencing once those APIs exist.
}

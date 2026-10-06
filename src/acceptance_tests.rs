use crate::test_support::{self as common, Fixture};
use serde_json::{Value, json};
struct Client(common::Client);
impl Client {
    async fn open(fixture: &Fixture) -> Self {
        Self(common::connect(fixture.state_async().await.unwrap()).await)
    }
    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        common::tool(&self.0, name, arguments).await
    }
    async fn call(&mut self, namespace: &str, function: &str, arguments: Value) -> Value {
        self.tool(
            "call",
            json!({"namespace":namespace,"function":function,"arguments":arguments}),
        )
        .await
    }
    async fn close(self) {
        self.0.cancel().await.unwrap();
    }
}

async fn install(client: &mut Client) {
    let fixtures: Value = serde_json::from_str(include_str!("../examples/setup.json")).unwrap();
    for fixture in fixtures.as_array().unwrap() {
        let namespace = &fixture["namespace"];
        client
            .tool("namespace.create", json!({"name":namespace}))
            .await;
        client
            .tool("db.create", json!({"namespace":namespace,"database":"app"}))
            .await;
        client.tool("db.migrate", json!({"namespace":namespace,"database":"app","migrations":[{"id":"initial","sql":fixture["sql"]}]})).await;
        let source = match fixture["file"].as_str().unwrap() {
            "notes.py" => include_str!("../examples/notes.py"),
            "todos.py" => include_str!("../examples/todos.py"),
            "chat.py" => include_str!("../examples/chat.py"),
            other => panic!("unknown example {other}"),
        };
        client
            .tool(
                "fs.write",
                json!({"namespace":namespace,"path":"/api.py","text":source}),
            )
            .await;
        for endpoint in fixture["endpoints"].as_array().unwrap() {
            let mut declaration = endpoint.clone();
            declaration["namespace"] = namespace.clone();
            declaration["file"] = json!("/api.py");
            client.tool("function.declare", declaration).await;
        }
    }
}

#[tokio::test]
async fn sdk_notes_todos_chat_reopen_and_compose() {
    let directory = Fixture::new();
    let mut first = Client::open(&directory).await;
    install(&mut first).await;
    assert_eq!(
        first
            .call("notes", "add", json!({"text":"persistent note"}))
            .await,
        json!([1, "persistent note"])
    );
    assert_eq!(
        first.call("todos", "add", json!({"text":"ship MVP"})).await,
        json!([1, "ship MVP", 0])
    );
    assert_eq!(
        first.call("todos", "complete", json!({"id":1})).await,
        json!([[1, "ship MVP", 1]])
    );
    let schema = first
        .tool("db.inspect", json!({"namespace":"chat","database":"app"}))
        .await;
    assert_eq!(schema["tables"][0]["name"], "messages");
    let mut second = Client::open(&directory).await;
    let arguments =
        json!({"room":"shared","sender":"model-a","text":"hello","request_id":"unique-message"});
    assert_eq!(
        first.call("chat", "post", arguments.clone()).await,
        json!([1, "shared", "model-a", "hello"])
    );
    assert_eq!(
        first.call("chat", "post", arguments).await,
        json!([1, "shared", "model-a", "hello"])
    );
    assert_eq!(
        second
            .call("chat", "poll", json!({"room":"shared","after":0}))
            .await,
        json!([[1, "shared", "model-a", "hello"]])
    );
    assert_eq!(
        second
            .call("chat", "poll", json!({"room":"shared","after":1}))
            .await,
        json!([])
    );
    let composed = first.tool("execute", json!({"script":"call('notes', 'add', {'text': inputs})\ncall('notes', 'list')","inputs":"composed note","idempotency_key":"compose-once"})).await;
    assert_eq!(
        composed["value"],
        json!([[1, "persistent note"], [2, "composed note"]])
    );
    second.close().await;
    first.close().await;
    let mut restarted = Client::open(&directory).await;
    assert_eq!(
        restarted.call("notes", "list", json!({})).await,
        composed["value"]
    );
    assert_eq!(
        restarted.call("todos", "list", json!({})).await,
        json!([[1, "ship MVP", 1]])
    );
    assert_eq!(
        restarted
            .call("chat", "poll", json!({"room":"shared"}))
            .await,
        json!([[1, "shared", "model-a", "hello"]])
    );
    restarted.close().await;
    // TODO(acceptance): exercise acknowledgements and lease fencing once those APIs exist.
}

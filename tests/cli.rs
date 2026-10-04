use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("statemcp-cli-{}-{suffix}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, tool: &str, arguments: &[&str], stdin: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_statemcp"))
            .args(["cli", tool])
            .arg(self.0.join("data"))
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        } else {
            child.stdin.take();
        }
        child.wait_with_output().unwrap()
    }
    fn ok(&self, tool: &str, arguments: &[&str]) -> Value {
        self.value(self.run(tool, arguments, None))
    }
    fn value(&self, output: Output) -> Value {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn fails(&self, tool: &str, arguments: &[&str]) -> Value {
        let output = self.run(tool, arguments, None);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        serde_json::from_slice(&output.stderr).unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn all_seven_cli_tools_publish_invoke_and_manipulate_state() {
    let workspace = Workspace::new();
    workspace.ok("state_namespace", &["create", "--name", "notes"]);
    workspace.ok(
        "state_db",
        &["create", "--namespace", "notes", "--database", "app"],
    );
    workspace.ok("state_db", &["migrate", "--namespace", "notes", "--database", "app", "--migrations", r#"[{"id":"initial","sql":"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT NOT NULL)"}]"#]);
    let inspect = workspace.ok(
        "state_db",
        &["inspect", "--namespace", "notes", "--database", "app"],
    );
    assert_eq!(inspect["tables"][0]["name"], "notes");
    let python = "def add(text):\n    return db_execute('app', 'INSERT INTO notes(text) VALUES (?) RETURNING id, text', [text])['rows'][0]";
    workspace.ok(
        "state_fs",
        &[
            "write",
            "--namespace",
            "notes",
            "--path",
            "/api.py",
            "--text",
            python,
        ],
    );
    let schema = workspace.0.join("input.json");
    fs::write(&schema, r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}"#).unwrap();
    let schema_arg = format!("@{}", schema.display());
    workspace.ok(
        "state_function",
        &[
            "declare",
            "--namespace",
            "notes",
            "--name",
            "add",
            "--file",
            "/api.py",
            "--symbol",
            "add",
            "--databases",
            r#"[{"database":"app","access":"write"}]"#,
            "--input-schema",
            &schema_arg,
        ],
    );
    let described = workspace.ok(
        "state_describe",
        &["--namespace", "notes", "--function", "add"],
    );
    assert_eq!(described["functions"][0]["name"], "add");
    assert_eq!(
        workspace.ok(
            "state_call",
            &["notes", "add", "--arguments", r#"{"text":"first"}"#]
        ),
        json!([1, "first"])
    );
    let script = workspace.0.join("compose.py");
    fs::write(&script,"call('notes', 'add', {'text': inputs})\nwrite_text('/composed', inputs)\ndb_query('app', 'SELECT text FROM notes ORDER BY id')").unwrap();
    let script_arg = format!("@{}", script.display());
    let result = workspace.ok(
        "state_execute",
        &[
            &script_arg,
            "--namespace",
            "notes",
            "--inputs",
            "\"second\"",
            "--idempotency-key",
            "once",
        ],
    );
    assert_eq!(result["value"]["rows"], json!([["first"], ["second"]]));
    assert_eq!(
        workspace.ok(
            "state_execute",
            &[
                &script_arg,
                "--namespace",
                "notes",
                "--inputs",
                "\"second\"",
                "--idempotency-key",
                "once"
            ]
        ),
        result
    );
    workspace.ok(
        "state_namespace",
        &["copy", "--namespace", "notes", "--name", "fork"],
    );
    workspace.ok(
        "state_namespace",
        &["rename", "--namespace", "fork", "--name", "renamed"],
    );
    assert_eq!(
        workspace.ok(
            "state_call",
            &["renamed", "add", "--arguments", r#"{"text":"fork"}"#]
        ),
        json!([3, "fork"])
    );
    let count = workspace.ok(
        "state_db",
        &[
            "query",
            "--namespace",
            "notes",
            "--database",
            "app",
            "--sql",
            "SELECT count(*) FROM notes WHERE text != ?",
            "--params",
            "[\"none\"]",
        ],
    );
    assert_eq!(count["rows"], json!([[2]]));
    workspace.ok(
        "state_fs",
        &[
            "copy",
            "--namespace",
            "notes",
            "--path",
            "/composed",
            "--destination",
            "/copy",
        ],
    );
    workspace.ok(
        "state_fs",
        &[
            "move",
            "--namespace",
            "notes",
            "--path",
            "/copy",
            "--destination",
            "/moved",
        ],
    );
    workspace.ok(
        "state_fs",
        &[
            "append",
            "--namespace",
            "notes",
            "--path",
            "/moved",
            "--text",
            "!",
        ],
    );
    assert_eq!(
        workspace.ok(
            "state_fs",
            &["read", "--namespace", "notes", "--path", "/moved"]
        )["text"],
        "second!"
    );
    workspace.ok(
        "state_fs",
        &["delete", "--namespace", "notes", "--path", "/moved"],
    );
    workspace.ok(
        "state_function",
        &["remove", "--namespace", "renamed", "--name", "add"],
    );
    workspace.ok(
        "state_db",
        &["drop", "--namespace", "renamed", "--database", "app"],
    );
    workspace.ok("state_namespace", &["delete", "--namespace", "renamed"]);
    let tools = workspace.ok("state_describe", &[]);
    assert_eq!(tools["tools"].as_array().unwrap().len(), 30);
}

#[test]
fn raw_json_file_and_stdin_match_service_results_and_cli_failures_roll_back() {
    let workspace = Workspace::new();
    workspace.ok(
        "state_namespace",
        &["--json", r#"{"action":"create","name":"raw"}"#],
    );
    let input = workspace.0.join("request.json");
    fs::write(
        &input,
        r#"{"action":"write","namespace":"raw","path":"/binary","base64":"AP8="}"#,
    )
    .unwrap();
    let input_arg = format!("@{}", input.display());
    workspace.ok("state_fs", &["--json", &input_arg]);
    assert_eq!(
        workspace.ok(
            "state_fs",
            &["read", "--namespace", "raw", "--path", "/binary"]
        )["base64"],
        "AP8="
    );
    let result = workspace.value(workspace.run(
        "state_execute",
        &["--json", "-"],
        Some(r#"{"script":"inputs + 1","inputs":41}"#),
    ));
    assert_eq!(result["value"], 42);
    let result = workspace.value(workspace.run(
        "state_execute",
        &["-", "--namespace", "raw"],
        Some("write_text('/stdin', 'saved')\nread_text('/stdin')"),
    ));
    assert_eq!(result["value"], "saved");
    let failure = workspace.fails(
        "state_execute",
        &["write_text('/rollback', 'no')\n1 / 0", "--namespace", "raw"],
    );
    assert!(failure["error"]["code"].is_string());
    let missing = workspace.fails(
        "state_fs",
        &["read", "--namespace", "raw", "--path", "/rollback"],
    );
    assert_eq!(missing["error"]["code"], "NOT_FOUND");
    let malformed = workspace.run("state_namespace", &["create", "--json", "{bad"], None);
    assert!(!malformed.status.success());
    assert!(malformed.stdout.is_empty());
    let ambiguous = workspace.run(
        "state_namespace",
        &["create", "--name", "x", "--json", r#"{"name":"y"}"#],
        None,
    );
    assert!(!ambiguous.status.success());
    let names = workspace.ok("state_namespace", &["list"]);
    assert_eq!(names["namespaces"].as_array().unwrap().len(), 1);
    let invalid = workspace.fails("state_db", &["--json", r#"{"action":"query","namespace":"raw","database":"app","sql":"SELECT 1","extra":true}"#]);
    assert_eq!(invalid["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn cli_groups_cover_mcp_tools_and_paths_are_required() {
    let workspace = Workspace::new();
    let descriptions = workspace.ok("state_describe", &[]);
    let groups: std::collections::BTreeSet<_> = descriptions["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            format!(
                "state_{}",
                tool["name"].as_str().unwrap().split('.').next().unwrap()
            )
        })
        .collect();
    assert_eq!(groups.len(), 7);
    for name in &groups {
        let help = Command::new(env!("CARGO_BIN_EXE_statemcp"))
            .args(["cli", name, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success());
        let missing_path = Command::new(env!("CARGO_BIN_EXE_statemcp"))
            .args(["cli", name])
            .output()
            .unwrap();
        assert!(!missing_path.status.success());
    }
    let unknown = Command::new(env!("CARGO_BIN_EXE_statemcp"))
        .args(["cli", "unknown"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
}

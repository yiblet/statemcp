use super::{Cli, Command};
use clap::Parser;
use serde_json::{Value, json};

struct Workspace(state_core::Controller);
impl Workspace {
    fn new() -> Self {
        let backing = state_store::Store::builder()
            .with_fs(state_fs::MemoryFs::new())
            .open("/state")
            .unwrap();
        Self(state_core::Controller::with_backend(
            state_domain::Store::from_storage(backing).unwrap(),
            std::sync::Arc::new(state_core::EmbeddedBackend),
        ))
    }
    fn run(&self, tool: &str, arguments: &[&str]) -> anyhow::Result<String> {
        let cli = Cli::try_parse_from(
            ["statemcp", "cli", tool, "/state"]
                .into_iter()
                .chain(arguments.iter().copied()),
        )?;
        let Command::Cli { tool } = cli.command else {
            anyhow::bail!("expected CLI command")
        };
        tool.prepare()?.run(&self.0).map_err(Into::into)
    }
    fn ok(&self, tool: &str, arguments: &[&str]) -> Value {
        serde_json::from_str(&self.run(tool, arguments).unwrap()).unwrap()
    }
    fn fails(&self, tool: &str, arguments: &[&str]) -> Value {
        let error = self.run(tool, arguments).unwrap_err();
        let error = error.downcast_ref::<state_core::Error>().unwrap();
        json!({"error": error})
    }
}

#[test]
fn direct_cli_tools_publish_invoke_and_manipulate_state() {
    let workspace = Workspace::new();
    workspace.ok("namespace.create", &["--name", "notes"]);
    workspace.ok("db.create", &["--namespace", "notes", "--database", "app"]);
    workspace.ok("db.migrate", &[ "--namespace", "notes", "--database", "app", "--migrations", r#"[{"id":"initial","sql":"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT NOT NULL)"}]"#]);
    let inspect = workspace.ok("db.inspect", &["--namespace", "notes", "--database", "app"]);
    assert_eq!(inspect["tables"][0]["name"], "notes");
    let python = "def add(text):\n    return db_execute('app', 'INSERT INTO notes(text) VALUES (?) RETURNING id, text', [text])['rows'][0]";
    workspace.ok(
        "fs.write",
        &[
            "--namespace",
            "notes",
            "--path",
            "/api.py",
            "--text",
            python,
        ],
    );
    let schema_arg = r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}"#;
    workspace.ok(
        "function.declare",
        &[
            "--namespace",
            "notes",
            "--name",
            "add",
            "--file",
            "/api.py",
            "--symbol",
            "add",
            "--input-schema",
            schema_arg,
        ],
    );
    let described = workspace.ok("describe", &["--namespace", "notes", "--function", "add"]);
    assert_eq!(described["functions"][0]["name"], "add");
    assert_eq!(
        workspace.ok(
            "call",
            &["notes", "add", "--arguments", r#"{"text":"first"}"#]
        ),
        json!([1, "first"])
    );
    let script_arg = "call('notes', 'add', {'text': inputs})\nwrite_text('/composed', inputs)\ndb_query('app', 'SELECT text FROM notes ORDER BY id')";
    let result = workspace.ok(
        "execute",
        &[
            script_arg,
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
            "execute",
            &[
                script_arg,
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
        "namespace.copy",
        &["--namespace", "notes", "--name", "fork"],
    );
    workspace.ok(
        "namespace.update",
        &["--namespace", "fork", "--name", "renamed"],
    );
    assert_eq!(
        workspace.ok(
            "call",
            &["renamed", "add", "--arguments", r#"{"text":"fork"}"#]
        ),
        json!([3, "fork"])
    );
    let count = workspace.ok(
        "db.query",
        &[
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
        "fs.copy",
        &[
            "--namespace",
            "notes",
            "--path",
            "/composed",
            "--destination",
            "/copy",
        ],
    );
    workspace.ok(
        "fs.move",
        &[
            "--namespace",
            "notes",
            "--path",
            "/copy",
            "--destination",
            "/moved",
        ],
    );
    workspace.ok(
        "fs.append",
        &["--namespace", "notes", "--path", "/moved", "--text", "!"],
    );
    assert_eq!(
        workspace.ok("fs.read", &["--namespace", "notes", "--path", "/moved"])["text"],
        "second!"
    );
    workspace.ok("fs.delete", &["--namespace", "notes", "--path", "/moved"]);
    workspace.ok(
        "function.remove",
        &["--namespace", "renamed", "--name", "add"],
    );
    workspace.ok("db.drop", &["--namespace", "renamed", "--database", "app"]);
    workspace.ok("namespace.delete", &["--namespace", "renamed"]);
    let tools = workspace.ok("describe", &[]);
    assert_eq!(tools["tools"].as_array().unwrap().len(), 34);
}

#[test]
fn raw_json_matches_service_results_and_cli_failures_roll_back() {
    let workspace = Workspace::new();
    workspace.ok("namespace.create", &["--json", r#"{"name":"raw"}"#]);
    workspace.ok(
        "fs.write",
        &[
            "--json",
            r#"{"namespace":"raw","path":"/binary","base64":"AP8="}"#,
        ],
    );
    assert_eq!(
        workspace.ok("fs.read", &["--namespace", "raw", "--path", "/binary"])["base64"],
        "AP8="
    );
    let result = workspace.ok(
        "execute",
        &["--json", r#"{"script":"inputs + 1","inputs":41}"#],
    );
    assert_eq!(result["value"], 42);
    let result = workspace.ok(
        "execute",
        &[
            "write_text('/inline', 'saved')\nread_text('/inline')",
            "--namespace",
            "raw",
        ],
    );
    assert_eq!(result["value"], "saved");
    let failure = workspace.fails(
        "execute",
        &["write_text('/rollback', 'no')\n1 / 0", "--namespace", "raw"],
    );
    assert!(failure["error"]["code"].is_string());
    let missing = workspace.fails("fs.read", &["--namespace", "raw", "--path", "/rollback"]);
    assert_eq!(missing["error"]["code"], "NOT_FOUND");
    assert!(
        workspace
            .run("namespace.create", &["--json", "{bad"])
            .is_err()
    );
    assert!(
        workspace
            .run(
                "namespace.create",
                &["--name", "x", "--json", r#"{"name":"y"}"#]
            )
            .is_err()
    );
    let names = workspace.ok("namespace.list", &[]);
    assert_eq!(names["namespaces"].as_array().unwrap().len(), 1);
    let invalid = workspace.fails(
        "db.query",
        &[
            "--json",
            r#"{"namespace":"raw","database":"app","sql":"SELECT 1","extra":true}"#,
        ],
    );
    assert_eq!(invalid["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn cli_commands_cover_mcp_tools_and_paths_are_required() {
    let workspace = Workspace::new();
    let descriptions = workspace.ok("describe", &[]);
    let groups: std::collections::BTreeSet<_> = descriptions["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(groups.len(), 34);
    for name in &groups {
        let help = Cli::try_parse_from(["statemcp", "cli", name, "--help"])
            .err()
            .unwrap();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(Cli::try_parse_from(["statemcp", "cli", name]).is_err());
    }
    assert!(Cli::try_parse_from(["statemcp", "cli", "unknown"]).is_err());
}

#[test]
fn help_version_and_argument_errors() {
    for argument in ["--help", "--version"] {
        let error = Cli::try_parse_from(["statemcp", argument]).err().unwrap();
        assert!(matches!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
        ));
        assert!(error.to_string().contains("statemcp"));
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
        assert!(Cli::try_parse_from(std::iter::once("statemcp").chain(arguments)).is_err());
    }
}

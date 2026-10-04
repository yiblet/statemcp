//! Bounded CLI input loading and unambiguous merging of JSON and named flags.
use super::Operation;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map, Value, json};
use state_core::NamespaceAction;
use statemcp::{Request, Tool, protocol::MAX_FRAME_BYTES};
use std::{
    fs::File,
    io::{self, Read},
};

pub fn source(input: &str) -> Result<String> {
    if input == "-" {
        read(io::stdin().lock())
    } else if let Some(path) = input.strip_prefix('@') {
        read(File::open(path).with_context(|| format!("cannot read {path}"))?)
    } else if input.len() > MAX_FRAME_BYTES {
        bail!("input exceeds 8 MiB")
    } else {
        Ok(input.to_owned())
    }
}

fn read(reader: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("cannot read input")?;
    if bytes.len() > MAX_FRAME_BYTES {
        bail!("input exceeds 8 MiB");
    }
    String::from_utf8(bytes).context("input is not UTF-8")
}

pub fn json_value(input: &str) -> Result<Value> {
    serde_json::from_str(&source(input)?).context("invalid JSON")
}

fn merge(input: Option<&str>, fields: &impl Serialize) -> Result<Map<String, Value>> {
    let mut map = match input {
        Some(input) => {
            let Value::Object(map) = json_value(input)? else {
                bail!("--json must contain an object");
            };
            map
        }
        None => Map::new(),
    };
    let Value::Object(fields) = serde_json::to_value(fields)? else {
        bail!("CLI fields must serialize to an object");
    };
    for (name, value) in fields {
        if map.contains_key(&name) {
            bail!("field {name} supplied in both --json and named arguments");
        }
        map.insert(name, value);
    }
    Ok(map)
}

pub fn request(tool: Tool, input: Option<&str>, fields: &impl Serialize) -> Result<Request> {
    Request::grouped(tool, Value::Object(merge(input, fields)?)).map_err(Into::into)
}

pub fn operation(args: &Operation, fields: &impl Serialize, tool: Tool) -> Result<Request> {
    let mut map = merge(args.json.as_deref(), fields)?;
    if let Some(action) = &args.action
        && map.insert("action".into(), json!(action)).is_some()
    {
        bail!("action supplied both positionally and inside --json");
    }
    if !map.contains_key("action") {
        bail!("supply an action positionally or inside --json");
    }
    if tool == Tool::Namespace
        && let Some(wire) = map["action"].as_str()
        && let Ok(action) = wire.parse::<NamespaceAction>()
    {
        map.insert("action".into(), json!(action));
    }
    Request::grouped(tool, Value::Object(map)).map_err(Into::into)
}

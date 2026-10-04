//! Bounded CLI input loading and unambiguous merging of JSON and named flags.
use super::Operation;
use serde::Serialize;
use serde_json::{Value, json};
use statemcp::protocol::MAX_FRAME_BYTES;
use std::{
    fs::File,
    io::{self, Read},
};

pub fn source(input: &str) -> Result<String, String> {
    if input == "-" {
        read(io::stdin().lock())
    } else if let Some(path) = input.strip_prefix('@') {
        read(File::open(path).map_err(|e| format!("cannot read {path}: {e}"))?)
    } else if input.len() > MAX_FRAME_BYTES {
        Err("input exceeds 8 MiB".into())
    } else {
        Ok(input.to_owned())
    }
}
fn read(reader: impl Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("input exceeds 8 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "input is not UTF-8".into())
}
pub fn json_value(input: &str) -> Result<Value, String> {
    serde_json::from_str(&source(input)?).map_err(|e| format!("invalid JSON: {e}"))
}
pub fn merge(input: Option<&str>, fields: &impl Serialize) -> Result<Value, String> {
    let mut value = match input {
        Some(input) => json_value(input)?,
        None => json!({}),
    };
    let map = value
        .as_object_mut()
        .ok_or("--json must contain an object")?;
    let fields = json!(fields);
    for (name, value) in fields.as_object().expect("CLI fields are an object") {
        if map.insert(name.clone(), value.clone()).is_some() {
            return Err(format!(
                "field {name} supplied in both --json and named arguments"
            ));
        }
    }
    Ok(value)
}

pub fn operation(args: &Operation, fields: &impl Serialize, rename: bool) -> Result<Value, String> {
    let mut value = merge(args.json.as_deref(), fields)?;
    let map = value.as_object_mut().expect("merged object");
    if let Some(action) = &args.action
        && map.insert("action".into(), json!(action)).is_some()
    {
        return Err("action supplied both positionally and inside --json".into());
    }
    if !map.contains_key("action") {
        return Err("supply an action positionally or inside --json".into());
    }
    if rename && map["action"] == "rename" {
        map.insert("action".into(), json!("update"));
    }
    Ok(value)
}

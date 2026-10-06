//! Bounded CLI input loading and unambiguous merging of JSON and named flags.
use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use statemcp::protocol::MAX_FRAME_BYTES;
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

pub fn parse_json<T: DeserializeOwned>(input: &str) -> Result<T> {
    serde_json::from_str(&source(input)?).context("invalid JSON")
}
fn merge(input: Option<&str>, fields: &impl Serialize) -> Result<Map<String, Value>> {
    let mut map = match input {
        Some(input) => {
            let Value::Object(map) = parse_json(input)? else {
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

/// Merge external JSON and CLI flags at this transport boundary, then retain only the concrete input.
pub fn parse<T: DeserializeOwned>(input: Option<&str>, fields: &impl Serialize) -> Result<T> {
    serde_json::from_value(Value::Object(merge(input, fields)?)).map_err(|e| {
        state_core::Error::new(state_core::ErrorCode::InvalidArgument, e.to_string()).into()
    })
}
pub fn parse_receipted<T: DeserializeOwned>(
    input: Option<&str>,
    fields: &impl Serialize,
    tool: &str,
) -> Result<(T, Option<state_core::ReceiptIdentity>)> {
    let arguments = merge(input, fields)?;
    let receipt = arguments
        .get("idempotency_key")
        .map(|key| {
            state_core::ReceiptIdentity::for_arguments(
                key.as_str().ok_or_else(|| {
                    state_core::Error::new(
                        state_core::ErrorCode::InvalidArgument,
                        "idempotency_key must be a string",
                    )
                })?,
                tool,
                &arguments,
            )
        })
        .transpose()?;
    let input = serde_json::from_value(Value::Object(arguments)).map_err(|e| {
        state_core::Error::new(state_core::ErrorCode::InvalidArgument, e.to_string())
    })?;
    Ok((input, receipt))
}

pub fn discovery_mode(value: &str) -> Result<state_core::inputs::DiscoveryMode> {
    use state_core::inputs::DiscoveryMode;
    match value {
        "overview" => Ok(DiscoveryMode::Overview),
        "full" => Ok(DiscoveryMode::Full),
        "runtime" => Ok(DiscoveryMode::Runtime),
        "readme" => Ok(DiscoveryMode::Readme),
        _ => bail!("unknown discovery mode"),
    }
}

/// One JSON array flag, distinct from Clap's repeated scalar arguments.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct JsonArray<T>(Vec<T>);

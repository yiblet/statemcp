//! Bounded CLI input loading and unambiguous merging of JSON and named flags.
use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use statemcp::protocol::MAX_FRAME_BYTES;
use std::{
    fs::File,
    io::{self, Read},
};

enum InputSource<'a> {
    Inline(&'a str),
    Stdin,
    File(&'a str),
}
impl<'a> InputSource<'a> {
    fn parse(input: &'a str) -> Result<Self> {
        if input == "-" {
            Ok(Self::Stdin)
        } else if let Some(path) = input.strip_prefix('@') {
            Ok(Self::File(path))
        } else if input.len() > MAX_FRAME_BYTES {
            bail!("input exceeds 8 MiB")
        } else {
            Ok(Self::Inline(input))
        }
    }
}

pub fn source(input: &str) -> Result<String> {
    resolve(
        InputSource::parse(input)?,
        || Ok(io::stdin().lock()),
        |path| File::open(path).with_context(|| format!("cannot read {path}")),
    )
}

// Readers stay at the CLI boundary. Parsing and size limits are shared by
// inline, file and stdin sources, and tests supply only memory-backed readers.
fn resolve<S: Read, R: Read>(
    source: InputSource<'_>,
    stdin: impl FnOnce() -> Result<S>,
    open_file: impl FnOnce(&str) -> Result<R>,
) -> Result<String> {
    match source {
        InputSource::Inline(text) => Ok(text.to_owned()),
        InputSource::Stdin => read(stdin()?),
        InputSource::File(path) => read(open_file(path)?),
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

pub fn discovery_mode(value: &str) -> Result<state_contract::discovery::DiscoveryMode> {
    use state_contract::discovery::DiscoveryMode;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_file_and_stdin_resolve_to_the_same_text() -> Result<()> {
        let text = "inputs + 1";
        for input in [text, "-", "@script.py"] {
            let source = InputSource::parse(input)?;
            let loaded = resolve(
                source,
                || Ok(text.as_bytes()),
                |path| {
                    assert_eq!(path, "script.py");
                    Ok(io::Cursor::new(text.as_bytes()))
                },
            )?;
            assert_eq!(loaded, text);
        }
        Ok(())
    }
    #[test]
    fn all_sources_reject_oversized_input_and_readers_reject_invalid_utf8() -> Result<()> {
        let bytes = vec![b'x'; MAX_FRAME_BYTES + 1];
        assert!(InputSource::parse(std::str::from_utf8(&bytes)?).is_err());
        assert!(
            resolve(
                InputSource::Stdin,
                || Ok(bytes.as_slice()),
                |_| Ok(io::empty())
            )
            .is_err()
        );
        assert!(
            resolve(
                InputSource::File("large"),
                || Ok(io::empty()),
                |_| Ok(bytes.as_slice())
            )
            .is_err()
        );
        assert!(read(&[0xff][..]).is_err());
        Ok(())
    }
    #[test]
    fn loading_errors_are_preserved() {
        let result = resolve(
            InputSource::File("missing"),
            || Ok(io::empty()),
            |_| -> Result<io::Empty> { Err(io::Error::from(io::ErrorKind::NotFound).into()) },
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .and_then(|error| error.downcast_ref::<io::Error>())
                .map(io::Error::kind),
            Some(io::ErrorKind::NotFound)
        );
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::PermissionDenied.into())
            }
        }
        assert!(read(FailedReader).is_err());
    }
}

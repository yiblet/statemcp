//! Shared JSON argument and virtual path validation.
use crate::{Error, Result};
use serde_json::Value;

pub(crate) fn required<'a>(args: &'a Value, field: &str) -> Result<&'a str> {
    args.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("{field} must be a string")))
}
pub(crate) fn name(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(Error::invalid(
            "name must contain 1–128 bytes without slash or control characters",
        ));
    }
    Ok(value.to_string())
}
pub(crate) fn virtual_path(value: &str) -> Result<String> {
    if !value.starts_with('/')
        || value.len() > 4096
        || value.chars().any(|c| c == '\0' || c == '\\')
    {
        return Err(Error::invalid(
            "path must be an absolute virtual POSIX path",
        ));
    }
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(Error::invalid("parent traversal is forbidden")),
            _ => parts.push(part),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

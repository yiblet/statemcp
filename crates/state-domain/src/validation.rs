//! Domain name parsing and path normalization.
use crate::{Error, Result};
use state_contract::VirtualPath;
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
    Ok(VirtualPath::parse(value)?.as_str().to_owned())
}

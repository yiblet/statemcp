//! Bounded buffered form parsing; uploads remain bytes and never touch host files.
use crate::{ErrorCode, RuntimeError};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, RuntimeError>;
fn bad(message: &str) -> RuntimeError {
    RuntimeError::new(ErrorCode::HttpBadRequest, message)
}
/// One owned upload, including its field name and uninterpreted bytes.
pub(super) struct Upload {
    pub name: String,
    pub filename: String,
    pub content_type: String,
    pub body: Vec<u8>,
}
/// Parsed form fields and uploads, bounded by the already collected request body.
#[derive(Default)]
pub(super) struct Form {
    pub fields: Vec<(String, String)>,
    pub files: Vec<Upload>,
}
fn parameters(value: &str) -> Result<(String, BTreeMap<String, String>)> {
    let mut chars = value.chars().peekable();
    let mut main = String::new();
    for c in chars.by_ref() {
        if c == ';' {
            break;
        }
        main.push(c);
    }
    let mut params = BTreeMap::new();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut name = String::new();
        let mut found = false;
        for c in chars.by_ref() {
            if c == '=' {
                found = true;
                break;
            }
            if c == ';' {
                return Err(bad("malformed media parameter"));
            }
            name.push(c);
        }
        if !found {
            return Err(bad("malformed media parameter"));
        }
        let mut result = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => result.push(
                        chars
                            .next()
                            .ok_or_else(|| bad("malformed quoted parameter"))?,
                    ),
                    _ => result.push(c),
                }
            }
            if !closed {
                return Err(bad("unclosed quoted parameter"));
            }
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            if chars.next().is_some_and(|c| c != ';') {
                return Err(bad("malformed quoted parameter suffix"));
            }
        } else {
            for c in chars.by_ref() {
                if c == ';' {
                    break;
                }
                result.push(c);
            }
            result = result.trim().into();
        }
        if name.trim().is_empty()
            || params
                .insert(name.trim().to_ascii_lowercase(), result)
                .is_some()
        {
            return Err(bad("duplicate or empty parameter"));
        }
    }
    Ok((main.trim().to_ascii_lowercase(), params))
}
/// Decode supported form media types with at most 256 total fields/uploads.
pub(super) fn parse(content_type: &str, body: &[u8]) -> Result<Form> {
    let (media, params) = parameters(content_type)?;
    if media == "application/x-www-form-urlencoded" {
        let fields = url::form_urlencoded::parse(body)
            .into_owned()
            .take(257)
            .collect::<Vec<_>>();
        if fields.len() > 256 {
            return Err(bad("too many form fields"));
        }
        return Ok(Form {
            fields,
            files: Vec::new(),
        });
    }
    if media != "multipart/form-data" {
        return Ok(Form::default());
    }
    let boundary = params
        .get("boundary")
        .ok_or_else(|| bad("multipart boundary missing"))?;
    if boundary.is_empty()
        || boundary.len() > 70
        || boundary.bytes().any(|b| !(32..127).contains(&b))
    {
        return Err(bad("invalid multipart boundary"));
    }
    let first = format!("--{boundary}").into_bytes();
    let marker = format!("\r\n--{boundary}").into_bytes();
    let mut rest = body
        .strip_prefix(first.as_slice())
        .ok_or_else(|| bad("invalid multipart start"))?;
    if rest.starts_with(b"--") {
        return Ok(Form::default());
    }
    rest = rest
        .strip_prefix(b"\r\n")
        .ok_or_else(|| bad("invalid multipart boundary line"))?;
    let mut form = Form::default();
    loop {
        // SAFETY: marker includes four literal prefix bytes, so windows never receives zero.
        let position = rest
            .windows(marker.len())
            .enumerate()
            .find_map(|(i, bytes)| {
                let suffix = rest.get(i + marker.len()..)?;
                (bytes == marker && (suffix.starts_with(b"\r\n") || suffix.starts_with(b"--")))
                    .then_some(i)
            })
            .ok_or_else(|| bad("multipart end missing"))?;
        let part = rest
            .get(..position)
            .ok_or_else(|| bad("invalid multipart part"))?;
        let split = part
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| bad("multipart headers missing"))?;
        if split > 8192 {
            return Err(bad("multipart part headers too large"));
        }
        let header = std::str::from_utf8(
            part.get(..split)
                .ok_or_else(|| bad("invalid multipart headers"))?,
        )
        .map_err(|_| bad("multipart headers are not UTF-8"))?;
        let data = part
            .get(split + 4..)
            .ok_or_else(|| bad("invalid multipart payload"))?;
        let mut disposition = None;
        let mut content_type = None;
        for line in header.split("\r\n") {
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| bad("invalid multipart header"))?;
            if name.eq_ignore_ascii_case("content-disposition")
                && disposition.replace(value.trim()).is_some()
            {
                return Err(bad("duplicate content-disposition"));
            }
            if name.eq_ignore_ascii_case("content-type")
                && content_type.replace(value.trim()).is_some()
            {
                return Err(bad("duplicate content-type"));
            }
        }
        let (kind, params) =
            parameters(disposition.ok_or_else(|| bad("multipart disposition missing"))?)?;
        if kind != "form-data" {
            return Err(bad("multipart part is not form-data"));
        }
        let name = params
            .get("name")
            .ok_or_else(|| bad("multipart name missing"))?;
        if let Some(filename) = params.get("filename") {
            form.files.push(Upload {
                name: name.clone(),
                filename: filename.clone(),
                content_type: content_type.unwrap_or("application/octet-stream").into(),
                body: data.to_vec(),
            });
        } else {
            form.fields.push((
                name.clone(),
                String::from_utf8(data.to_vec())
                    .map_err(|_| bad("multipart field is not UTF-8"))?,
            ));
        }
        if form.fields.len() + form.files.len() > 256 {
            return Err(bad("too many multipart fields"));
        }
        rest = rest
            .get(position + marker.len()..)
            .ok_or_else(|| bad("invalid multipart offset"))?;
        if rest.starts_with(b"--") {
            break;
        }
        rest = rest
            .strip_prefix(b"\r\n")
            .ok_or_else(|| bad("invalid multipart separator"))?;
    }
    Ok(form)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_fields_quoted_parameters_and_binary_upload() -> Result<()> {
        let body=b"--x\r\nContent-Disposition: form-data; name=\"tag\"\r\n\r\na\r\n--x\r\nContent-Disposition: form-data; name=\"tag\"\r\n\r\nb\r\n--x\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"a;b.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n\0\xff\r\n--x--\r\n";
        let form = parse("multipart/form-data; boundary=\"x\"", body)?;
        assert_eq!(
            form.fields,
            [("tag".into(), "a".into()), ("tag".into(), "b".into())]
        );
        assert_eq!(
            form.files.first().map(|f| f.body.as_slice()),
            Some([0, 255].as_slice())
        );
        assert_eq!(
            form.files.first().map(|f| f.filename.as_str()),
            Some("a;b.bin")
        );
        assert!(parse("multipart/form-data; boundary=x", b"bad").is_err());
        Ok(())
    }
}

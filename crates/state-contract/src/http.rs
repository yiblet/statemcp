//! Namespace HTTP registration and bounded transport/runtime contracts.
use crate::{ParseError, VirtualPath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, ParseError>;
fn invalid(message: impl Into<String>) -> ParseError {
    ParseError::invalid(message)
}

/// An RFC HTTP method token, including extension methods.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct Method(String);
impl Method {
    /// Parse a case-sensitive method token.
    pub fn parse(value: &str) -> Result<Self> {
        ::http::Method::from_bytes(value.as_bytes()).map_err(|_| invalid("invalid HTTP method"))?;
        Ok(Self(value.into()))
    }
    /// Wire token.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Method {
    type Error = ParseError;
    fn try_from(v: String) -> Result<Self> {
        Self::parse(&v)
    }
}
impl From<Method> for String {
    fn from(v: Method) -> Self {
        v.0
    }
}

/// Parsed namespace-relative route with unique named captures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct RoutePattern {
    wire: String,
    segments: Vec<Segment>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Capture(String),
    Tail(String),
}
/// Route ordering at each segment; an exact endpoint outranks an empty catch-all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutePriority {
    CatchAll,
    Capture,
    Literal,
    End,
}
impl RoutePattern {
    /// Parse literals, single `{name}` captures and a final `{*name}` capture.
    pub fn parse(value: &str) -> Result<Self> {
        if !value.starts_with('/')
            || value.len() > 4096
            || value.contains(['\\', '\0', '%', '?', '#'])
            || value.contains("//")
            || value.chars().any(char::is_control)
        {
            return Err(invalid("invalid route path"));
        }
        let mut segments = Vec::new();
        let mut names = BTreeSet::new();
        let parts: Vec<_> = value.trim_start_matches('/').split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() && (value == "/" || index + 1 == parts.len()) {
                continue;
            }
            if part.is_empty() || matches!(*part, "." | "..") {
                return Err(invalid("invalid route segment"));
            }
            let segment =
                if let Some(capture) = part.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
                    let (tail, name) = capture
                        .strip_prefix('*')
                        .map_or((false, capture), |n| (true, n));
                    let mut chars = name.chars();
                    if !matches!(chars.next(),Some(c) if c.is_ascii_alphabetic() || c=='_')
                        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                        || !names.insert(name.to_owned())
                    {
                        return Err(invalid("invalid or duplicate capture name"));
                    }
                    if tail && index + 1 != parts.len() {
                        return Err(invalid("catch-all must be final"));
                    }
                    if tail {
                        Segment::Tail(name.into())
                    } else {
                        Segment::Capture(name.into())
                    }
                } else {
                    if part.contains(['{', '}']) {
                        return Err(invalid("unbalanced route capture"));
                    }
                    Segment::Literal((*part).into())
                };
            segments.push(segment);
        }
        Ok(Self {
            wire: value.into(),
            segments,
        })
    }
    /// Registration pattern as provided.
    pub fn as_str(&self) -> &str {
        &self.wire
    }
    /// Match an already decoded external namespace-relative path.
    pub fn captures(&self, path: &str) -> Option<BTreeMap<String, String>> {
        if self.wire.ends_with('/') != path.ends_with('/')
            && !matches!(self.segments.last(), Some(Segment::Tail(_)))
        {
            return None;
        }
        let parts: Vec<_> = path
            .trim_matches('/')
            .split('/')
            .filter(|p| !p.is_empty())
            .collect();
        let mut result = BTreeMap::new();
        let mut index = 0;
        for segment in &self.segments {
            match segment {
                Segment::Tail(name) => {
                    result.insert(name.clone(), parts.get(index..)?.join("/"));
                    return Some(result);
                }
                Segment::Literal(value) => {
                    if parts.get(index).copied() != Some(value.as_str()) {
                        return None;
                    }
                }
                Segment::Capture(name) => {
                    result.insert(name.clone(), (*parts.get(index)?).into());
                }
            };
            index += 1;
        }
        (index == parts.len()).then_some(result)
    }
    /// Lexicographic specificity: literal, then capture, then catch-all.
    pub fn specificity(&self) -> Vec<RoutePriority> {
        self.segments
            .iter()
            .map(|segment| match segment {
                Segment::Literal(_) => RoutePriority::Literal,
                Segment::Capture(_) => RoutePriority::Capture,
                Segment::Tail(_) => RoutePriority::CatchAll,
            })
            .chain([RoutePriority::End])
            .collect()
    }
    /// True if matching paths can have equal precedence; reject common-method registration.
    pub fn ambiguous(&self, other: &Self) -> bool {
        if self.specificity() != other.specificity()
            || self.wire.ends_with('/') != other.wire.ends_with('/')
        {
            return false;
        }
        self.segments
            .iter()
            .zip(&other.segments)
            .all(|(a, b)| match (a, b) {
                (Segment::Literal(a), Segment::Literal(b)) => a == b,
                _ => true,
            })
    }
    /// Tail capture for static containment.
    pub fn tail(&self) -> Option<&str> {
        match self.segments.last() {
            Some(Segment::Tail(name)) => Some(name),
            _ => None,
        }
    }
}
impl TryFrom<String> for RoutePattern {
    type Error = ParseError;
    fn try_from(v: String) -> Result<Self> {
        Self::parse(&v)
    }
}
impl From<RoutePattern> for String {
    fn from(v: RoutePattern) -> Self {
        v.wire
    }
}

/// Decode URL path once, rejecting ambiguous separators and traversal.
pub fn decode_path(path: &str) -> Result<String> {
    if !path.starts_with('/') || path.len() > 12288 || path.contains(['\\', '\0']) {
        return Err(invalid("invalid URL path"));
    }
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hex = |b: u8| char::from(b).to_digit(16).map(|v| v as u8);
            let a = bytes
                .next()
                .and_then(hex)
                .ok_or_else(|| invalid("malformed percent encoding"))?;
            let b = bytes
                .next()
                .and_then(hex)
                .ok_or_else(|| invalid("malformed percent encoding"))?;
            let byte = a * 16 + b;
            if matches!(byte, b'/' | b'\\' | 0) {
                return Err(invalid("encoded separator forbidden"));
            }
            decoded.push(byte);
        } else {
            decoded.push(byte);
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| invalid("URL path is not UTF-8"))?;
    if decoded.len() > 4096
        || decoded.chars().any(char::is_control)
        || decoded.contains("//")
        || decoded.split('/').any(|p| matches!(p, "." | ".."))
    {
        return Err(invalid("ambiguous URL path"));
    }
    Ok(decoded)
}

/// Publication target; static mounts expose only their directory descendants.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetInput {
    Function {
        file: String,
        symbol: String,
    },
    Static {
        directory: String,
        #[serde(default)]
        index: Option<String>,
    },
}
/// Publish or replace a named route. Empty/omitted methods means all function methods.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiPublishInput {
    pub namespace: String,
    pub name: String,
    pub path: RoutePattern,
    #[serde(default)]
    pub methods: BTreeSet<Method>,
    pub target: TargetInput,
    #[serde(default)]
    pub expected_revision: Option<String>,
}
/// List routes in a namespace.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiListInput {
    pub namespace: String,
}
/// Get a named route.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiGetInput {
    pub namespace: String,
    pub name: String,
}
/// Remove a named route with an optional namespace revision guard.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiRemoveInput {
    pub namespace: String,
    pub name: String,
    #[serde(default)]
    pub expected_revision: Option<String>,
}
/// HTTP handler source pinned at publication, independently of JSON function ABI.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handler {
    pub file: String,
    pub symbol: String,
    pub sources: crate::function::FunctionSources,
}
/// Persisted route target with pinned source or live static mount.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteTarget {
    Function(Handler),
    Static {
        directory: String,
        index: Option<String>,
    },
}
/// Durable registration stored alongside namespace files/functions.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub name: String,
    pub path: RoutePattern,
    pub methods: BTreeSet<Method>,
    pub target: RouteTarget,
}
impl Route {
    /// Method selection includes implicit HEAD for GET.
    pub fn accepts(&self, method: &Method) -> bool {
        self.methods.is_empty()
            || self.methods.contains(method)
            || (method.as_str() == "HEAD" && self.methods.iter().any(|m| m.as_str() == "GET"))
    }
    /// Metadata without pinned source code.
    pub fn metadata(&self, namespace: &str, namespace_id: &str) -> ApiPublished {
        ApiPublished {
            namespace: namespace.into(),
            namespace_id: namespace_id.into(),
            name: self.name.clone(),
            path: self.path.clone(),
            methods: self.methods.clone(),
            target: match &self.target {
                RouteTarget::Function(h) => TargetInput::Function {
                    file: h.file.clone(),
                    symbol: h.symbol.clone(),
                },
                RouteTarget::Static { directory, index } => TargetInput::Static {
                    directory: directory.clone(),
                    index: index.clone(),
                },
            },
            urls: Vec::new(),
        }
    }
}
/// Publication/discovery result; HTTP MCP fills URLs from validated hosting information.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiPublished {
    pub namespace: String,
    pub namespace_id: String,
    pub name: String,
    pub path: RoutePattern,
    pub methods: BTreeSet<Method>,
    pub target: TargetInput,
    pub urls: Vec<String>,
}

/// Successful route removal, suitable for committed mutation responses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiRemoved {
    pub name: String,
    pub removed: bool,
}

/// Owned, bounded HTTP request passed directly from transport to runtime.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    pub method: Method,
    pub path: String,
    pub query: String,
    pub scheme: String,
    pub host: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    #[serde(default)]
    pub view_args: BTreeMap<String, String>,
}
/// Fully validated buffered response. Construction prevents invalid wire metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ResponseRecord", into = "ResponseRecord")]
pub struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseRecord {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl HttpResponse {
    /// Validate final status, field values and framing before transaction publication.
    pub fn new(status: u16, mut headers: Vec<(String, String)>, mut body: Vec<u8>) -> Result<Self> {
        if !(200..1000).contains(&status) {
            return Err(invalid("invalid terminal HTTP status"));
        }
        for (name, value) in &mut headers {
            let parsed = ::http::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| invalid("invalid response header name"))?;
            ::http::HeaderValue::from_bytes(value.as_bytes())
                .map_err(|_| invalid("invalid response header value"))?;
            *name = parsed.as_str().into();
            if matches!(
                name.as_str(),
                "connection"
                    | "transfer-encoding"
                    | "upgrade"
                    | "keep-alive"
                    | "trailer"
                    | "proxy-authenticate"
                    | "proxy-authorization"
                    | "te"
            ) {
                return Err(invalid("hop-by-hop response header forbidden"));
            }
            if name == "content-length"
                && status != 204
                && status != 304
                && value.parse::<usize>().ok() != Some(body.len())
            {
                return Err(invalid("inconsistent content-length"));
            }
        }
        let mut lengths = headers.iter().filter(|(n, _)| n == "content-length");
        if lengths.next().is_some() && lengths.next().is_some() {
            return Err(invalid("duplicate content-length"));
        }
        if status == 204 || status == 304 {
            body.clear();
            headers.retain(|(n, _)| n != "content-length");
        }
        Ok(Self {
            status,
            headers,
            body,
        })
    }
    /// Wire status.
    pub fn status(&self) -> u16 {
        self.status
    }
    /// Ordered fields, including repeated Set-Cookie.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }
    /// Representation bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    /// Move response fields into a transport, suppressing a HEAD body only after validation.
    pub fn into_parts(self) -> (u16, Vec<(String, String)>, Vec<u8>) {
        (self.status, self.headers, self.body)
    }
}
impl TryFrom<ResponseRecord> for HttpResponse {
    type Error = ParseError;
    fn try_from(v: ResponseRecord) -> Result<Self> {
        Self::new(v.status, v.headers, v.body)
    }
}
impl From<HttpResponse> for ResponseRecord {
    fn from(v: HttpResponse) -> Self {
        Self {
            status: v.status,
            headers: v.headers,
            body: v.body,
        }
    }
}
/// Resolve a static tail without permitting traversal out of its mount.
pub fn static_path(directory: &str, tail: &str) -> Result<VirtualPath> {
    let root = VirtualPath::parse(directory)?;
    let path = VirtualPath::parse(&format!("{}/{tail}", root.as_str().trim_end_matches('/')))?;
    if !root.contains(&path) {
        return Err(invalid("static mount escape"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn methods_paths_and_routes() -> Result<()> {
        assert_eq!(Method::parse("CUSTOM")?.as_str(), "CUSTOM");
        assert!(Method::parse("BAD METHOD").is_err());
        let a = RoutePattern::parse("/items/{id}")?;
        assert_eq!(
            a.captures("/items/7").and_then(|m| m.get("id").cloned()),
            Some("7".into())
        );
        assert!(a.ambiguous(&RoutePattern::parse("/items/{other}")?));
        assert!(!a.ambiguous(&RoutePattern::parse("/items/new")?));
        assert!(RoutePattern::parse("/{id}/{id}").is_err());
        assert!(RoutePattern::parse("//a").is_err());
        assert!(RoutePattern::parse("/a\n").is_err());
        assert!(
            RoutePattern::parse("/a/")?.specificity()
                > RoutePattern::parse("/a/{*rest}")?.specificity()
        );
        assert!(RoutePattern::parse("/{*all}/suffix").is_err());
        assert_eq!(decode_path("/a/%E2%9C%93")?, "/a/✓");
        for p in ["/a/../x", "/a/%2F", "/a/%00", "/a/%xx", "/a//x"] {
            assert!(decode_path(p).is_err());
        }
        assert!(static_path("/public", "../secret").is_err());
        Ok(())
    }
    #[test]
    fn response_validation() -> Result<()> {
        assert!(HttpResponse::new(99, vec![], vec![]).is_err());
        assert!(HttpResponse::new(200, vec![("x".into(), "\r\nbad".into())], vec![]).is_err());
        assert!(
            HttpResponse::new(200, vec![("content-length".into(), "1".into())], vec![]).is_err()
        );
        let r = HttpResponse::new(
            200,
            vec![
                ("set-cookie".into(), "a=1".into()),
                ("set-cookie".into(), "b=2".into()),
            ],
            vec![0, 255],
        )?;
        assert_eq!(r.headers().len(), 2);
        assert_eq!(r.body(), [0, 255]);
        Ok(())
    }
}

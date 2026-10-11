//! Native Python HTTP objects and Flask buffered response conversion.
use super::*;
use monty_types::{
    MontyUuid,
    unstable::{self, MontyNode},
};
use state_contract::http::{HttpRequest, HttpResponse};

/// Validated response and bounded diagnostics from an HTTP handler.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpRunResult {
    pub response: HttpResponse,
    pub stdout: String,
}

/// Invoke one positional HTTP Request, keeping effects denied during initialization.
pub fn invoke_http(
    source: &str,
    symbol: &str,
    modules: &ModuleSources,
    request: HttpRequest,
    limits: &Limits,
    host: &mut HostCallback<'_>,
) -> Result<HttpRunResult, RuntimeError> {
    let mut session = Session::new(source, limits)?;
    let mut bindings = Bindings::new(request, limits)?;
    let request = bindings.request_object()?;
    let response_class = bindings.response_class();
    session.http = Some(bindings);
    // Response exists during initialization; constructing it is local and effect-free.
    let mut inputs = session.module_inputs(modules)?;
    inputs.push("Response", response_class);
    if !identifier(symbol) || symbol.starts_with("__state_") || HOST_FUNCTIONS.contains(&symbol) {
        return Err(invalid_result("invalid HTTP symbol"));
    }
    let progress = session
        .repl()
        .feed_start(source, inputs, session.print())
        .map_err(repl_error)?;
    let (repl, _) = session.drive(progress, None)?;
    if !repl.has_function(symbol) {
        return Err(invalid_result("HTTP symbol is not callable"));
    }
    let initialization = session.started.elapsed();
    let mut inputs = NamedValues::new();
    inputs.push("__state_http_request", request);
    let progress = repl
        .feed_start(
            &format!("{symbol}(__state_http_request)"),
            inputs,
            session.print(),
        )
        .map_err(repl_error)?;
    let (_, value) = session.drive(progress, Some(host))?;
    session.check_deadline()?;
    let response = normalize(value.as_ref(), limits)?;
    if response
        .body()
        .len()
        .saturating_add(session.stdout.text.len())
        > limits.max_output_bytes
    {
        return Err(limit_error("HTTP output byte limit exceeded"));
    }
    host.profile(session.profile(initialization))?;
    Ok(HttpRunResult {
        response,
        stdout: session.stdout.text,
    })
}

fn id() -> MontyUuid {
    MontyUuid::from_bytes(*uuid::Uuid::new_v4().as_bytes())
}
#[derive(Clone)]
struct HostType(MontyObject);
fn class(name: &str) -> HostType {
    HostType(MontyObject::class_type(name, id(), true, false, []))
}
fn instance(class: HostType, id: MontyUuid, attrs: Vec<(MontyObject, MontyObject)>) -> MontyObject {
    // SAFETY: HostType has only the class() constructor, which always creates a class type.
    MontyObject::class_instance(class.0, id, attrs)
}
fn attr(name: &str, value: MontyObject) -> (MontyObject, MontyObject) {
    (MontyObject::string(name), value)
}
fn error(code: ErrorCode, message: impl Into<String>) -> RuntimeError {
    RuntimeError::new(code, message)
}

struct Multi {
    id: MontyUuid,
    pairs: Vec<(String, MontyObject)>,
    insensitive: bool,
}
pub(super) struct Bindings {
    request: HttpRequest,
    request_id: MontyUuid,
    response_id: MontyUuid,
    response_type: HostType,
    multis: Vec<Multi>,
    limits: Limits,
}
impl Bindings {
    fn new(request: HttpRequest, limits: &Limits) -> Result<Self, RuntimeError> {
        let metadata = request.headers.iter().fold(
            request
                .path
                .len()
                .saturating_add(request.query.len())
                .saturating_add(request.host.len()),
            |size, (k, v)| size.saturating_add(k.len()).saturating_add(v.len()),
        );
        if request.body.len() > limits.max_output_bytes || metadata > 96 * 1024 {
            return Err(limit_error("HTTP request byte limit exceeded"));
        }
        Ok(Self {
            request,
            request_id: id(),
            response_id: id(),
            response_type: class("Response"),
            multis: Vec::new(),
            limits: limits.clone(),
        })
    }
    fn response_class(&mut self) -> MontyObject {
        // Use the actual class identity for constructor dispatch.
        if let MontyNode::ClassType(c) = unstable::root_node(&self.response_type.0) {
            self.response_id = c.id;
        }
        self.response_type.0.clone()
    }
    fn multi(&mut self, pairs: Vec<(String, String)>, insensitive: bool) -> MontyObject {
        self.multi_objects(
            pairs
                .into_iter()
                .map(|(k, v)| (k, MontyObject::string(v)))
                .collect(),
            insensitive,
        )
    }
    fn multi_objects(
        &mut self,
        pairs: Vec<(String, MontyObject)>,
        insensitive: bool,
    ) -> MontyObject {
        let mut mapping = BTreeMap::new();
        for (name, value) in &pairs {
            mapping
                .entry(if insensitive {
                    name.to_ascii_lowercase()
                } else {
                    name.clone()
                })
                .or_insert_with(|| value.clone());
        }
        let object_id = id();
        let mapping = MontyObject::dict(
            mapping
                .into_iter()
                .map(|(k, v)| (MontyObject::string(k), v)),
        );
        self.multis.push(Multi {
            id: object_id,
            pairs,
            insensitive,
        });
        instance(
            class("MultiDict"),
            object_id,
            vec![
                attr("_mapping", mapping),
                attr("_mapping_casefold", MontyObject::bool(insensitive)),
            ],
        )
    }
    fn request_object(&mut self) -> Result<MontyObject, RuntimeError> {
        let headers = self.multi(self.request.headers.clone(), true);
        let args = self.multi(
            url::form_urlencoded::parse(self.request.query.as_bytes())
                .into_owned()
                .collect(),
            false,
        );
        let body = MontyObject::bytes(self.request.body.clone());
        let content_type = self.header("content-type").unwrap_or("").to_owned();
        let mimetype = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let url = self.url(true);
        let base_url = self.url(false);
        let full_path = if self.request.query.is_empty() {
            format!("{}?", self.request.path)
        } else {
            format!("{}?{}", self.request.path, self.request.query)
        };
        let view_args = MontyObject::dict(
            self.request
                .view_args
                .iter()
                .map(|(k, v)| (MontyObject::string(k), MontyObject::string(v))),
        );
        let cookies = self
            .header("cookie")
            .unwrap_or("")
            .split(';')
            .filter_map(|s| s.trim().split_once('='))
            .map(|(k, v)| (MontyObject::string(k), MontyObject::string(v)))
            .collect::<Vec<_>>();
        Ok(instance(
            class("Request"),
            self.request_id,
            vec![
                attr("method", MontyObject::string(self.request.method.as_str())),
                attr("body", body.clone()),
                attr("data", body),
                attr("path", MontyObject::string(&self.request.path)),
                attr("full_path", MontyObject::string(full_path)),
                attr("url", MontyObject::string(url)),
                attr("base_url", MontyObject::string(base_url)),
                attr("scheme", MontyObject::string(&self.request.scheme)),
                attr("host", MontyObject::string(&self.request.host)),
                attr("headers", headers),
                attr("args", args),
                attr("view_args", view_args),
                attr("cookies", MontyObject::dict(cookies)),
                attr("content_type", MontyObject::string(content_type)),
                attr("mimetype", MontyObject::string(mimetype.clone())),
                attr(
                    "content_length",
                    MontyObject::int(self.request.body.len() as i64),
                ),
                attr("is_json", MontyObject::bool(is_json(&mimetype))),
            ],
        ))
    }
    fn url(&self, query: bool) -> String {
        format!(
            "{}://{}{}{}",
            self.request.scheme,
            self.request.host,
            self.request
                .path
                .split('/')
                .map(|part| part
                    .bytes()
                    .map(
                        |byte| if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                            char::from(byte).to_string()
                        } else {
                            format!("%{byte:02X}")
                        }
                    )
                    .collect::<String>())
                .collect::<Vec<_>>()
                .join("/"),
            if query && !self.request.query.is_empty() {
                format!("?{}", self.request.query)
            } else {
                String::new()
            }
        )
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self, force: bool, silent: bool) -> Result<MontyObject, RuntimeError> {
        let media = self
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let value = if !force && !is_json(media) {
            Err(error(
                ErrorCode::HttpUnsupportedMediaType,
                "request JSON requires application/json",
            ))
        } else {
            serde_json::from_slice::<Value>(&self.request.body)
                .map_err(|_| error(ErrorCode::HttpBadRequest, "malformed JSON request"))
        };
        match value {
            Ok(value) => JsonBudget::new(&self.limits).encode(&value, 0),
            Err(_) if silent => Ok(MontyObject::none()),
            Err(e) => Err(e),
        }
    }
    fn text(&self) -> Result<MontyObject, RuntimeError> {
        String::from_utf8(self.request.body.clone())
            .map(MontyObject::string)
            .map_err(|_| error(ErrorCode::HttpBadRequest, "request body is not UTF-8"))
    }
    pub(super) fn lookup(
        &mut self,
        object: MontyUuid,
        name: &str,
    ) -> Result<Option<MontyObject>, RuntimeError> {
        if object != self.request_id {
            return Ok(None);
        }
        match name {
            "json" => self.json(false, false).map(Some),
            "text" => self.text().map(Some),
            "form" | "files" => {
                let form = crate::http_form::parse(
                    self.header("content-type").unwrap_or(""),
                    &self.request.body,
                )?;
                if name == "form" {
                    Ok(Some(self.multi(form.fields, false)))
                } else {
                    let files = form
                        .files
                        .into_iter()
                        .map(|file| {
                            let object = MontyObject::named_tuple(
                                "Upload",
                                ["filename", "content_type", "body"],
                                [
                                    MontyObject::string(file.filename),
                                    MontyObject::string(file.content_type),
                                    MontyObject::bytes(file.body),
                                ],
                            );
                            (file.name, object)
                        })
                        .collect();
                    Ok(Some(self.multi_objects(files, false)))
                }
            }
            _ => Ok(None),
        }
    }
    pub(super) fn call(
        &mut self,
        object: MontyUuid,
        name: &str,
        args: Vec<ObjectRef<'_>>,
        kwargs: Vec<(ObjectRef<'_>, ObjectRef<'_>)>,
    ) -> Result<Option<MontyObject>, RuntimeError> {
        let named = |index: usize, name: &str| -> Result<Option<ObjectRef<'_>>, RuntimeError> {
            let a = args.get(index).copied();
            let k = kwargs
                .iter()
                .find(|(k, _)| k.as_str() == Some(name))
                .map(|(_, v)| *v);
            if a.is_some() && k.is_some() {
                return Err(invalid_result("argument supplied twice"));
            }
            Ok(a.or(k))
        };
        if object == self.request_id {
            return match name {
                "get_json" => {
                    if args.len() > 2
                        || kwargs
                            .iter()
                            .any(|(k, _)| !matches!(k.as_str(), Some("force" | "silent")))
                    {
                        return Err(invalid_result("invalid get_json arguments"));
                    }
                    self.json(
                        named(0, "force")?.is_some_and(|v| v.is_truthy()),
                        named(1, "silent")?.is_some_and(|v| v.is_truthy()),
                    )
                    .map(Some)
                }
                "get_data" => {
                    if args.len() > 1 || kwargs.iter().any(|(k, _)| k.as_str() != Some("as_text")) {
                        return Err(invalid_result("invalid get_data arguments"));
                    }
                    if named(0, "as_text")?.is_some_and(|v| v.is_truthy()) {
                        self.text().map(Some)
                    } else {
                        Ok(Some(MontyObject::bytes(self.request.body.clone())))
                    }
                }
                _ => Ok(None),
            };
        }
        if object == self.response_id {
            if name != "Response" && name != "__call__" {
                return Ok(None);
            }
            if args.len() > 3
                || kwargs.iter().any(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        Some("response" | "status" | "headers" | "mimetype" | "content_type")
                    )
                })
            {
                return Err(invalid_result("invalid Response arguments"));
            }
            let body =
                named(0, "response")?.map_or_else(|| MontyObject::string(""), |v| v.to_owned());
            let status =
                named(1, "status")?.map_or_else(|| MontyObject::int(200), |v| v.to_owned());
            let headers =
                named(2, "headers")?.map_or_else(|| MontyObject::dict([]), |v| v.to_owned());
            let content_type =
                named(usize::MAX, "content_type")?.or(named(usize::MAX, "mimetype")?);
            let mut attrs = vec![
                attr("response", body),
                attr("status", status),
                attr("headers", headers),
            ];
            if let Some(v) = content_type {
                attrs.push(attr("content_type", v.to_owned()));
            }
            return Ok(Some(instance(self.response_type.clone(), id(), attrs)));
        }
        let Some(multi) = self.multis.iter().find(|m| m.id == object) else {
            return Ok(None);
        };
        let key = named(0, "key")?.and_then(|v| v.as_str());
        if matches!(name, "get" | "getlist") && key.is_none() {
            return Err(invalid_result("MultiDict key must be supplied as text"));
        }
        if name == "getlist"
            && (args.len() > 1 || kwargs.iter().any(|(k, _)| k.as_str() != Some("key")))
        {
            return Err(invalid_result("invalid MultiDict.getlist arguments"));
        }
        if name == "items"
            && (args.len() > 1 || kwargs.iter().any(|(k, _)| k.as_str() != Some("multi")))
        {
            return Err(invalid_result("invalid MultiDict.items arguments"));
        }
        if name == "keys" && (!args.is_empty() || !kwargs.is_empty()) {
            return Err(invalid_result("keys takes no arguments"));
        }
        let matches = |k: &str| {
            key.is_some_and(|key| {
                if multi.insensitive {
                    k.eq_ignore_ascii_case(key)
                } else {
                    k == key
                }
            })
        };
        match name {
            "get" => {
                if args.len() > 2
                    || kwargs
                        .iter()
                        .any(|(k, _)| !matches!(k.as_str(), Some("key" | "default")))
                {
                    return Err(invalid_result("invalid MultiDict.get arguments"));
                }
                let default = named(1, "default")?.map_or_else(MontyObject::none, |v| v.to_owned());
                Ok(Some(
                    multi
                        .pairs
                        .iter()
                        .find(|(k, _)| matches(k))
                        .map_or(default, |(_, v)| v.clone()),
                ))
            }
            "getlist" => Ok(Some(MontyObject::list(
                multi
                    .pairs
                    .iter()
                    .filter(|(k, _)| matches(k))
                    .map(|(_, v)| v.clone()),
            ))),
            "items" => {
                let all = named(0, "multi")?.is_some_and(|v| v.is_truthy());
                let mut seen = BTreeMap::new();
                Ok(Some(MontyObject::list(
                    multi
                        .pairs
                        .iter()
                        .filter(|(k, _)| all || seen.insert(k.clone(), ()).is_none())
                        .map(|(k, v)| MontyObject::tuple([MontyObject::string(k), v.clone()])),
                )))
            }
            "keys" => Ok(Some(MontyObject::list(
                multi.pairs.iter().map(|(k, _)| MontyObject::string(k)),
            ))),
            _ => Ok(None),
        }
    }
}
fn is_json(media: &str) -> bool {
    media.eq_ignore_ascii_case("application/json") || media.to_ascii_lowercase().ends_with("+json")
}
fn response_error(message: impl Into<String>) -> RuntimeError {
    error(ErrorCode::PythonError, message)
}
fn field<'a>(value: ObjectRef<'a>, name: &str) -> Option<ObjectRef<'a>> {
    value
        .pairs()?
        .into_iter()
        .find(|(k, _)| k.as_str() == Some(name))
        .map(|(_, v)| v)
}
fn status(value: ObjectRef<'_>) -> Result<u16, RuntimeError> {
    if let Some(text) = value.as_str()
        && (text.chars().any(char::is_control)
            || text
                .get(..3)
                .is_none_or(|n| !n.bytes().all(|b| b.is_ascii_digit()))
            || (text.len() > 3 && !text.get(3..).is_some_and(|v| v.starts_with(' '))))
    {
        return Err(response_error("invalid response status line"));
    }
    let number = if let Some(n) = value.as_int() {
        u16::try_from(n).ok()
    } else if let Some(s) = value.as_str() {
        s.split(' ').next().and_then(|v| v.parse().ok())
    } else {
        None
    };
    number
        .filter(|n| (200..1000).contains(n))
        .ok_or_else(|| response_error("invalid response status"))
}
fn headers(value: ObjectRef<'_>) -> Result<Vec<(String, String)>, RuntimeError> {
    let pairs = if value.type_name() == "dict" {
        value
            .pairs()
            .ok_or_else(|| response_error("invalid headers"))?
    } else {
        value
            .items()
            .ok_or_else(|| response_error("headers must be a mapping or pairs"))?
            .into_iter()
            .map(|pair| {
                let items = pair
                    .items()
                    .ok_or_else(|| response_error("invalid header pair"))?;
                match items.as_slice() {
                    [k, v] => Ok((*k, *v)),
                    _ => Err(response_error("invalid header pair")),
                }
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    pairs
        .into_iter()
        .map(|(k, v)| {
            Ok((
                k.as_str()
                    .ok_or_else(|| response_error("header name must be text"))?
                    .into(),
                v.as_str()
                    .ok_or_else(|| response_error("header value must be text"))?
                    .into(),
            ))
        })
        .collect()
}
fn normalize(value: ObjectRef<'_>, limits: &Limits) -> Result<HttpResponse, RuntimeError> {
    let (body, code, fields, content_type) = if value.type_name() == "Response" {
        (
            field(value, "response").ok_or_else(|| response_error("invalid Response"))?,
            field(value, "status")
                .map(status)
                .transpose()?
                .unwrap_or(200),
            field(value, "headers")
                .map(headers)
                .transpose()?
                .unwrap_or_default(),
            field(value, "content_type").and_then(|v| v.as_str()),
        )
    } else if value.type_name() == "tuple" {
        let items = value
            .items()
            .ok_or_else(|| response_error("invalid response tuple"))?;
        match items.as_slice() {
            [body, s] if s.as_int().is_some() || s.as_str().is_some() => {
                (*body, status(*s)?, vec![], None)
            }
            [body, h] => (*body, 200, headers(*h)?, None),
            [body, s, h] => (*body, status(*s)?, headers(*h)?, None),
            _ => {
                return Err(response_error(
                    "HTTP response tuple needs two or three items",
                ));
            }
        }
    } else {
        (value, 200, vec![], None)
    };
    if body.type_name() == "Response" {
        let r = normalize(body, limits)?;
        let inherit_status=value.type_name()=="tuple" && value.items().is_some_and(|items|matches!(items.as_slice(),[_,headers] if headers.as_int().is_none() && headers.as_str().is_none()));
        let (nested_status, mut nested, body) = r.into_parts();
        for (name, _) in &fields {
            nested.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        }
        nested.extend(fields);
        return HttpResponse::new(
            if inherit_status { nested_status } else { code },
            nested,
            body,
        )
        .map_err(|e| response_error(e.message));
    }
    let (body, default_type) = match unstable::node(body) {
        MontyNode::Bytes(bytes) => (bytes.clone(), "text/html; charset=utf-8"),
        _ if body.type_name() == "str" => (
            body.as_str()
                .ok_or_else(|| response_error("invalid string"))?
                .as_bytes()
                .to_vec(),
            "text/html; charset=utf-8",
        ),
        _ if matches!(body.type_name(), "dict" | "list") => {
            let json = JsonBudget::new(limits).decode(body, 0)?;
            let mut bytes = serde_json::to_vec(&json).map_err(|e| response_error(e.to_string()))?;
            bytes.push(b'\n');
            (bytes, "application/json")
        }
        _ => return Err(response_error("unsupported HTTP response body")),
    };
    let mut fields = fields;
    if !fields
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
    {
        fields.push((
            "content-type".into(),
            content_type.unwrap_or(default_type).into(),
        ));
    }
    let size = body.len() + fields.iter().map(|(n, v)| n.len() + v.len()).sum::<usize>();
    if size > limits.max_output_bytes {
        return Err(limit_error("HTTP response byte limit exceeded"));
    }
    HttpResponse::new(code, fields, body).map_err(|e| response_error(e.message))
}

/// Initialize and validate a handler module with HTTP Response available, denying host effects.
pub fn validate_http(
    source: &str,
    symbol: &str,
    modules: &ModuleSources,
    limits: &Limits,
) -> Result<(), RuntimeError> {
    let request = HttpRequest {
        method: state_contract::http::Method::parse("GET")
            .map_err(|e| invalid_result(e.message))?,
        path: "/".into(),
        query: String::new(),
        scheme: "http".into(),
        host: "localhost".into(),
        headers: Vec::new(),
        body: Vec::new(),
        view_args: BTreeMap::new(),
    };
    let mut session = Session::new(source, limits)?;
    let mut bindings = Bindings::new(request, limits)?;
    let response = bindings.response_class();
    session.http = Some(bindings);
    if !identifier(symbol) || symbol.starts_with("__state_") || HOST_FUNCTIONS.contains(&symbol) {
        return Err(invalid_result("invalid HTTP symbol"));
    }
    let mut inputs = session.module_inputs(modules)?;
    inputs.push("Response", response);
    let progress = session
        .repl()
        .feed_start(source, inputs, session.print())
        .map_err(repl_error)?;
    let (repl, _) = session.drive(progress, None)?;
    if !repl.has_function(symbol) {
        return Err(invalid_result("HTTP symbol is not callable"));
    }
    session.check_deadline()
}

#[cfg(test)]
mod tests {
    use super::*;
    use state_contract::http::Method;
    fn request() -> Result<HttpRequest, RuntimeError> {
        Ok(HttpRequest {
            method: Method::parse("PATCH").map_err(|e| invalid_result(e.message))?,
            path: "/notes/items/7".into(),
            query: "tag=a&tag=b&blank=".into(),
            scheme: "http".into(),
            host: "localhost:8000".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-test".into(), "first".into()),
                ("x-test".into(), "second".into()),
            ],
            body: br#"{"n":7}"#.to_vec(),
            view_args: BTreeMap::from([("id".into(), "7".into())]),
        })
    }
    fn run(body: &str, request: HttpRequest) -> Result<HttpResponse, RuntimeError> {
        let source = format!(
            "def handle(request):\n{}",
            body.lines()
                .map(|l| format!("    {l}\n"))
                .collect::<String>()
        );
        invoke_http(
            &source,
            "handle",
            &ModuleSources::default(),
            request,
            &Limits::default(),
            &mut |_: &str, _: Vec<Value>, _: serde_json::Map<String, Value>| {
                Err(invalid_result("no host configured"))
            },
        )
        .map(|r| r.response)
    }
    #[test]
    fn positional_request_attributes_and_multidicts() -> Result<(), RuntimeError> {
        let r = run(
            "return {'method':request.method,'n':request.json['n'],'tags':request.args.getlist('tag'),'first':request.args['tag'],'id':request.view_args['id'],'h':request.headers.getlist('X-Test'),'text':request.text}",
            request()?,
        )?;
        let json: Value =
            serde_json::from_slice(r.body()).map_err(|e| invalid_result(e.to_string()))?;
        assert_eq!(json["method"], "PATCH");
        assert_eq!(json["n"], 7);
        assert_eq!(json["tags"], serde_json::json!(["a", "b"]));
        assert_eq!(json["h"], serde_json::json!(["first", "second"]));
        assert_eq!(json["first"], "a");
        Ok(())
    }
    #[test]
    fn flask_tuples_lists_bytes_and_response() -> Result<(), RuntimeError> {
        let r = run(
            "return ('created', '201 CREATED', [('Set-Cookie','a=1'),('Set-Cookie','b=2')])",
            request()?,
        )?;
        assert_eq!(r.status(), 201);
        assert_eq!(
            r.headers()
                .iter()
                .filter(|(k, _)| k == "set-cookie")
                .count(),
            2
        );
        let r = run("return ['body', 202]", request()?)?;
        assert_eq!(r.status(), 200);
        assert_eq!(
            serde_json::from_slice::<Value>(r.body()).map_err(|e| invalid_result(e.to_string()))?,
            serde_json::json!(["body", 202])
        );
        let mut req = request()?;
        req.body = vec![0, 255, 10];
        let r = run("return request.get_data(), 202", req)?;
        assert_eq!(r.body(), [0, 255, 10]);
        let r = run(
            "return Response(b'hello', status=203, content_type='application/octet-stream')",
            request()?,
        )?;
        assert_eq!(r.status(), 203);
        assert_eq!(r.body(), b"hello");
        Ok(())
    }
    #[test]
    fn forms_files_case_insensitive_indexing_and_response_status() -> Result<(), RuntimeError> {
        let mut req = request()?;
        req.headers = vec![
            (
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
            ("x-test".into(), "value".into()),
        ];
        req.body = b"tag=a&tag=b".to_vec();
        let response = run(
            "return {'tags':request.form.getlist('tag'),'header':request.headers['X-TeSt'],'count':len(request.args),'keys':list(request.args),'files':bool(request.files)}",
            req.clone(),
        )?;
        let json: Value =
            serde_json::from_slice(response.body()).map_err(|e| invalid_result(e.to_string()))?;
        assert_eq!(json["tags"], serde_json::json!(["a", "b"]));
        assert_eq!(json["header"], "value");
        assert_eq!(json["files"], false);
        req.headers = vec![(
            "content-type".into(),
            "multipart/form-data; boundary=x".into(),
        )];
        req.body=b"--x\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.bin\"\r\n\r\n\0\xff\r\n--x--\r\n".to_vec();
        assert_eq!(
            run("return request.files['file'].body", req)?.body(),
            [0, 255]
        );
        assert_eq!(
            run(
                "return Response('body',status=202), {'X-Test':'value'}",
                request()?
            )?
            .status(),
            202
        );
        Ok(())
    }
    #[test]
    fn bad_json_and_response_fail_before_completion() -> Result<(), RuntimeError> {
        let mut req = request()?;
        req.body = b"bad".to_vec();
        assert!(
            matches!(run("return request.json",req.clone()),Err(e) if e.code==ErrorCode::HttpBadRequest)
        );
        let r = run("return {'ignored': request.get_json(silent=True)}", req)?;
        assert_eq!(r.status(), 200);
        for body in [
            "return None",
            "return 1",
            "return ('x', 1000)",
            "return ('x', {'X-Test':'bad\\r\\nheader'})",
        ] {
            assert!(run(body, request()?).is_err());
        }
        Ok(())
    }
}

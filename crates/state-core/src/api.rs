//! HTTP route publication, lookup and transactional namespace hosting.
use crate::{Context, Error, Result, Runtime, Sources};
use state_contract::http::*;
use state_runtime::ModuleSources;

/// HTTP declarations with injectable source and runtime validation boundaries.
#[derive(Clone)]
pub struct Api {
    sources: Sources,
    runtime: Runtime,
}
impl Api {
    /// Supply source loading and interpreter dependencies explicitly.
    pub fn new(sources: Sources, runtime: Runtime) -> Self {
        Self { sources, runtime }
    }
    /// Pin a handler or publish a live static mount. All failures poison the caller's Context.
    pub fn publish(
        &self,
        mut input: ApiPublishInput,
        context: &mut Context,
    ) -> Result<ApiPublished> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().into();
            let target = match &input.target {
                TargetInput::Function { file, symbol } => {
                    let file = state_contract::VirtualPath::parse(file)?;
                    let limits = context.runtime_limits()?;
                    let sources = self.sources.snapshot(
                        &input.namespace,
                        file.as_str(),
                        limits.max_source_bytes,
                        context,
                    )?;
                    let modules = ModuleSources {
                        entry_path: file.as_str().into(),
                        files: sources.modules,
                    };
                    self.runtime
                        .validate_http(&sources.source, &modules, symbol, &limits)?;
                    context.check()?;
                    RouteTarget::Function(Handler {
                        file: file.as_str().into(),
                        symbol: symbol.clone(),
                        sources: state_contract::function::FunctionSources {
                            source: sources.source,
                            modules: modules.files,
                        },
                    })
                }
                TargetInput::Static { directory, index } => {
                    let directory = state_contract::VirtualPath::parse(directory)?;
                    if input.path.tail().is_none() {
                        return Err(Error::invalid(
                            "static route requires a final catch-all capture",
                        ));
                    }
                    if input.methods.is_empty() {
                        input.methods.insert(Method::parse("GET")?);
                    }
                    if input
                        .methods
                        .iter()
                        .any(|m| !matches!(m.as_str(), "GET" | "HEAD"))
                    {
                        return Err(Error::invalid("static mounts accept only GET/HEAD"));
                    }
                    if let Some(index) = index
                        && (index.is_empty()
                            || index.contains(['/', '\\'])
                            || matches!(index.as_str(), "." | ".."))
                    {
                        return Err(Error::invalid("index must be a single file name"));
                    }
                    RouteTarget::Static {
                        directory: directory.as_str().into(),
                        index: index.clone(),
                    }
                }
            };
            let route = Route {
                name: input.name.clone(),
                path: input.path.clone(),
                methods: input.methods.clone(),
                target,
            };
            Ok(context.transaction().publish_api(&input, route)?)
        })
    }
    /// Get metadata without exposing pinned source.
    pub fn get(&self, input: ApiGetInput, context: &mut Context) -> Result<ApiPublished> {
        context.operation(|context| {
            let namespace = context.namespace(&input.namespace)?;
            let tx = context.transaction();
            tx.api_routes(namespace.as_str())?
                .remove(&input.name)
                .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "API route does not exist"))
        })
    }
    /// List metadata for the namespace's named routes.
    pub fn list(&self, input: ApiListInput, context: &mut Context) -> Result<Vec<ApiPublished>> {
        context.operation(|context| {
            let namespace = context.namespace(&input.namespace)?;
            let tx = context.transaction();
            Ok(tx.api_routes(namespace.as_str())?.into_values().collect())
        })
    }
    /// Remove a route with an optional namespace revision guard.
    pub fn remove(&self, mut input: ApiRemoveInput, context: &mut Context) -> Result<ApiRemoved> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().into();
            context.transaction().remove_api(&input)?;
            Ok(ApiRemoved {
                name: input.name,
                removed: true,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Controller, test_support};
    use serde::de::DeserializeOwned;
    use serde_json::json;
    fn parse<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
        serde_json::from_value(value).map_err(|e| Error::invalid(e.to_string()))
    }
    fn setup() -> Result<Controller> {
        let fs = state_fs::MemoryFs::new();
        let c = test_support::memory_controller(&fs)?;
        c.create_namespace(parse(json!({"name":"web"}))?)?;
        c.create_namespace(parse(json!({"name":"other"}))?)?;
        c.create_database(parse(json!({"namespace":"web","database":"app"}))?)?;
        c.execute_database(parse(
            json!({"namespace":"web","database":"app","sql":"CREATE TABLE counts(n);"}),
        )?)?;
        c.execute_database(parse(
            json!({"namespace":"web","database":"app","sql":"INSERT INTO counts VALUES (0)"}),
        )?)?;
        Ok(c)
    }
    fn publish(
        c: &Controller,
        name: &str,
        path: &str,
        body: &str,
        methods: serde_json::Value,
    ) -> Result<()> {
        let source = format!(
            "def handle(request):\n{}",
            body.lines()
                .map(|line| format!("    {line}\n"))
                .collect::<String>()
        );
        let file = format!("/{name}.py");
        c.write_file(parse(json!({"namespace":"web","path":file,"text":source}))?)?;
        c.publish_api(parse(json!({"namespace":"web","name":name,"path":path,"methods":methods,"target":{"kind":"function","file":file,"symbol":"handle"}}))?)?;
        Ok(())
    }
    fn request(method: &str, path: &str) -> Result<HttpRequest> {
        Ok(HttpRequest {
            method: Method::parse(method)?,
            path: format!("/web{path}"),
            query: String::new(),
            scheme: "http".into(),
            host: "localhost:8000".into(),
            headers: vec![],
            body: vec![],
            view_args: Default::default(),
        })
    }
    fn count(c: &Controller) -> Result<serde_json::Value> {
        serde_json::to_value(c.query_database(parse(
            json!({"namespace":"web","database":"app","sql":"SELECT n FROM counts"}),
        )?)?)
        .map_err(|e| Error::invalid(e.to_string()))
    }
    #[test]
    fn http_writes_commit_and_invalid_response_rolls_back() -> Result<()> {
        let c = setup()?;
        publish(
            &c,
            "bump",
            "/bump",
            "db_execute('app','UPDATE counts SET n=n+1')\nreturn {'n': db_query('app','SELECT n FROM counts')['rows'][0][0]}, 201",
            json!([]),
        )?;
        let response = c.handle_http("web", "/bump", request("CUSTOM", "/bump")?)?;
        assert_eq!(response.status(), 201);
        assert_eq!(count(&c)?["rows"], json!([[1]]));
        publish(
            &c,
            "bad",
            "/bad",
            "db_execute('app','UPDATE counts SET n=n+1')\nwrite_text('/partial','bad')\nreturn None",
            json!([]),
        )?;
        assert!(
            c.handle_http("web", "/bad", request("POST", "/bad")?)
                .is_err()
        );
        assert_eq!(count(&c)?["rows"], json!([[1]]));
        assert!(
            c.read_file(parse(json!({"namespace":"web","path":"/partial"}))?)
                .is_err()
        );
        // A valid application error status is a result and commits its writes.
        publish(
            &c,
            "error",
            "/error",
            "db_execute('app','UPDATE counts SET n=n+1')\nreturn 'application error', 422",
            json!([]),
        )?;
        assert_eq!(
            c.handle_http("web", "/error", request("POST", "/error")?)?
                .status(),
            422
        );
        assert_eq!(count(&c)?["rows"], json!([[2]]));
        Ok(())
    }
    #[test]
    fn namespace_scope_pinning_and_lifecycle() -> Result<()> {
        let c = setup()?;
        publish(
            &c,
            "own",
            "/own",
            "return mcp('api.list', {'namespace':'self'})",
            json!([]),
        )?;
        assert_eq!(
            c.handle_http("web", "/own", request("GET", "/own")?)?
                .status(),
            200
        );
        publish(
            &c,
            "escape",
            "/escape",
            "return mcp('fs.write',{'namespace':'other','path':'/escape','text':'bad'})",
            json!([]),
        )?;
        assert!(
            matches!(c.handle_http("web","/escape",request("GET","/escape")?),Err(e) if e.code==crate::ErrorCode::PermissionDenied)
        );
        c.write_file(parse(
            json!({"namespace":"web","path":"/answer.py","text":"def answer():\n    return 42"}),
        )?)?;
        c.declare_function(parse(
            json!({"namespace":"web","name":"answer","file":"/answer.py","symbol":"answer"}),
        )?)?;
        publish(
            &c,
            "nested",
            "/nested",
            "return {'answer':call('self','answer')}",
            json!([]),
        )?;
        let nested = c.handle_http("web", "/nested", request("GET", "/nested")?)?;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(nested.body())
                .map_err(|e| Error::invalid(e.to_string()))?["answer"],
            42
        );
        publish(&c, "pin", "/pin", "return 'pinned'", json!(["GET"]))?;
        c.write_file(parse(json!({"namespace":"web","path":"/pin.py","text":"def handle(request):\n    return 'changed'"}))?)?;
        assert_eq!(
            c.handle_http("web", "/pin", request("GET", "/pin")?)?
                .body(),
            b"pinned"
        );
        c.copy_namespace(parse(json!({"namespace":"web","name":"copy"}))?)?;
        assert_eq!(
            c.handle_http("copy", "/pin", request("GET", "/pin")?)?
                .body(),
            b"pinned"
        );
        c.update_namespace(parse(json!({"namespace":"copy","name":"renamed"}))?)?;
        assert_eq!(
            c.handle_http("renamed", "/pin", request("GET", "/pin")?)?
                .body(),
            b"pinned"
        );
        c.delete_namespace(parse(json!({"namespace":"renamed"}))?)?;
        assert!(
            c.handle_http("renamed", "/pin", request("GET", "/pin")?)
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn routing_methods_precedence_and_conflicts() -> Result<()> {
        let c = setup()?;
        publish(
            &c,
            "capture",
            "/item/{id}",
            "return request.view_args",
            json!(["GET"]),
        )?;
        publish(&c, "literal", "/item/new", "return 'literal'", json!([]))?;
        assert_eq!(
            c.handle_http("web", "/item/new", request("GET", "/item/new")?)?
                .body(),
            b"literal"
        );
        assert_eq!(
            c.handle_http("web", "/item/7", request("POST", "/item/7")?)?
                .status(),
            405
        );
        assert_eq!(
            c.handle_http("web", "/item/7", request("OPTIONS", "/item/7")?)?
                .status(),
            204
        );
        publish(
            &c,
            "head",
            "/item/{other}",
            "return 'explicit head'",
            json!(["HEAD"]),
        )?;
        assert_eq!(
            c.handle_http("web", "/item/7", request("HEAD", "/item/7")?)?
                .body(),
            b"explicit head"
        );
        assert!(
            publish(
                &c,
                "ambiguous",
                "/item/{other}",
                "return 'bad'",
                json!(["GET"])
            )
            .is_err()
        );
        assert_eq!(
            c.handle_http("web", "/missing", request("GET", "/missing")?)?
                .status(),
            404
        );
        Ok(())
    }
    #[test]
    fn static_binary_index_etag_and_live_files() -> Result<()> {
        let c = setup()?;
        c.write_file(parse(
            json!({"namespace":"web","path":"/public/index.html","text":"<h1>hello</h1>"}),
        )?)?;
        c.write_file(parse(
            json!({"namespace":"web","path":"/public/data.bin","base64":"AP8="}),
        )?)?;
        c.publish_api(parse(json!({"namespace":"web","name":"site","path":"/site/{*path}","target":{"kind":"static","directory":"/public","index":"index.html"}}))?)?;
        assert_eq!(
            c.handle_http("web", "/site", request("GET", "/site")?)?
                .status(),
            308
        );
        let response = c.handle_http("web", "/site/", request("GET", "/site/")?)?;
        assert_eq!(response.body(), b"<h1>hello</h1>");
        let etag = response
            .headers()
            .iter()
            .find(|(k, _)| k == "etag")
            .map(|(_, v)| v.clone())
            .ok_or_else(|| Error::invalid("missing etag"))?;
        let mut req = request("GET", "/site/")?;
        req.headers.push(("if-none-match".into(), etag));
        assert_eq!(c.handle_http("web", "/site/", req)?.status(), 304);
        assert_eq!(
            c.handle_http("web", "/site/data.bin", request("GET", "/site/data.bin")?)?
                .body(),
            [0, 255]
        );
        c.write_file(parse(
            json!({"namespace":"web","path":"/public/index.html","text":"updated"}),
        )?)?;
        assert_eq!(
            c.handle_http("web", "/site/", request("GET", "/site/")?)?
                .body(),
            b"updated"
        );
        let guarded=c.publish_api(parse(json!({"namespace":"web","name":"site","path":"/site/{*path}","expected_revision":"stale","target":{"kind":"static","directory":"/public"}}))?);
        assert!(matches!(guarded,Err(error) if error.code==crate::ErrorCode::Conflict));
        let routes = c.list_api(ApiListInput {
            namespace: "web".into(),
        })?;
        assert_eq!(routes.len(), 1);
        c.remove_api(ApiRemoveInput {
            namespace: "web".into(),
            name: "site".into(),
            expected_revision: None,
        })?;
        assert!(
            c.list_api(ApiListInput {
                namespace: "web".into()
            })?
            .is_empty()
        );
        Ok(())
    }
}

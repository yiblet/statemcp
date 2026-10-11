//! Transactional route resolution, scoped HTTP execution and static serving.
use crate::{Context, Error, Execution, Files, Result};
use state_contract::http::*;
use std::collections::BTreeSet;
/// HTTP use case composed from route metadata, live files and scoped execution.
#[derive(Clone)]
pub struct HttpApplication {
    files: Files,
    execution: Execution,
}
impl HttpApplication {
    /// Construct hosting with the same primitive and execution services as other adapters.
    pub fn new(files: Files, execution: Execution) -> Self {
        Self { files, execution }
    }
    /// Resolve and execute within the caller's transaction; never opens a nested root.
    pub fn handle(
        &self,
        namespace: &str,
        path: &str,
        mut request: HttpRequest,
        context: &mut Context,
    ) -> Result<HttpResponse> {
        context.operation(|context| {
            let namespace = context.namespace(namespace)?;
            // Clone the selected declaration: callbacks can replace routes while the pinned run is active.
            let routes = context.transaction().api_routes(namespace.as_str())?;
            let mut matches = routes
                .values()
                .filter_map(|route| route.path.captures(path).map(|captures| (route, captures)))
                .collect::<Vec<_>>();
            matches.sort_by_key(|(route, _)| std::cmp::Reverse(route.path.specificity()));
            if let Some((best, _)) = matches.first() {
                let rank = best.path.specificity();
                matches.retain(|(route, _)| route.path.specificity() == rank);
            }
            let method = &request.method;
            let selected = matches
                .iter()
                .find(|(route, _)| route.methods.contains(method))
                .or_else(|| {
                    matches.iter().find(|(route, _)| {
                        route.methods.is_empty()
                            || route.methods.contains(method)
                            || (method.as_str() == "HEAD"
                                && route.methods.iter().any(|m| m.as_str() == "GET"))
                    })
                });
            let (route, captures) = match selected {
                Some((route, captures)) => ((*route).clone(), captures.clone()),
                None if matches.is_empty() => return response(404, vec![], b"Not Found".to_vec()),
                None => {
                    let mut allowed = BTreeSet::new();
                    for (route, _) in &matches {
                        for method in &route.methods {
                            allowed.insert(method.as_str().to_owned());
                            if method.as_str() == "GET" {
                                allowed.insert("HEAD".into());
                            }
                        }
                    }
                    allowed.insert("OPTIONS".into());
                    return response(
                        if method.as_str() == "OPTIONS" {
                            204
                        } else {
                            405
                        },
                        vec![(
                            "allow".into(),
                            allowed.into_iter().collect::<Vec<_>>().join(", "),
                        )],
                        vec![],
                    );
                }
            };
            let route = context
                .transaction()
                .api_route(namespace.as_str(), &route.name)?;
            request.view_args = captures;
            match route.target {
                RouteTarget::Function(handler) => self
                    .execution
                    .invoke_http_handler(namespace, handler, request, context),
                RouteTarget::Static { directory, index } => {
                    context.scoped(namespace.clone(), |context| {
                        let capture = route.path.tail().ok_or_else(|| {
                            Error::new(
                                crate::ErrorCode::CorruptStore,
                                "static route has no capture",
                            )
                        })?;
                        let tail = request
                            .view_args
                            .get(capture)
                            .map(String::as_str)
                            .ok_or_else(|| {
                                Error::new(
                                    crate::ErrorCode::CorruptStore,
                                    "static capture missing after route match",
                                )
                            })?;
                        let path = static_path(&directory, tail)?;
                        let mut resolved = path.as_str().to_owned();
                        let stat = self.files.stat_file(
                            state_contract::file::FsStatInput {
                                namespace: namespace.as_str().into(),
                                path: resolved.clone(),
                            },
                            context,
                        )?;
                        if matches!(stat, state_contract::file::FileStat::Directory { .. }) {
                            if !request.path.ends_with('/') {
                                let location = format!(
                                    "{}/{}",
                                    request.path,
                                    if request.query.is_empty() {
                                        String::new()
                                    } else {
                                        format!("?{}", request.query)
                                    }
                                );
                                return response(308, vec![("location".into(), location)], vec![]);
                            }
                            let Some(index) = index else {
                                return response(404, vec![], b"Not Found".to_vec());
                            };
                            resolved = format!("{}/{index}", resolved.trim_end_matches('/'));
                        }
                        let bytes = self.files.read_bytes(
                            state_contract::file::FsReadInput {
                                namespace: namespace.as_str().into(),
                                path: resolved.clone(),
                            },
                            context,
                        )?;
                        let etag = format!("\"{:x}\"", sha2::Sha256::digest(&bytes));
                        let fields = vec![
                            ("etag".into(), etag.clone()),
                            ("content-type".into(), mime(&resolved).into()),
                            ("content-length".into(), bytes.len().to_string()),
                        ];
                        if request.headers.iter().any(|(k, v)| {
                            k.eq_ignore_ascii_case("if-none-match")
                                && (v == "*"
                                    || v.split(',')
                                        .any(|s| s.trim().trim_start_matches("W/") == etag))
                        }) {
                            return response(304, fields, vec![]);
                        }
                        response(200, fields, bytes)
                    })
                }
            }
        })
    }
}
use sha2::Digest;
fn response(status: u16, fields: Vec<(String, String)>, bytes: Vec<u8>) -> Result<HttpResponse> {
    HttpResponse::new(status, fields, bytes).map_err(Into::into)
}
fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

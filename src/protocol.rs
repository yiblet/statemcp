//! MCP tools use the official SDK's typed parameter, routing, and handler macros.
use crate::{Controller, CoreError};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        common::FromContextPart, router::tool::ToolRouter, tool::ToolCallContext,
        wrapper::Parameters,
    },
    model::{
        CacheScope, CallToolResult, ContentBlock, Implementation, ListResourcesResult,
        PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Serialize;
use state_contract::http::{ApiGetInput, ApiListInput, ApiPublishInput, ApiRemoveInput};
use state_contract::{database::*, discovery::*, execution::*, file::*, function::*, namespace::*};
use state_http::{BlockingError, BlockingExecutor, Hosting, RequestOrigin};

#[path = "resources.rs"]
mod resources;

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const SERVER_INSTRUCTIONS: &str = "Start with namespace.list to discover available namespaces and their descriptions. Then use function.list to discover published functions in the relevant namespace. StateMCP lets you create SQLite databases and publish Python tools that use them. Create a namespace and databases, write Python source with fs.write, then publish with function.declare and invoke with call. Use describe with mode=runtime for Python helpers and mode=full for exact schemas. Namespace selectors accept names or stable UUIDs. Published functions have full access to their own namespace only; root execute scripts have owner access. Use expected_revision and expected_version for optimistic concurrency. Tool content contains a JSON-encoded text block; isError indicates an application failure. Resources: use resources/list to discover guides and resources/read to load a matching guide before starting that workload. statemcp://guides/api-and-sites — HTTP API and website authoring: small MCP payloads, CDN imports, and simple HTMX/Tailwind patterns. Read it when creating or editing HTTP routes, handlers, pages, or static sites. statemcp://guides/python — Writing Python in StateMCP: execution, helpers, namespace scope, imports, transactions, and return values. Read it before writing or changing scripts, published functions, or HTTP handlers.";

#[derive(Clone)]
pub struct Server {
    transfers: Option<state_http::Transfers>,
    hosting: Option<Hosting>,
    executor: BlockingExecutor,
    tool_router: ToolRouter<Self>,
}

/// Preserve the exact external inputs for idempotency hashing before typed extraction.
struct ReceiptContext(Option<state_core::ReceiptIdentity>);
impl<S> FromContextPart<ToolCallContext<'_, S>> for ReceiptContext {
    fn from_context_part(context: &mut ToolCallContext<'_, S>) -> Result<Self, ErrorData> {
        let identity = context
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("idempotency_key").map(|key| (arguments, key)))
            .map(|(arguments, key)| {
                let key = key.as_str().ok_or_else(|| {
                    ErrorData::invalid_params("idempotency_key must be a string", None)
                })?;
                state_core::ReceiptIdentity::for_arguments(key, context.name(), arguments)
                    .map_err(|e| ErrorData::invalid_params(e.message, None))
            })
            .transpose()?;
        Ok(Self(identity))
    }
}

struct HostedOrigin(Option<RequestOrigin>);
impl<S> FromContextPart<ToolCallContext<'_, S>> for HostedOrigin {
    fn from_context_part(context: &mut ToolCallContext<'_, S>) -> Result<Self, ErrorData> {
        Ok(Self(
            context
                .request_context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<RequestOrigin>())
                .cloned(),
        ))
    }
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: &'a CoreError,
}
fn tool_error(error: &CoreError) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&ErrorEnvelope { error })
        .map_err(|_| ErrorData::internal_error("Error serialization failed", None))?;
    Ok(CallToolResult::error(vec![ContentBlock::text(text)]))
}

impl Server {
    /// Configure endpoint URL reporting from actual listener addresses.
    pub fn with_hosting(mut self, hosting: Hosting) -> Self {
        self.hosting = Some(hosting);
        self
    }
    /// Enable HTTP-only file capabilities using shared listener token state.
    pub fn with_transfers(mut self, transfers: state_http::Transfers) -> Self {
        self.transfers = Some(transfers);
        self.tool_router.enable_route("fs.upload");
        self.tool_router.enable_route("fs.download");
        self
    }
    /// Construct an MCP adapter with explicitly supplied shared blocking execution.
    pub fn new(executor: BlockingExecutor) -> Self {
        let mut tool_router = Self::tool_router();
        tool_router.disable_route("fs.upload");
        tool_router.disable_route("fs.download");
        Self {
            transfers: None,
            hosting: None,
            executor,
            tool_router,
        }
    }
    async fn run_blocking<T: serde::Serialize + Send + 'static>(
        &self,
        action: impl FnOnce(Controller) -> state_core::Result<T> + Send + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let span = tracing::info_span!("tool_call");
        let result = {
            use tracing::Instrument;
            self.executor.run(action).instrument(span).await
        };
        match result {
            Ok(value) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string(&value)
                    .map_err(|_| ErrorData::internal_error("Result serialization failed", None))?,
            )])),
            Err(BlockingError::Application(error)) => {
                tracing::debug!(code = %error.code, "Tool invocation failed");
                tool_error(&error)
            }
            Err(BlockingError::Task(error)) => {
                tracing::error!(%error, "Invocation worker failed");
                Err(ErrorData::internal_error("Invocation worker failed", None))
            }
        }
    }
}

#[tool_router]
impl Server {
    /// Mint a 10-minute single-use multipart upload URL for exact namespace paths. Send file fields named by returned paths. HTTP only; maximum 10 MiB of file data per request.
    #[tool(name = "fs.upload")]
    async fn fs_upload(
        &self,
        Parameters(input): Parameters<state_contract::transfer::FsUploadInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let service = self
            .transfers
            .as_ref()
            .ok_or_else(|| ErrorData::invalid_params("HTTP transfer service unavailable", None))?;
        transfer_result(service.upload(input).await)
    }
    /// Mint a 10-minute single-use download URL for exact paths or glob patterns. GET streams a tar archive preserving folders. HTTP only; format defaults to tar.
    #[tool(name = "fs.download")]
    async fn fs_download(
        &self,
        Parameters(input): Parameters<state_contract::transfer::FsDownloadInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let service = self
            .transfers
            .as_ref()
            .ok_or_else(|| ErrorData::invalid_params("HTTP transfer service unavailable", None))?;
        transfer_result(service.download(input).await)
    }
    /// Create an empty namespace with an optional description for files, SQLite databases, and published functions.
    #[tool(name = "namespace.create")]
    async fn namespace_create(
        &self,
        Parameters(inputs): Parameters<NamespaceCreateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.create_namespace(inputs))
            .await
    }
    /// Primary entry point for discovering what is available in StateMCP. List namespaces with their names, descriptions, stable IDs, revisions, and resource counts. Then use function.list to discover published functions in the relevant namespace.
    #[tool(name = "namespace.list")]
    async fn namespace_list(
        &self,
        Parameters(_inputs): Parameters<NamespaceListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.list_namespaces())
            .await
    }
    /// Get a namespace’s name, description, stable ID, revision, and resource counts.
    #[tool(name = "namespace.get")]
    async fn namespace_get(
        &self,
        Parameters(inputs): Parameters<NamespaceGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.get_namespace(inputs))
            .await
    }
    /// Update a namespace name or description while preserving its stable ID and contents. Omitted description is preserved; null clears it.
    #[tool(name = "namespace.update")]
    async fn namespace_update(
        &self,
        Parameters(inputs): Parameters<NamespaceUpdateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.update_namespace(inputs))
            .await
    }
    /// Copy a namespace and its contents to a new name. Subsequent changes are independent.
    #[tool(name = "namespace.copy")]
    async fn namespace_copy(
        &self,
        Parameters(inputs): Parameters<NamespaceCopyInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.copy_namespace(inputs))
            .await
    }
    /// Delete a namespace and its contents from active use. Its name remains reserved.
    #[tool(name = "namespace.delete")]
    async fn namespace_delete(
        &self,
        Parameters(inputs): Parameters<NamespaceDeleteInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.delete_namespace(inputs))
            .await
    }

    /// List retained revisions newest first, including named pins.
    #[tool(name = "namespace.history")]
    async fn namespace_history(
        &self,
        Parameters(inputs): Parameters<NamespaceGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.namespace_history(inputs))
            .await
    }

    /// Pin a retained namespace revision with an optional unique name. At most seven pins; unpin one before adding an eighth.
    #[tool(name = "namespace.pin")]
    async fn namespace_pin(
        &self,
        Parameters(inputs): Parameters<NamespacePinInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.pin_revision(inputs))
            .await
    }

    /// Unpin a retained revision. Content outside recent history is collected at commit.
    #[tool(name = "namespace.unpin")]
    async fn namespace_unpin(
        &self,
        Parameters(inputs): Parameters<NamespaceRevisionInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.unpin_revision(inputs))
            .await
    }
    /// Read a virtual file as UTF-8 text or base64 for binary data.
    #[tool(name = "fs.read")]
    async fn fs_read(
        &self,
        Parameters(inputs): Parameters<FsReadInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.read_file(inputs))
            .await
    }
    /// Get a virtual file’s metadata or check whether a virtual directory exists.
    #[tool(name = "fs.stat")]
    async fn fs_stat(
        &self,
        Parameters(inputs): Parameters<FsStatInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.stat_file(inputs))
            .await
    }
    /// List files and directories directly beneath a virtual path.
    #[tool(name = "fs.list")]
    async fn fs_list(
        &self,
        Parameters(inputs): Parameters<FsListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.list_files(inputs))
            .await
    }
    /// Create or replace a virtual file. Supply either text or base64.
    #[tool(name = "fs.write")]
    async fn fs_write(
        &self,
        Parameters(inputs): Parameters<FsWriteInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.write_file(inputs))
            .await
    }
    /// Append text or base64-encoded bytes to a virtual file.
    #[tool(name = "fs.append")]
    async fn fs_append(
        &self,
        Parameters(inputs): Parameters<FsAppendInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.append_file(inputs))
            .await
    }
    /// Move a virtual file to another path in the same namespace.
    #[tool(name = "fs.move")]
    async fn fs_move(
        &self,
        Parameters(inputs): Parameters<FsMoveInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.move_file(inputs))
            .await
    }
    /// Copy a virtual file to another path in the same namespace.
    #[tool(name = "fs.copy")]
    async fn fs_copy(
        &self,
        Parameters(inputs): Parameters<FsCopyInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.copy_file(inputs))
            .await
    }
    /// Delete a virtual file, or delete a directory’s contents with recursive=true.
    #[tool(name = "fs.delete")]
    async fn fs_delete(
        &self,
        Parameters(inputs): Parameters<FsDeleteInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.delete_file(inputs))
            .await
    }
    /// List the named SQLite databases in a namespace.
    #[tool(name = "db.list")]
    async fn db_list(
        &self,
        Parameters(inputs): Parameters<DbListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.list_databases(inputs))
            .await
    }
    /// Create an empty named SQLite database in a namespace.
    #[tool(name = "db.create")]
    async fn db_create(
        &self,
        Parameters(inputs): Parameters<DbCreateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.create_database(inputs))
            .await
    }
    /// Drop a named SQLite database. Functions resolve database names within their namespace on each call.
    #[tool(name = "db.drop")]
    async fn db_drop(
        &self,
        Parameters(inputs): Parameters<DbDropInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.drop_database(inputs))
            .await
    }
    /// Run one read-only SQL statement with optional bound parameters. Returns columns, rows, and rows_affected.
    #[tool(name = "db.query")]
    async fn db_query(
        &self,
        Parameters(inputs): Parameters<DbQueryInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.query_database(inputs))
            .await
    }
    /// Run one SQL statement with optional bound parameters, including writes and RETURNING. Returns columns, rows, and rows_affected.
    #[tool(name = "db.execute")]
    async fn db_execute(
        &self,
        Parameters(inputs): Parameters<DbExecuteInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.execute_database(inputs))
            .await
    }
    /// Inspect a database’s tables, columns, indexes, foreign keys, schema, migration history, and storage metadata.
    #[tool(name = "db.inspect")]
    async fn db_inspect(
        &self,
        Parameters(inputs): Parameters<DbInspectInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.inspect_database(inputs))
            .await
    }
    /// Get the ordered IDs and checksums of applied database migrations.
    #[tool(name = "db.migrations")]
    async fn db_migrations(
        &self,
        Parameters(inputs): Parameters<DbMigrationsInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.database_migrations(inputs))
            .await
    }
    /// Apply an ordered batch of SQL migrations atomically. Previously applied IDs must match their stored checksums and order.
    #[tool(name = "db.migrate")]
    async fn db_migrate(
        &self,
        Parameters(inputs): Parameters<DbMigrateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.migrate_database(inputs))
            .await
    }
    /// List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts, without source code. Use function.get with namespace and name for a full declaration.
    #[tool(name = "function.list")]
    async fn function_list(
        &self,
        Parameters(inputs): Parameters<FunctionListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.list_functions(inputs))
            .await
    }
    /// Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again.
    #[tool(name = "function.get")]
    async fn function_get(
        &self,
        Parameters(inputs): Parameters<FunctionGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.get_function(inputs))
            .await
    }
    /// Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected.
    #[tool(name = "function.remove")]
    async fn function_remove(
        &self,
        Parameters(inputs): Parameters<FunctionRemoveInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.remove_function(inputs))
            .await
    }
    /// Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions can access all files, databases, and functions in their own namespace only. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls. Before writing Python, read MCP resource statemcp://guides/python (execution, helpers, scope, imports, transactions, and return values).
    #[tool(name = "function.declare")]
    async fn function_declare(
        &self,
        Parameters(inputs): Parameters<FunctionDeclareInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.declare_function(inputs))
            .await
    }
    /// Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete declaration. Functions retain full access to their own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version. Before writing Python, read MCP resource statemcp://guides/python (execution, helpers, scope, imports, transactions, and return values).
    #[tool(name = "function.update")]
    async fn function_update(
        &self,
        Parameters(inputs): Parameters<FunctionUpdateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.update_function(inputs))
            .await
    }
    /// Invoke a published function using namespace, function, and an arguments object matching its input schema. Returns the function's JSON value directly. The function runs with full access to its own namespace only; nested calls and writes share one transaction and failures roll back changes. expected_version guards against calling changed code. Reuse an idempotency_key only with the identical request to replay a completed result without repeating writes.
    #[tool(name = "call")]
    async fn call(
        &self,
        receipt: ReceiptContext,
        Parameters(inputs): Parameters<CallInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.call_function(inputs, receipt.0))
            .await
    }
    /// Run a Python script with owner access in one transaction. The inputs argument is available as the Python variable inputs; the final expression and printed output are returned as {value, stdout}. Set namespace to use db_query(database, sql, params=[]), db_execute(database, sql, params=[]), db_inspect(database), read_text(path), and write_text(path, text). SQL helpers return {columns, rows, rows_affected}. Use mcp(name, arguments={}) for any tool or call(namespace, function, arguments={}) for a published function. Failures roll back all nested writes. Imports load Python files and packages from the selected namespace, with per-run module caching. Published functions use pinned module sources. Reuse an idempotency_key only for an identical request. Before writing Python, read MCP resource statemcp://guides/python (execution, helpers, scope, imports, transactions, and return values).
    #[tool(name = "execute")]
    async fn execute(
        &self,
        receipt: ReceiptContext,
        Parameters(inputs): Parameters<ExecuteInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.execute_script(inputs, receipt.0))
            .await
    }
    /// Publish or replace an HTTP function route or static website mount beneath /namespace/. Handler receives one Request and returns Flask buffered response forms. Omitted methods means all methods for functions. Namespace-wide access and atomic writes; source/imports pinned at publication. Returns hosted URL patterns. For HTTP API or website work, first read MCP resource statemcp://guides/api-and-sites (small payloads, CDN imports, HTMX and Tailwind patterns).
    #[tool(name = "api.publish")]
    async fn api_publish(
        &self,
        origin: HostedOrigin,
        Parameters(input): Parameters<ApiPublishInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let hosting = self.hosting.clone();
        self.run_blocking(move |controller| {
            let mut result = controller.publish_api(input)?;
            if let Some(hosting) = hosting {
                hosting.locate(&mut result.result, origin.0.as_ref());
            }
            Ok(result)
        })
        .await
    }
    /// Get one named HTTP route and its hosted URL patterns, without pinned source.
    #[tool(name = "api.get")]
    async fn api_get(
        &self,
        origin: HostedOrigin,
        Parameters(input): Parameters<ApiGetInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let hosting = self.hosting.clone();
        self.run_blocking(move |controller| {
            let mut result = controller.get_api(input)?;
            if let Some(hosting) = hosting {
                hosting.locate(&mut result, origin.0.as_ref());
            }
            Ok(result)
        })
        .await
    }
    /// List HTTP function routes and static mounts in a namespace with hosted URL patterns.
    #[tool(name = "api.list")]
    async fn api_list(
        &self,
        origin: HostedOrigin,
        Parameters(input): Parameters<ApiListInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let hosting = self.hosting.clone();
        self.run_blocking(move |controller| {
            let mut result = controller.list_api(input)?;
            if let Some(hosting) = hosting {
                for route in &mut result {
                    hosting.locate(route, origin.0.as_ref());
                }
            }
            Ok(result)
        })
        .await
    }
    /// Remove a named HTTP route, optionally guarding the namespace revision.
    #[tool(name = "api.remove")]
    async fn api_remove(
        &self,
        Parameters(input): Parameters<ApiRemoveInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |controller| controller.remove_api(input))
            .await
    }
    /// Get a compact API overview, mode=runtime for a Python authoring guide, or mode=full for all schemas, or mode=readme for the bundled documentation. Select tool for one tool’s schema, or namespace and optional function for endpoint contracts.
    #[tool(name = "describe")]
    async fn describe(
        &self,
        Parameters(inputs): Parameters<DescribeInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let full = match &inputs {
            DescribeInput::Overview(input) => input.mode == Some(DiscoveryMode::Full),
            _ => false,
        };
        let extras: Vec<state_core::ToolDefinition> = if self.transfers.is_some() {
            [Self::fs_upload_tool_attr(), Self::fs_download_tool_attr()].into_iter().map(|tool| {
                let description = if tool.name == "fs.upload" { "Mint a 10-minute single-use multipart upload URL for exact paths; HTTP only, 10 MiB total." } else { "Mint a 10-minute single-use tar download URL for exact paths or globs; HTTP only." };
                state_core::ToolDefinition { name: tool.name.into_owned(), description, input_schema: full.then(|| rmcp::schemars::Schema::from((*tool.input_schema).clone())) }
            }).collect()
        } else {
            Vec::new()
        };
        if let DescribeInput::Tool(input) = &inputs
            && matches!(input.tool.as_str(), "fs.upload" | "fs.download")
            && let Some(tool) = self.tool_router.get(&input.tool)
        {
            #[derive(Serialize)]
            struct ToolDiscovery<'a> {
                tool: &'a rmcp::model::Tool,
            }
            return Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string(&ToolDiscovery { tool }).map_err(|_| {
                    ErrorData::internal_error("discovery serialization failed", None)
                })?,
            )]));
        }
        self.run_blocking(move |state| {
            let mut discovery = state.describe(inputs)?;
            discovery.extend_tools(extras);
            Ok(discovery)
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("statemcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(SERVER_INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        if request.is_some_and(|params| params.cursor.is_some()) {
            return Err(ErrorData::invalid_params(
                "Resource catalog has no continuation cursor",
                None,
            ));
        }
        Ok(ListResourcesResult::with_all_items(resources::list())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Public))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let content = resources::read(&request.uri)
            .ok_or_else(|| ErrorData::resource_not_found("Unknown StateMCP resource URI", None))?;
        Ok(ReadResourceResult::new(vec![content])
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Public)
            .into())
    }
}

fn transfer_result(
    result: Result<state_contract::transfer::TransferTicket, BlockingError>,
) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(ticket) => Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string(&ticket)
                .map_err(|_| ErrorData::internal_error("transfer serialization failed", None))?,
        )])),
        Err(BlockingError::Application(error)) => tool_error(&error),
        Err(BlockingError::Task(_)) => {
            Err(ErrorData::internal_error("transfer worker failed", None))
        }
    }
}

/// SDK-generated tool definitions.
pub fn tool_definitions() -> Vec<rmcp::model::Tool> {
    let mut router = Server::tool_router();
    router.disable_route("fs.upload");
    router.disable_route("fs.download");
    router.list_all()
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;

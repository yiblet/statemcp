//! MCP tools use the official SDK's typed parameter, routing, and handler macros.
use crate::{CoreError, State};
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{
        common::FromContextPart, router::tool::ToolRouter, tool::ToolCallContext,
        wrapper::Parameters,
    },
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use serde::Serialize;
use state_core::inputs::*;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const SERVER_INSTRUCTIONS: &str = "Start with namespace.list to discover available namespaces and their descriptions. Then use function.list to discover published functions in the relevant namespace. StateMCP lets you create SQLite databases and publish Python tools that use them. Create a namespace and databases, write Python source with fs.write, then publish with function.declare and invoke with call. Use describe with mode=runtime for Python helpers and mode=full for exact schemas. Namespace selectors accept names or stable UUIDs. Published functions have full access to their own namespace only; root execute scripts have owner access. Use expected_revision and expected_version for optimistic concurrency. Tool content contains a JSON-encoded text block; isError indicates an application failure.";

#[derive(Clone)]
pub struct Server {
    state: State,
    slots: Arc<Semaphore>,
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
    pub fn new(state: State) -> Self {
        Self {
            state,
            slots: Arc::new(Semaphore::new(16)),
            tool_router: Self::tool_router(),
        }
    }
    async fn run_blocking<T: serde::Serialize + Send + 'static>(
        &self,
        action: impl FnOnce(State) -> state_core::Result<T> + Send + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let permit = match self.slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return tool_error(&CoreError::new(
                    state_core::ErrorCode::Busy,
                    "Too many active invocations; retry later",
                ));
            }
        };
        let state = self.state.clone();
        let span = tracing::info_span!("tool_call");
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _span = span.enter();
            action(state)
        })
        .await;
        match result {
            Ok(Ok(value)) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string(&value)
                    .map_err(|_| ErrorData::internal_error("Result serialization failed", None))?,
            )])),
            Ok(Err(error)) => {
                tracing::debug!(code = %error.code, "Tool invocation failed");
                tool_error(&error)
            }
            Err(error) => {
                tracing::error!(%error, "Invocation worker failed");
                Err(ErrorData::internal_error("Invocation worker failed", None))
            }
        }
    }
}

#[tool_router]
impl Server {
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
    /// Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions can access all files, databases, and functions in their own namespace only. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls.
    #[tool(name = "function.declare")]
    async fn function_declare(
        &self,
        Parameters(inputs): Parameters<FunctionDeclareInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.declare_function(inputs))
            .await
    }
    /// Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete declaration. Functions retain full access to their own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version.
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
    /// Run a Python script with owner access in one transaction. The inputs argument is available as the Python variable inputs; the final expression and printed output are returned as {value, stdout}. Set namespace to use db_query(database, sql, params=[]), db_execute(database, sql, params=[]), db_inspect(database), read_text(path), and write_text(path, text). SQL helpers return {columns, rows, rows_affected}. Use mcp(name, arguments={}) for any tool or call(namespace, function, arguments={}) for a published function. Failures roll back all nested writes. Imports load Python files and packages from the selected namespace, with per-run module caching. Published functions use pinned module sources. Reuse an idempotency_key only for an identical request.
    #[tool(name = "execute")]
    async fn execute(
        &self,
        receipt: ReceiptContext,
        Parameters(inputs): Parameters<ExecuteInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.execute_script(inputs, receipt.0))
            .await
    }
    /// Get a compact API overview, mode=runtime for a Python authoring guide, or mode=full for all schemas, or mode=readme for the bundled documentation. Select tool for one tool’s schema, or namespace and optional function for endpoint contracts.
    #[tool(name = "describe")]
    async fn describe(
        &self,
        Parameters(inputs): Parameters<DescribeInput>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_blocking(move |state| state.describe(inputs)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("statemcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}

/// SDK-generated tool definitions.
pub fn tool_definitions() -> Vec<rmcp::model::Tool> {
    Server::tool_router().list_all()
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;

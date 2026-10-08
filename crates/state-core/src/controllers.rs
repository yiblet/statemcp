//! Concrete service operations with one transaction per root call.
use crate::{
    Context, CoreLimits, Databases, DiscoveryService, EmbeddedBackend, Error, Execution, Files,
    Functions, Namespaces as NamespaceService, Result, Runtime, RuntimeBackend, Sources,
};
use serde::Serialize;
use serde_json::Value;
use state_contract::{
    database::*, execution::*, file::*, function::*, namespace::*, publication::*,
};
use state_domain::Store;
use std::{path::Path, sync::Arc};

/// Composes feature services and owns root transaction, receipt and result lifecycle.
/// Operations are synchronous; async callers must use a blocking execution boundary.
#[derive(Clone)]
pub struct Controller {
    store: Store,
    runtime: Runtime,
    namespaces: NamespaceService,
    files: Files,
    databases: Databases,
    functions: Functions,
    execution: Execution,
    discovery: DiscoveryService,
    limits: CoreLimits,
    principal: String,
}
impl Controller {
    /// Open an embedded service; does not launch another executable.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::with_backend(
            Store::open(path)?,
            Arc::new(EmbeddedBackend),
        ))
    }
    /// Construct application controllers from explicit storage and runtime dependencies.
    pub fn new(store: Store, runtime: Runtime) -> Self {
        let files = Files;
        let databases = Databases;
        let namespaces = NamespaceService;
        let functions = Functions::new(Sources::new(Arc::new(files)), runtime.clone());
        let discovery = DiscoveryService::new(functions.clone());
        let execution = Execution::new(
            functions.clone(),
            files,
            databases,
            namespaces,
            discovery.clone(),
            runtime.clone(),
        );
        Self {
            store,
            runtime,
            namespaces,
            files,
            databases,
            functions,
            execution,
            discovery,
            limits: CoreLimits::default(),
            principal: "owner".into(),
        }
    }
    /// Construct controllers around an interchangeable configured runtime backend.
    pub fn with_backend(store: Store, backend: Arc<dyn RuntimeBackend>) -> Self {
        Self::new(store, Runtime::new(backend))
    }
    /// File operations for custom typed use cases opened with `transaction`.
    pub fn files(&self) -> &Files {
        &self.files
    }
    /// Database operations for custom typed use cases opened with `transaction`.
    pub fn databases(&self) -> &Databases {
        &self.databases
    }
    /// Namespace operations for custom typed use cases opened with `transaction`.
    pub fn namespaces(&self) -> &NamespaceService {
        &self.namespaces
    }
    /// Function publication and lookup for custom typed use cases.
    pub fn functions(&self) -> &Functions {
        &self.functions
    }
    /// Python execution using the controller's configured feature services.
    pub fn execution(&self) -> &Execution {
        &self.execution
    }
    /// Discovery using the configured function declaration service.
    pub fn discovery(&self) -> &DiscoveryService {
        &self.discovery
    }
    /// Interpreter dependency shared by controllers and their feature services.
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }
    /// Set the trusted receipt identity; rejects empty identities and values over 256 bytes.
    /// This configures receipt scope and does not authenticate or authorize a caller.
    pub fn with_principal(mut self, principal: impl Into<String>) -> Result<Self> {
        let principal = principal.into();
        if principal.is_empty() || principal.len() > 256 {
            return Err(Error::invalid("principal must contain 1..256 bytes"));
        }
        self.principal = principal;
        Ok(self)
    }
    /// Configure budgets used by subsequently opened root contexts.
    pub fn with_limits(mut self, limits: CoreLimits) -> Self {
        self.limits = limits;
        self
    }
    /// Access the configured persistence dependency for embedding and maintenance.
    pub fn store(&self) -> &Store {
        &self.store
    }
}

impl Controller {
    /// Run a typed use case in one transaction shared by all supplied service calls.
    /// Any failed service call poisons the context even if the closure catches its error.
    /// The controller validates the result and commits only after successful completion.
    /// This synchronous entrypoint must run on a blocking thread in async applications.
    pub fn transaction<T: Serialize>(
        &self,
        action: impl FnOnce(&mut Context) -> Result<T>,
    ) -> Result<(T, CommitOutcome)> {
        let mut root = Context::new(self.store.begin()?, self.limits.clone());
        let result = action(&mut root)?;
        root.check()?;
        crate::bounded(&result, self.limits.max_result_bytes)?;
        let commit = root.finish()?;
        Ok((result, commit))
    }
    fn mutation<T: Serialize>(
        &self,
        namespace: &str,
        action: impl FnOnce(&mut Context) -> Result<T>,
    ) -> Result<Committed<T>> {
        let ((result, id), commit) = self.transaction(|root| {
            let result = action(root)?;
            let id = root.transaction().namespace_identity(namespace)?.to_owned();
            Ok((result, id))
        })?;
        Ok(Committed {
            result,
            revision: commit.revision(&id),
        })
    }
    fn namespace_mutation(
        &self,
        action: impl FnOnce(&mut Context) -> Result<NamespaceSummary>,
    ) -> Result<Committed<NamespaceSummary>> {
        let (result, commit) = self.transaction(action)?;
        let revision = commit.revision(&result.id);
        Ok(Committed { result, revision })
    }
    fn receipted<T: ReceiptPayload>(
        &self,
        receipt: Option<&crate::ReceiptIdentity>,
        action: impl FnOnce(&mut Context) -> Result<T>,
    ) -> Result<T> {
        if let Some(identity) = receipt
            && let Some(receipt) = self.store.receipt(&self.principal, identity.key())?
        {
            if receipt.request_hash != identity.hash() {
                return Err(Error::new(
                    crate::ErrorCode::IdempotencyMismatch,
                    "key was already used for a different request",
                ));
            }
            return T::from_receipt(receipt.result);
        }
        let (result, commit) = self.transaction(|root| {
            let result = action(root)?;
            if let Some(identity) = receipt {
                root.transaction().set_receipt(
                    &self.principal,
                    identity.key(),
                    identity.hash(),
                    result.to_receipt(),
                )?;
            }
            Ok(result)
        })?;
        match commit {
            CommitOutcome::Published(_) => Ok(result),
            CommitOutcome::Replayed(result) => T::from_receipt(result),
        }
    }
    /// List namespaces with their names, descriptions, stable IDs, revisions, and resource counts.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn list_namespaces(&self) -> Result<Namespaces> {
        self.transaction(|root| self.namespaces.list_namespaces(NamespaceListInput {}, root))
            .map(|(result, _)| result)
    }
    /// Get a namespace’s name, description, stable ID, revision, and resource counts.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn get_namespace(
        &self,
        input: state_contract::namespace::NamespaceGetInput,
    ) -> Result<NamespaceInfo> {
        self.transaction(|root| self.namespaces.get_namespace(input, root))
            .map(|(result, _)| result)
    }
    /// Delete a namespace and its contents from active use. Its name remains reserved.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn delete_namespace(
        &self,
        input: state_contract::namespace::NamespaceDeleteInput,
    ) -> Result<NamespaceDeleted> {
        self.transaction(|root| self.namespaces.delete_namespace(input, root))
            .map(|(result, _)| result)
    }
    /// Read a virtual file as UTF-8 text or base64 for binary data.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn read_file(&self, input: state_contract::file::FsReadInput) -> Result<FileContents> {
        self.transaction(|root| self.files.read_file(input, root))
            .map(|(result, _)| result)
    }
    /// Get a virtual file’s metadata or check whether a virtual directory exists.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn stat_file(&self, input: state_contract::file::FsStatInput) -> Result<FileStat> {
        self.transaction(|root| self.files.stat_file(input, root))
            .map(|(result, _)| result)
    }
    /// List files and directories directly beneath a virtual path.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn list_files(&self, input: state_contract::file::FsListInput) -> Result<FileEntries> {
        self.transaction(|root| self.files.list_files(input, root))
            .map(|(result, _)| result)
    }
    /// List the named SQLite databases in a namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn list_databases(
        &self,
        input: state_contract::database::DbListInput,
    ) -> Result<state_contract::database::Databases> {
        self.transaction(|root| self.databases.list_databases(input, root))
            .map(|(result, _)| result)
    }
    /// Run one read-only SQL statement with optional bound parameters. Returns columns, rows, and rows_affected.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn query_database(
        &self,
        input: state_contract::database::DbQueryInput,
    ) -> Result<SqlResult> {
        self.transaction(|root| self.databases.query_database(input, root))
            .map(|(result, _)| result)
    }
    /// Inspect a database’s tables, columns, indexes, foreign keys, schema, migration history, and storage metadata.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn inspect_database(
        &self,
        input: state_contract::database::DbInspectInput,
    ) -> Result<DatabaseInspection> {
        self.transaction(|root| self.databases.inspect_database(input, root))
            .map(|(result, _)| result)
    }
    /// Get the ordered IDs and checksums of applied database migrations.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn database_migrations(
        &self,
        input: state_contract::database::DbMigrationsInput,
    ) -> Result<Migrations> {
        self.transaction(|root| self.databases.database_migrations(input, root))
            .map(|(result, _)| result)
    }
    /// List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts, without source code. Use function.get with namespace and name for a full declaration.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn list_functions(
        &self,
        input: state_contract::function::FunctionListInput,
    ) -> Result<state_contract::function::Functions> {
        self.transaction(|root| self.functions.list_functions(input, root))
            .map(|(result, _)| result)
    }
    /// Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn get_function(
        &self,
        input: state_contract::function::FunctionGetInput,
    ) -> Result<state_contract::function::FunctionDeclaration> {
        self.transaction(|root| self.functions.get_function(input, root))
            .map(|(result, _)| result)
    }
    /// Create an empty namespace with an optional description for files, SQLite databases, and published functions.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn create_namespace(
        &self,
        input: state_contract::namespace::NamespaceCreateInput,
    ) -> Result<Committed<NamespaceSummary>> {
        self.namespace_mutation(|root| self.namespaces.create_namespace(input, root))
    }
    /// Update a namespace name or description while preserving its stable ID and contents. Omitted description is preserved; null clears it.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn update_namespace(
        &self,
        input: state_contract::namespace::NamespaceUpdateInput,
    ) -> Result<Committed<NamespaceSummary>> {
        self.namespace_mutation(|root| self.namespaces.update_namespace(input, root))
    }
    /// Copy a namespace and its contents to a new name. Subsequent changes are independent.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn copy_namespace(
        &self,
        input: state_contract::namespace::NamespaceCopyInput,
    ) -> Result<Committed<NamespaceSummary>> {
        self.namespace_mutation(|root| self.namespaces.copy_namespace(input, root))
    }
    /// Create or replace a virtual file. Supply either text or base64.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn write_file(
        &self,
        input: state_contract::file::FsWriteInput,
    ) -> Result<Committed<FileWritten>> {
        let namespace = match &input {
            FsWriteInput::Text(file) => &file.namespace,
            FsWriteInput::Binary(file) => &file.namespace,
        }
        .clone();
        self.mutation(&namespace, |root| self.files.write_file(input, root))
    }
    /// Append text or base64-encoded bytes to a virtual file.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn append_file(
        &self,
        input: state_contract::file::FsAppendInput,
    ) -> Result<Committed<FileWritten>> {
        let namespace = match &input.0 {
            FsWriteInput::Text(file) => &file.namespace,
            FsWriteInput::Binary(file) => &file.namespace,
        }
        .clone();
        self.mutation(&namespace, |root| self.files.append_file(input, root))
    }
    /// Move a virtual file to another path in the same namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn move_file(
        &self,
        input: state_contract::file::FsMoveInput,
    ) -> Result<Committed<FileMoved>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.files.move_file(input, root)
        })
    }
    /// Copy a virtual file to another path in the same namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn copy_file(
        &self,
        input: state_contract::file::FsCopyInput,
    ) -> Result<Committed<FileMoved>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.files.copy_file(input, root)
        })
    }
    /// Delete a virtual file, or delete a directory’s contents with recursive=true.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn delete_file(
        &self,
        input: state_contract::file::FsDeleteInput,
    ) -> Result<Committed<FilesDeleted>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.files.delete_file(input, root)
        })
    }
    /// Create an empty named SQLite database in a namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn create_database(
        &self,
        input: state_contract::database::DbCreateInput,
    ) -> Result<Committed<DatabaseCreated>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.databases.create_database(input, root)
        })
    }
    /// Drop a named SQLite database. Functions resolve database names within their namespace on each call.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn drop_database(
        &self,
        input: state_contract::database::DbDropInput,
    ) -> Result<Committed<DatabaseDropped>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.databases.drop_database(input, root)
        })
    }
    /// Run one SQL statement with optional bound parameters, including writes and RETURNING. Returns columns, rows, and rows_affected.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn execute_database(
        &self,
        input: state_contract::database::DbExecuteInput,
    ) -> Result<Committed<SqlResult>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.databases.execute_database(input, root)
        })
    }
    /// Apply an ordered batch of SQL migrations atomically. Previously applied IDs must match their stored checksums and order.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn migrate_database(
        &self,
        input: state_contract::database::DbMigrateInput,
    ) -> Result<Committed<MigrationsApplied>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.databases.migrate_database(input, root)
        })
    }
    /// Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn remove_function(
        &self,
        input: state_contract::function::FunctionRemoveInput,
    ) -> Result<Committed<FunctionRemoved>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.functions.remove_function(input, root)
        })
    }
    /// Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions can access all files, databases, and functions in their own namespace only. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn declare_function(
        &self,
        input: state_contract::function::FunctionDeclareInput,
    ) -> Result<Committed<FunctionPublished>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.functions.declare_function(input, root)
        })
    }
    /// Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete declaration. Functions retain full access to their own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn update_function(
        &self,
        input: state_contract::function::FunctionUpdateInput,
    ) -> Result<Committed<FunctionPublished>> {
        self.mutation(&input.namespace.clone(), |root| {
            self.functions.update_function(input, root)
        })
    }
    /// Invoke a published function using namespace, function, and an arguments object matching its input schema. Returns the function's JSON value directly. The function runs with full access to its own namespace only; nested calls and writes share one transaction and failures roll back changes. expected_version guards against calling changed code. Reuse an idempotency_key only with the identical request to replay a completed result without repeating writes.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn call_function(
        &self,
        input: state_contract::execution::CallInput,
        receipt: Option<crate::ReceiptIdentity>,
    ) -> Result<Value> {
        crate::ReceiptIdentity::check(input.idempotency_key.as_deref(), receipt.as_ref())?;
        self.receipted(receipt.as_ref(), |root| {
            self.execution.call_function(input, root)
        })
    }
    /// Run a Python script with owner access in one transaction. The inputs argument is available as the Python variable inputs; the final expression and printed output are returned as {value, stdout}. Set namespace to use db_query(database, sql, params=[]), db_execute(database, sql, params=[]), db_inspect(database), read_text(path), and write_text(path, text). SQL helpers return {columns, rows, rows_affected}. Use mcp(name, arguments={}) for any tool or call(namespace, function, arguments={}) for a published function. Failures roll back all nested writes. Imports load Python files and packages from the selected namespace, with per-run module caching. Published functions use pinned module sources. Reuse an idempotency_key only for an identical request.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn execute_script(
        &self,
        input: state_contract::execution::ExecuteInput,
        receipt: Option<crate::ReceiptIdentity>,
    ) -> Result<ExecutionResult> {
        crate::ReceiptIdentity::check(input.idempotency_key.as_deref(), receipt.as_ref())?;
        self.receipted(receipt.as_ref(), |root| {
            self.execution.execute_script(input, root)
        })
    }
    /// Get a compact API overview, mode=runtime for a Python authoring guide, or mode=full for all schemas, or mode=readme for the bundled documentation. Select tool for one tool’s schema, or namespace and optional function for endpoint contracts.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn describe(
        &self,
        input: state_contract::discovery::DescribeInput,
    ) -> Result<crate::Discovery> {
        self.transaction(|root| self.discovery.describe(input, root))
            .map(|(result, _)| result)
    }
}

trait ReceiptPayload: Serialize {
    fn to_receipt(&self) -> ReceiptResult;
    fn from_receipt(result: ReceiptResult) -> Result<Self>
    where
        Self: Sized;
}
impl ReceiptPayload for Value {
    fn to_receipt(&self) -> ReceiptResult {
        ReceiptResult::Function(self.clone())
    }
    fn from_receipt(result: ReceiptResult) -> Result<Self> {
        match result {
            ReceiptResult::Function(value) => Ok(value),
            ReceiptResult::Script(_) => Err(Error::new(
                crate::ErrorCode::CorruptStore,
                "receipt result kind differs",
            )),
        }
    }
}
impl ReceiptPayload for ExecutionResult {
    fn to_receipt(&self) -> ReceiptResult {
        ReceiptResult::Script(self.clone())
    }
    fn from_receipt(result: ReceiptResult) -> Result<Self> {
        match result {
            ReceiptResult::Script(result) => Ok(result),
            ReceiptResult::Function(_) => Err(Error::new(
                crate::ErrorCode::CorruptStore,
                "receipt result kind differs",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn nested_service_failure_rolls_back_files_and_multiple_databases() -> Result<()> {
        let fs = state_fs::MemoryFs::new();
        let controller = Controller::new(test_support::memory_store(&fs)?, Runtime::embedded());
        controller.transaction(|context| {
            controller.namespaces.create_namespace(
                NamespaceCreateInput {
                    name: "notes".into(),
                    description: DescriptionChange::Unchanged,
                },
                context,
            )?;
            for database in ["first", "second"] {
                controller.databases.create_database(
                    DbCreateInput {
                        namespace: "notes".into(),
                        database: database.into(),
                        expected_revision: None,
                    },
                    context,
                )?;
                controller.databases.execute_database(
                    DbExecuteInput {
                        namespace: "notes".into(),
                        database: database.into(),
                        sql: "CREATE TABLE entries(value TEXT)".into(),
                        params: None,
                        expected_revision: None,
                    },
                    context,
                )?;
            }
            Ok(())
        })?;
        let result = controller.transaction(|context| {
            controller.files.write_file(
                FsWriteInput::Text(FileTextInput {
                    namespace: "notes".into(),
                    path: "/pending".into(),
                    text: "staged".into(),
                    expected_revision: None,
                }),
                context,
            )?;
            for database in ["first", "second"] {
                controller.databases.execute_database(
                    DbExecuteInput {
                        namespace: "notes".into(),
                        database: database.into(),
                        sql: "INSERT INTO entries VALUES ('staged')".into(),
                        params: None,
                        expected_revision: None,
                    },
                    context,
                )?;
            }
            // Catching a service error must not convert a failed transaction into a successful root.
            let failed = controller.databases.execute_database(
                DbExecuteInput {
                    namespace: "notes".into(),
                    database: "first".into(),
                    sql: "INSERT INTO absent VALUES (1)".into(),
                    params: None,
                    expected_revision: None,
                },
                context,
            );
            assert!(failed.is_err());
            Ok(())
        });
        assert!(result.is_err());
        assert!(matches!(
            controller.read_file(FsReadInput {
                namespace: "notes".into(),
                path: "/pending".into(),
            }),
            Err(Error {
                code: crate::ErrorCode::NotFound,
                ..
            })
        ));
        for database in ["first", "second"] {
            let query = controller.query_database(DbQueryInput {
                namespace: "notes".into(),
                database: database.into(),
                sql: "SELECT value FROM entries".into(),
                params: None,
            })?;
            assert!(query.rows.is_empty());
        }
        Ok(())
    }
}

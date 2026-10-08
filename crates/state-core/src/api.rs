//! Concrete service operations with one transaction per root call.
use crate::{Error, Result, State, execution::Root, policy::Access};
use serde::Serialize;
use serde_json::Value;
use state_contract::{
    database::*, execution::*, file::*, function::*, namespace::*, publication::*,
};

impl State {
    fn transaction<T: Serialize>(
        &self,
        action: impl FnOnce(&mut Root) -> Result<T>,
    ) -> Result<(T, CommitOutcome)> {
        let mut root = Root::new(
            self.store.begin()?,
            self.backend.clone(),
            self.limits.clone(),
        );
        root.start_operation()?;
        let result = action(&mut root)?;
        root.check()?;
        crate::bounded(&result, self.limits.max_result_bytes)?;
        let commit = root.tx.commit()?;
        Ok((result, commit))
    }
    fn mutation<T: Serialize>(
        &self,
        namespace: &str,
        action: impl FnOnce(&mut Root) -> Result<T>,
    ) -> Result<Committed<T>> {
        let ((result, id), commit) = self.transaction(|root| {
            let result = action(root)?;
            let id = root.tx.namespace_identity(namespace)?.to_owned();
            Ok((result, id))
        })?;
        Ok(Committed {
            result,
            revision: commit.revision(&id),
        })
    }
    fn namespace_mutation(
        &self,
        action: impl FnOnce(&mut Root) -> Result<NamespaceSummary>,
    ) -> Result<Committed<NamespaceSummary>> {
        let (result, commit) = self.transaction(action)?;
        let revision = commit.revision(&result.id);
        Ok(Committed { result, revision })
    }
    fn receipted<T: ReceiptPayload>(
        &self,
        receipt: Option<&crate::ReceiptIdentity>,
        action: impl FnOnce(&mut Root) -> Result<T>,
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
                root.tx.set_receipt(
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
        self.transaction(|root| Ok(root.tx.list_namespaces()?))
            .map(|(result, _)| result)
    }
    /// Get a namespace’s name, description, stable ID, revision, and resource counts.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn get_namespace(
        &self,
        input: state_contract::namespace::NamespaceGetInput,
    ) -> Result<NamespaceInfo> {
        self.transaction(|root| Ok(root.tx.get_namespace(&input)?))
            .map(|(result, _)| result)
    }
    /// Delete a namespace and its contents from active use. Its name remains reserved.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn delete_namespace(
        &self,
        input: state_contract::namespace::NamespaceDeleteInput,
    ) -> Result<NamespaceDeleted> {
        self.transaction(|root| Ok(root.tx.delete_namespace(&input)?))
            .map(|(result, _)| result)
    }
    /// Read a virtual file as UTF-8 text or base64 for binary data.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn read_file(&self, input: state_contract::file::FsReadInput) -> Result<FileContents> {
        self.transaction(|root| Ok(root.tx.read_file(&input)?))
            .map(|(result, _)| result)
    }
    /// Get a virtual file’s metadata or check whether a virtual directory exists.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn stat_file(&self, input: state_contract::file::FsStatInput) -> Result<FileStat> {
        self.transaction(|root| Ok(root.tx.stat_file(&input)?))
            .map(|(result, _)| result)
    }
    /// List files and directories directly beneath a virtual path.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn list_files(&self, input: state_contract::file::FsListInput) -> Result<FileEntries> {
        self.transaction(|root| Ok(root.tx.list_files(&input)?))
            .map(|(result, _)| result)
    }
    /// List the named SQLite databases in a namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn list_databases(
        &self,
        input: state_contract::database::DbListInput,
    ) -> Result<Databases> {
        self.transaction(|root| Ok(root.tx.list_databases(&input)?))
            .map(|(result, _)| result)
    }
    /// Run one read-only SQL statement with optional bound parameters. Returns columns, rows, and rows_affected.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn query_database(
        &self,
        input: state_contract::database::DbQueryInput,
    ) -> Result<SqlResult> {
        self.transaction(|root| Ok(root.tx.query_database(&input)?))
            .map(|(result, _)| result)
    }
    /// Inspect a database’s tables, columns, indexes, foreign keys, schema, migration history, and storage metadata.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn inspect_database(
        &self,
        input: state_contract::database::DbInspectInput,
    ) -> Result<DatabaseInspection> {
        self.transaction(|root| Ok(root.tx.inspect_database(&input)?))
            .map(|(result, _)| result)
    }
    /// Get the ordered IDs and checksums of applied database migrations.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn database_migrations(
        &self,
        input: state_contract::database::DbMigrationsInput,
    ) -> Result<Migrations> {
        self.transaction(|root| Ok(root.tx.database_migrations(&input)?))
            .map(|(result, _)| result)
    }
    /// List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts, without source code. Use function.get with namespace and name for a full declaration.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn list_functions(
        &self,
        input: state_contract::function::FunctionListInput,
    ) -> Result<Functions> {
        self.transaction(|root| Ok(root.tx.list_functions(&input)?))
            .map(|(result, _)| result)
    }
    /// Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn get_function(
        &self,
        input: state_contract::function::FunctionGetInput,
    ) -> Result<state_contract::function::FunctionDeclaration> {
        self.transaction(|root| Ok(root.tx.get_function(&input)?))
            .map(|(result, _)| result)
    }
    /// Create an empty namespace with an optional description for files, SQLite databases, and published functions.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn create_namespace(
        &self,
        input: state_contract::namespace::NamespaceCreateInput,
    ) -> Result<Committed<NamespaceSummary>> {
        self.namespace_mutation(|root| Ok(root.tx.create_namespace(&input)?))
    }
    /// Update a namespace name or description while preserving its stable ID and contents. Omitted description is preserved; null clears it.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn update_namespace(
        &self,
        input: state_contract::namespace::NamespaceUpdateInput,
    ) -> Result<Committed<NamespaceSummary>> {
        self.namespace_mutation(|root| Ok(root.tx.update_namespace(&input)?))
    }
    /// Copy a namespace and its contents to a new name. Subsequent changes are independent.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn copy_namespace(
        &self,
        input: state_contract::namespace::NamespaceCopyInput,
    ) -> Result<Committed<NamespaceSummary>> {
        self.namespace_mutation(|root| Ok(root.tx.copy_namespace(&input)?))
    }
    /// Create or replace a virtual file. Supply either text or base64.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn write_file(
        &self,
        input: state_contract::file::FsWriteInput,
    ) -> Result<Committed<FileWritten>> {
        let (namespace, path) = match &input {
            state_contract::file::FsWriteInput::Text(file) => (&file.namespace, &file.path),
            state_contract::file::FsWriteInput::Binary(file) => (&file.namespace, &file.path),
        };
        self.mutation(namespace, |root| {
            let result = root.tx.write_file(&input)?;
            root.check_file_readable(namespace, path)?;
            Ok(result)
        })
    }
    /// Append text or base64-encoded bytes to a virtual file.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn append_file(
        &self,
        input: state_contract::file::FsAppendInput,
    ) -> Result<Committed<FileWritten>> {
        let (namespace, path) = match &input.0 {
            state_contract::file::FsWriteInput::Text(file) => (&file.namespace, &file.path),
            state_contract::file::FsWriteInput::Binary(file) => (&file.namespace, &file.path),
        };
        self.mutation(namespace, |root| {
            let result = root.tx.append_file(&input)?;
            root.check_file_readable(namespace, path)?;
            Ok(result)
        })
    }
    /// Move a virtual file to another path in the same namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn move_file(
        &self,
        input: state_contract::file::FsMoveInput,
    ) -> Result<Committed<FileMoved>> {
        self.mutation(&input.namespace, |root| Ok(root.tx.move_file(&input)?))
    }
    /// Copy a virtual file to another path in the same namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn copy_file(
        &self,
        input: state_contract::file::FsCopyInput,
    ) -> Result<Committed<FileMoved>> {
        self.mutation(&input.namespace, |root| Ok(root.tx.copy_file(&input)?))
    }
    /// Delete a virtual file, or delete a directory’s contents with recursive=true.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn delete_file(
        &self,
        input: state_contract::file::FsDeleteInput,
    ) -> Result<Committed<FilesDeleted>> {
        self.mutation(&input.namespace, |root| Ok(root.tx.delete_file(&input)?))
    }
    /// Create an empty named SQLite database in a namespace.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn create_database(
        &self,
        input: state_contract::database::DbCreateInput,
    ) -> Result<Committed<DatabaseCreated>> {
        self.mutation(&input.namespace, |root| {
            Ok(root.tx.create_database(&input)?)
        })
    }
    /// Drop a named SQLite database. Functions resolve database names within their namespace on each call.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn drop_database(
        &self,
        input: state_contract::database::DbDropInput,
    ) -> Result<Committed<DatabaseDropped>> {
        self.mutation(&input.namespace, |root| {
            Ok(root.tx.drop_database(&input)?)
        })
    }
    /// Run one SQL statement with optional bound parameters, including writes and RETURNING. Returns columns, rows, and rows_affected.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn execute_database(
        &self,
        input: state_contract::database::DbExecuteInput,
    ) -> Result<Committed<SqlResult>> {
        self.mutation(&input.namespace, |root| {
            Ok(root.tx.execute_database(&input)?)
        })
    }
    /// Apply an ordered batch of SQL migrations atomically. Previously applied IDs must match their stored checksums and order.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn migrate_database(
        &self,
        input: state_contract::database::DbMigrateInput,
    ) -> Result<Committed<MigrationsApplied>> {
        self.mutation(&input.namespace, |root| {
            Ok(root.tx.migrate_database(&input)?)
        })
    }
    /// Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn remove_function(
        &self,
        input: state_contract::function::FunctionRemoveInput,
    ) -> Result<Committed<FunctionRemoved>> {
        self.mutation(&input.namespace, |root| {
            Ok(root.tx.remove_function(&input)?)
        })
    }
    /// Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions can access all files, databases, and functions in their own namespace only. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn declare_function(
        &self,
        mut input: state_contract::function::FunctionDeclareInput,
    ) -> Result<Committed<FunctionPublished>> {
        self.mutation(&input.namespace.clone(), |root| {
            root.validate_function(
                &input.namespace,
                &input.file,
                &input.symbol,
                &mut input.input_schema,
                &mut input.output_schema,
            )?;
            Ok(root.tx.declare_function(&input)?)
        })
    }
    /// Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete declaration. Functions retain full access to their own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn update_function(
        &self,
        mut input: state_contract::function::FunctionUpdateInput,
    ) -> Result<Committed<FunctionPublished>> {
        self.mutation(&input.namespace.clone(), |root| {
            root.validate_function(
                &input.namespace,
                &input.file,
                &input.symbol,
                &mut input.input_schema,
                &mut input.output_schema,
            )?;
            Ok(root.tx.update_function(&input)?)
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
            root.call_function(
                &input.namespace,
                &input.function,
                input.arguments.unwrap_or_default(),
                input.expected_version.as_deref(),
            )
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
            let result = root.execute_script(
                &input.script,
                input.namespace.as_deref(),
                input.inputs.unwrap_or(Value::Null),
                &Access::Owner,
            )?;
            Ok(ExecutionResult {
                value: result.value,
                stdout: result.stdout,
            })
        })
    }
    /// Get a compact API overview, mode=runtime for a Python authoring guide, or mode=full for all schemas, or mode=readme for the bundled documentation. Select tool for one tool’s schema, or namespace and optional function for endpoint contracts.
    /// Runs in one root transaction; failures roll back all nested effects.
    pub fn describe(
        &self,
        input: state_contract::discovery::DescribeInput,
    ) -> Result<crate::Discovery> {
        self.transaction(|root| root.describe_typed(input))
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

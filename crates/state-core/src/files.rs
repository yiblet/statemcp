//! Typed files operations, shared by controllers and runtime hosts.
use crate::{Context, Error, Result};
use state_contract::file::*;

/// Files capability; transaction state belongs to the supplied context.
#[derive(Clone, Copy, Default)]
pub struct Files;
impl Files {
    /// Read raw bytes within the shared scope, without base64 or JSON conversion.
    pub fn read_bytes(&self, input: FsReadInput, context: &mut Context) -> Result<Vec<u8>> {
        context.operation(|context| {
            let namespace = context.namespace(&input.namespace)?;
            let bytes = context
                .transaction()
                .file_bytes(namespace.as_str(), &input.path)?;
            if bytes.len() > context.limits().max_result_bytes {
                return Err(Error::limit("file exceeds output byte limit"));
            }
            Ok(bytes)
        })
    }
    /// Read UTF-8 text within the shared scope; binary files return InvalidArgument.
    pub fn read_text(&self, input: FsReadInput, context: &mut Context) -> Result<String> {
        // Byte access consumes the operation budget once. Conversion errors must also poison it.
        let result = self.read_bytes(input, context).and_then(|bytes| {
            String::from_utf8(bytes).map_err(|_| Error::invalid("file is not UTF-8 text"))
        });
        context.record(result)
    }
    /// Read a virtual file as UTF-8 text or base64 for binary data.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn read_file(
        &self,
        mut input: state_contract::file::FsReadInput,
        context: &mut Context,
    ) -> Result<FileContents> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().read_file(&input)?)
        })
    }
    /// Get a virtual file’s metadata or check whether a virtual directory exists.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn stat_file(
        &self,
        mut input: state_contract::file::FsStatInput,
        context: &mut Context,
    ) -> Result<FileStat> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().stat_file(&input)?)
        })
    }
    /// List files and directories directly beneath a virtual path.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn list_files(
        &self,
        mut input: state_contract::file::FsListInput,
        context: &mut Context,
    ) -> Result<FileEntries> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().list_files(&input)?)
        })
    }
    /// Create or replace a virtual file. Supply either text or base64.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn write_file(
        &self,
        mut input: state_contract::file::FsWriteInput,
        context: &mut Context,
    ) -> Result<FileWritten> {
        context.operation(|context| {
            let namespace = match &mut input {
                FsWriteInput::Text(file) => &mut file.namespace,
                FsWriteInput::Binary(file) => &mut file.namespace,
            };
            *namespace = context.namespace(namespace)?.as_str().to_owned();
            let result = context.transaction().write_file(&input)?;
            let (namespace, path) = match input {
                FsWriteInput::Text(file) => (file.namespace, file.path),
                FsWriteInput::Binary(file) => (file.namespace, file.path),
            };
            let readable = context
                .transaction()
                .read_file(&FsReadInput { namespace, path })?;
            crate::bounded(&readable, context.limits().max_result_bytes)?;
            Ok(result)
        })
    }
    /// Append text or base64-encoded bytes to a virtual file.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn append_file(
        &self,
        mut input: state_contract::file::FsAppendInput,
        context: &mut Context,
    ) -> Result<FileWritten> {
        context.operation(|context| {
            let namespace = match &mut input.0 {
                FsWriteInput::Text(file) => &mut file.namespace,
                FsWriteInput::Binary(file) => &mut file.namespace,
            };
            *namespace = context.namespace(namespace)?.as_str().to_owned();
            let result = context.transaction().append_file(&input)?;
            let (namespace, path) = match input.0 {
                FsWriteInput::Text(file) => (file.namespace, file.path),
                FsWriteInput::Binary(file) => (file.namespace, file.path),
            };
            let readable = context
                .transaction()
                .read_file(&FsReadInput { namespace, path })?;
            crate::bounded(&readable, context.limits().max_result_bytes)?;
            Ok(result)
        })
    }
    /// Move a virtual file to another path in the same namespace.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn move_file(
        &self,
        mut input: state_contract::file::FsMoveInput,
        context: &mut Context,
    ) -> Result<FileMoved> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().move_file(&input)?)
        })
    }
    /// Copy a virtual file to another path in the same namespace.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn copy_file(
        &self,
        mut input: state_contract::file::FsCopyInput,
        context: &mut Context,
    ) -> Result<FileMoved> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().copy_file(&input)?)
        })
    }
    /// Delete a virtual file, or delete a directory’s contents with recursive=true.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn delete_file(
        &self,
        mut input: state_contract::file::FsDeleteInput,
        context: &mut Context,
    ) -> Result<FilesDeleted> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().delete_file(&input)?)
        })
    }
    pub(crate) fn source(
        &self,
        namespace: &str,
        file: &str,
        context: &mut Context,
    ) -> Result<String> {
        let id = context.namespace(namespace)?;
        context
            .transaction()
            .file_text(id.as_str(), file)
            .map_err(Into::into)
    }
    pub(crate) fn modules(
        &self,
        namespace: &str,
        context: &mut Context,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let id = context.namespace(namespace)?;
        let max = context.runtime_limits()?.max_source_bytes;
        context
            .transaction()
            .python_sources(id.as_str(), max)
            .map_err(Into::into)
    }
}

impl crate::SourceProvider for Files {
    fn snapshot(
        &self,
        namespace: &str,
        file: &str,
        max_source_bytes: usize,
        context: &mut Context,
    ) -> Result<state_contract::function::FunctionSources> {
        let namespace = context.namespace(namespace)?;
        let source = self.source(namespace.as_str(), file, context)?;
        let modules = context
            .transaction()
            .python_sources(namespace.as_str(), max_source_bytes.min(1024 * 1024))?;
        Ok(state_contract::function::FunctionSources { source, modules })
    }
}

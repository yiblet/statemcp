//! Direct typed domain operations.
use super::Transaction;
use crate::{
    Error, FunctionDeclaration, Result,
    identity::{hash, id},
    inputs::*,
    model::Database,
    records::*,
    sql,
    validation::{name, virtual_path},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::{BTreeMap, BTreeSet};

impl Transaction {
    /// List namespaces with their names, descriptions, stable IDs, revisions, and resource counts.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn list_namespaces(&mut self) -> Result<Namespaces> {
        self.operation(|tx| tx.list_namespaces_inner())
    }
    /// Get a namespace’s name, description, stable ID, revision, and resource counts.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn get_namespace(&mut self, input: &NamespaceGetInput) -> Result<NamespaceInfo> {
        self.operation(|tx| tx.get_namespace_inner(input))
    }
    /// Create an empty namespace with an optional description for files, SQLite databases, and published functions.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn create_namespace(&mut self, input: &NamespaceCreateInput) -> Result<NamespaceSummary> {
        self.operation(|tx| tx.create_namespace_inner(input))
    }
    /// Update a namespace name or description while preserving its stable ID and contents. Omitted description is preserved; null clears it.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn update_namespace(&mut self, input: &NamespaceUpdateInput) -> Result<NamespaceSummary> {
        self.operation(|tx| tx.update_namespace_inner(input))
    }
    /// Copy a namespace and its contents to a new name. Subsequent changes are independent.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn copy_namespace(&mut self, input: &NamespaceCopyInput) -> Result<NamespaceSummary> {
        self.operation(|tx| tx.copy_namespace_inner(input))
    }
    /// Delete a namespace and its contents from active use. Its name remains reserved.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn delete_namespace(&mut self, input: &NamespaceDeleteInput) -> Result<NamespaceDeleted> {
        self.operation(|tx| tx.delete_namespace_inner(input))
    }
    /// Read a virtual file as UTF-8 text or base64 for binary data.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn read_file(&mut self, input: &FsReadInput) -> Result<FileContents> {
        self.operation(|tx| tx.read_file_inner(input))
    }
    /// Get a virtual file’s metadata or check whether a virtual directory exists.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn stat_file(&mut self, input: &FsStatInput) -> Result<FileStat> {
        self.operation(|tx| tx.stat_file_inner(input))
    }
    /// List files and directories directly beneath a virtual path.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn list_files(&mut self, input: &FsListInput) -> Result<FileEntries> {
        self.operation(|tx| tx.list_files_inner(input))
    }
    /// Create or replace a virtual file. Supply either text or base64.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn write_file(&mut self, input: &FsWriteInput) -> Result<FileWritten> {
        self.operation(|tx| tx.write_file_inner(input))
    }
    /// Append text or base64-encoded bytes to a virtual file.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn append_file(&mut self, input: &FsAppendInput) -> Result<FileWritten> {
        self.operation(|tx| tx.append_file_inner(input))
    }
    /// Delete a virtual file, or delete a directory’s contents with recursive=true.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn delete_file(&mut self, input: &FsDeleteInput) -> Result<FilesDeleted> {
        self.operation(|tx| tx.delete_file_inner(input))
    }
    /// Move a virtual file to another path in the same namespace.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn move_file(&mut self, input: &FsMoveInput) -> Result<FileMoved> {
        self.operation(|tx| tx.move_file_inner(input))
    }
    /// Copy a virtual file to another path in the same namespace.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn copy_file(&mut self, input: &FsCopyInput) -> Result<FileMoved> {
        self.operation(|tx| tx.copy_file_inner(input))
    }
    /// List the named SQLite databases in a namespace.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn list_databases(&mut self, input: &DbListInput) -> Result<Databases> {
        self.operation(|tx| tx.list_databases_inner(input))
    }
    /// Create an empty named SQLite database in a namespace.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn create_database(&mut self, input: &DbCreateInput) -> Result<DatabaseCreated> {
        self.operation(|tx| tx.create_database_inner(input))
    }
    /// Drop a named SQLite database. Functions resolve database names within their namespace on each call.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn drop_database(&mut self, input: &DbDropInput) -> Result<DatabaseDropped> {
        self.operation(|tx| tx.drop_database_inner(input))
    }
    /// Run one read-only SQL statement with optional bound parameters. Returns columns, rows, and rows_affected.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn query_database(&mut self, input: &DbQueryInput) -> Result<SqlResult> {
        self.operation(|tx| tx.query_database_inner(input))
    }
    /// Inspect a database’s tables, columns, indexes, foreign keys, schema, migration history, and storage metadata.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn inspect_database(&mut self, input: &DbInspectInput) -> Result<DatabaseInspection> {
        self.operation(|tx| tx.inspect_database_inner(input))
    }
    /// Run one SQL statement with optional bound parameters, including writes and RETURNING. Returns columns, rows, and rows_affected.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn execute_database(&mut self, input: &DbExecuteInput) -> Result<SqlResult> {
        self.operation(|tx| tx.execute_database_inner(input))
    }
    /// Get the ordered IDs and checksums of applied database migrations.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn database_migrations(&mut self, input: &DbMigrationsInput) -> Result<Migrations> {
        self.operation(|tx| tx.database_migrations_inner(input))
    }
    /// Apply an ordered batch of SQL migrations atomically. Previously applied IDs must match their stored checksums and order.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn migrate_database(&mut self, input: &DbMigrateInput) -> Result<MigrationsApplied> {
        self.operation(|tx| tx.migrate_database_inner(input))
    }
    /// List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts, without source code. Use function.get with namespace and name for a full declaration.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn list_functions(&mut self, input: &FunctionListInput) -> Result<Functions> {
        self.operation(|tx| tx.list_functions_inner(input))
    }
    /// Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn get_function(&mut self, input: &FunctionGetInput) -> Result<FunctionDeclaration> {
        self.operation(|tx| tx.get_function_inner(input))
    }
    /// Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn remove_function(&mut self, input: &FunctionRemoveInput) -> Result<FunctionRemoved> {
        self.operation(|tx| tx.remove_function_inner(input))
    }
    /// Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions can access all files, databases, and functions in their own namespace only. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn declare_function(&mut self, input: &FunctionDeclareInput) -> Result<FunctionPublished> {
        self.operation(|tx| tx.declare_function_inner(input))
    }
    /// Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete declaration. Functions retain full access to their own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version.
    /// Effects are staged; an error aborts this transaction. Publish with `commit`.
    pub fn update_function(&mut self, input: &FunctionUpdateInput) -> Result<FunctionPublished> {
        self.operation(|tx| tx.update_function_inner(input))
    }

    pub(super) fn staged_namespace(&self, key: &str) -> Result<&crate::model::Namespace> {
        self.namespaces.get(key).ok_or_else(|| {
            Error::new(
                crate::ErrorCode::CorruptStore,
                "namespace is missing from the transaction",
            )
        })
    }
    fn staged_namespace_mut(&mut self, key: &str) -> Result<&mut crate::model::Namespace> {
        self.namespaces.get_mut(key).ok_or_else(|| {
            Error::new(
                crate::ErrorCode::CorruptStore,
                "namespace is missing from the transaction",
            )
        })
    }
    fn namespace_summary(ns: &crate::model::Namespace) -> NamespaceSummary {
        NamespaceSummary {
            id: ns.id.clone(),
            name: ns.name.clone(),
            description: ns.manifest.description.clone(),
            files: ns.manifest.files.len(),
            databases: ns.manifest.databases.len(),
            functions: ns.manifest.functions.len(),
        }
    }
    fn namespace_record(ns: &crate::model::Namespace) -> NamespaceInfo {
        NamespaceInfo {
            summary: Self::namespace_summary(ns),
            revision: ns.revision.clone(),
        }
    }
    fn namespace_for_change(&self, selector: &str, revision: Option<&str>) -> Result<String> {
        let key = self.selected(selector)?;
        self.check_revision(&key, revision)?;
        Ok(key)
    }
    fn list_namespaces_inner(&self) -> Result<Namespaces> {
        Ok(Namespaces {
            namespaces: self
                .namespaces
                .values()
                .filter(|ns| !ns.deleted)
                .map(Self::namespace_record)
                .collect(),
        })
    }
    fn get_namespace_inner(&self, input: &NamespaceGetInput) -> Result<NamespaceInfo> {
        Ok(Self::namespace_record(self.staged_namespace(
            self.namespace_identity(&input.namespace)?,
        )?))
    }
    fn create_namespace_inner(&mut self, input: &NamespaceCreateInput) -> Result<NamespaceSummary> {
        let name = name(&input.name)?;
        self.available(&name)?;
        let mut manifest = crate::model::Manifest::default();
        input.description.apply(&mut manifest.description);
        let key = id();
        self.namespaces.insert(
            key.clone(),
            crate::model::Namespace {
                id: key.clone(),
                name,
                revision: String::new(),
                deleted: false,
                manifest,
                dirty: true,
            },
        );
        Ok(Self::namespace_summary(self.staged_namespace(&key)?))
    }
    fn update_namespace_inner(&mut self, input: &NamespaceUpdateInput) -> Result<NamespaceSummary> {
        let key =
            self.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
        let new_name = input.name.as_deref().map(name).transpose()?;
        if let Some(name) = &new_name
            && *name != self.staged_namespace(&key)?.name
        {
            self.available(name)?;
        }
        let ns = self.staged_namespace_mut(&key)?;
        if let Some(name) = new_name {
            ns.name = name;
        }
        input.description.apply(&mut ns.manifest.description);
        ns.dirty = true;
        Ok(Self::namespace_summary(self.staged_namespace(&key)?))
    }
    fn copy_namespace_inner(&mut self, input: &NamespaceCopyInput) -> Result<NamespaceSummary> {
        let key =
            self.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
        let new_name = name(&input.name)?;
        self.available(&new_name)?;
        let mut ns = self.staged_namespace(&key)?.clone();
        self.freeze_databases(&key, &mut ns.manifest)?;
        ns.id = id();
        ns.name = new_name;
        input.description.apply(&mut ns.manifest.description);
        ns.dirty = true;
        let copy_key = ns.id.clone();
        self.namespaces.insert(copy_key.clone(), ns);
        Ok(Self::namespace_summary(self.staged_namespace(&copy_key)?))
    }
    fn delete_namespace_inner(&mut self, input: &NamespaceDeleteInput) -> Result<NamespaceDeleted> {
        let key =
            self.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
        self.staged_namespace_mut(&key)?.deleted = true;
        self.staged_namespace_mut(&key)?.dirty = true;
        Ok(NamespaceDeleted {
            deleted: true,
            id: key,
        })
    }
    fn read_file_inner(&self, input: &FsReadInput) -> Result<FileContents> {
        let key = self.namespace_identity(&input.namespace)?;
        let path = virtual_path(&input.path)?;
        let hash = self
            .staged_namespace(key)?
            .manifest
            .files
            .get(&path)
            .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "file does not exist"))?;
        let bytes = self.bytes(hash)?;
        let size = bytes.len();
        match String::from_utf8(bytes) {
            Ok(text) => Ok(FileContents::Text {
                path,
                kind: FileKind::File,
                size,
                text,
            }),
            Err(error) => Ok(FileContents::Binary {
                path,
                kind: FileKind::File,
                size,
                base64: STANDARD.encode(error.into_bytes()),
            }),
        }
    }
    fn stat_file_inner(&self, input: &FsStatInput) -> Result<FileStat> {
        let key = self.namespace_identity(&input.namespace)?;
        let path = virtual_path(&input.path)?;
        let files = &self.staged_namespace(key)?.manifest.files;
        if let Some(hash) = files.get(&path) {
            Ok(FileStat::File {
                path,
                kind: FileKind::File,
                size: self.bytes(hash)?.len(),
                hash: hash.clone(),
            })
        } else if path == "/"
            || files
                .keys()
                .any(|file| file.starts_with(&format!("{path}/")))
        {
            Ok(FileStat::Directory {
                path,
                kind: FileKind::Directory,
            })
        } else {
            Err(Error::new(
                crate::ErrorCode::NotFound,
                "file does not exist",
            ))
        }
    }
    fn list_files_inner(&self, input: &FsListInput) -> Result<FileEntries> {
        let key = self.namespace_identity(&input.namespace)?;
        let path = virtual_path(input.path.as_deref().unwrap_or("/"))?;
        let prefix = if path == "/" {
            "/".into()
        } else {
            format!("{path}/")
        };
        let mut entries = BTreeMap::new();
        for file in self.staged_namespace(key)?.manifest.files.keys() {
            if let Some(tail) = file.strip_prefix(&prefix) {
                let full = format!(
                    "{prefix}{}",
                    tail.split_once('/').map_or(tail, |(first, _)| first)
                );
                entries.entry(full.clone()).or_insert(FileEntry {
                    path: full,
                    kind: if tail.contains('/') {
                        FileKind::Directory
                    } else {
                        FileKind::File
                    },
                });
            }
        }
        if path != "/" && entries.is_empty() {
            return Err(Error::new(
                crate::ErrorCode::NotFound,
                "directory does not exist",
            ));
        }
        Ok(FileEntries {
            entries: entries.into_values().collect(),
        })
    }
    fn write_file_inner(&mut self, input: &FsWriteInput) -> Result<FileWritten> {
        self.save_file(input, false)
    }
    fn append_file_inner(&mut self, input: &FsAppendInput) -> Result<FileWritten> {
        self.save_file(&input.0, true)
    }
    fn save_file(&mut self, input: &FsWriteInput, append: bool) -> Result<FileWritten> {
        let (namespace, source_path, revision) = match input {
            FsWriteInput::Text(input) => (
                &input.namespace,
                &input.path,
                input.expected_revision.as_deref(),
            ),
            FsWriteInput::Binary(input) => (
                &input.namespace,
                &input.path,
                input.expected_revision.as_deref(),
            ),
        };
        let key = self.namespace_for_change(namespace, revision)?;
        let path = virtual_path(source_path)?;
        let files = &self.staged_namespace(&key)?.manifest.files;
        if path == "/"
            || files.keys().any(|file| {
                file.starts_with(&format!("{path}/")) || path.starts_with(&format!("{file}/"))
            })
        {
            return Err(Error::invalid("file collides with a directory"));
        }
        let mut bytes = if append {
            files
                .get(&path)
                .map(|hash| self.bytes(hash))
                .transpose()?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        match input {
            FsWriteInput::Text(input) => bytes.extend_from_slice(input.text.as_bytes()),
            FsWriteInput::Binary(input) => bytes.extend(
                STANDARD
                    .decode(&input.base64)
                    .map_err(|_| Error::invalid("invalid base64"))?,
            ),
        }
        let size = bytes.len();
        let hash = self.put_bytes(bytes)?;
        let ns = self.staged_namespace_mut(&key)?;
        ns.manifest.files.insert(path.clone(), hash);
        ns.dirty = true;
        Ok(FileWritten { path, size })
    }
    fn delete_file_inner(&mut self, input: &FsDeleteInput) -> Result<FilesDeleted> {
        let key =
            self.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
        let path = virtual_path(&input.path)?;
        let files = &self.staged_namespace(&key)?.manifest.files;
        let removed: Vec<_> = if files.contains_key(&path) {
            vec![path]
        } else if input.recursive.unwrap_or(false) {
            let prefix = if path == "/" {
                "/".into()
            } else {
                format!("{path}/")
            };
            files
                .keys()
                .filter(|file| file.starts_with(&prefix))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        if removed.is_empty() {
            return Err(Error::new(
                crate::ErrorCode::NotFound,
                "file does not exist (directory deletion requires recursive=true)",
            ));
        }
        let ns = self.staged_namespace_mut(&key)?;
        for path in &removed {
            ns.manifest.files.remove(path);
        }
        ns.dirty = true;
        Ok(FilesDeleted {
            deleted: removed.len(),
        })
    }
    fn move_file_inner(&mut self, input: &FsMoveInput) -> Result<FileMoved> {
        self.relocate_file(
            &input.namespace,
            &input.path,
            &input.destination,
            input.expected_revision.as_deref(),
            true,
        )
    }
    fn copy_file_inner(&mut self, input: &FsCopyInput) -> Result<FileMoved> {
        self.relocate_file(
            &input.namespace,
            &input.path,
            &input.destination,
            input.expected_revision.as_deref(),
            false,
        )
    }
    fn relocate_file(
        &mut self,
        namespace: &str,
        path: &str,
        destination: &str,
        revision: Option<&str>,
        moving: bool,
    ) -> Result<FileMoved> {
        let key = self.namespace_for_change(namespace, revision)?;
        let path = virtual_path(path)?;
        let destination = virtual_path(destination)?;
        let files = &self.staged_namespace(&key)?.manifest.files;
        let hash = files
            .get(&path)
            .cloned()
            .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "file does not exist"))?;
        if destination == "/"
            || files.contains_key(&destination)
            || files.keys().any(|file| {
                file.starts_with(&format!("{destination}/"))
                    || destination.starts_with(&format!("{file}/"))
            })
        {
            return Err(Error::new(
                crate::ErrorCode::AlreadyExists,
                "destination exists or collides with directory",
            ));
        }
        let ns = self.staged_namespace_mut(&key)?;
        ns.manifest.files.insert(destination.clone(), hash);
        if moving {
            ns.manifest.files.remove(&path);
        }
        ns.dirty = true;
        Ok(FileMoved { path: destination })
    }
    fn selected_database(
        &self,
        namespace: &str,
        database: &str,
        revision: Option<&str>,
    ) -> Result<(String, String, Database)> {
        let key = self.namespace_for_change(namespace, revision)?;
        let database = name(database)?;
        let record = self
            .staged_namespace(&key)?
            .manifest
            .databases
            .get(&database)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    crate::ErrorCode::NotFound,
                    format!("database {database} does not exist"),
                )
            })?;
        Ok((key, database, record))
    }
    fn list_databases_inner(&self, input: &DbListInput) -> Result<Databases> {
        let key = self.namespace_identity(&input.namespace)?;
        Ok(Databases {
            databases: self
                .staged_namespace(key)?
                .manifest
                .databases
                .iter()
                .map(|(name, db)| DatabaseInfo {
                    name: name.clone(),
                    id: db.id.clone(),
                    migrations: db.migrations.clone(),
                })
                .collect(),
        })
    }
    fn create_database_inner(&mut self, input: &DbCreateInput) -> Result<DatabaseCreated> {
        let key =
            self.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
        let name = name(&input.database)?;
        if self
            .staged_namespace(&key)?
            .manifest
            .databases
            .contains_key(&name)
        {
            return Err(Error::new(
                crate::ErrorCode::AlreadyExists,
                "database already exists",
            ));
        }
        let path = self.working(None)?;
        let conn = sql::open(&path, true)?;
        conn.execute_batch(
            "CREATE TABLE _state_initialization(x); DROP TABLE _state_initialization;",
        )?;
        conn.close().map_err(|(_, error)| error)?;
        let db = Database {
            id: id(),
            snapshot: self.seal(&path)?,
            migrations: Vec::new(),
        };
        let ns = self.staged_namespace_mut(&key)?;
        ns.manifest.databases.insert(name.clone(), db);
        ns.dirty = true;
        Ok(DatabaseCreated {
            name,
            created: true,
        })
    }
    fn drop_database_inner(&mut self, input: &DbDropInput) -> Result<DatabaseDropped> {
        let (key, name, _db) = self.selected_database(
            &input.namespace,
            &input.database,
            input.expected_revision.as_deref(),
        )?;
        self.working_databases.remove(&(key.clone(), name.clone()));
        let ns = self.staged_namespace_mut(&key)?;
        ns.manifest.databases.remove(&name);
        ns.dirty = true;
        Ok(DatabaseDropped { dropped: true })
    }
    fn read_database<T>(
        &self,
        namespace: &str,
        database: &str,
        action: impl FnOnce(&rusqlite::Connection, &Database, bool, String) -> Result<T>,
    ) -> Result<T> {
        let (key, name, db) = self.selected_database(namespace, database, None)?;
        if let Some(working) = self.working_databases.get(&(key, name.clone())) {
            action(&working.connection, &db, true, name)
        } else {
            let conn = sql::open(&self.snapshot_path(&db.snapshot), false)?;
            action(&conn, &db, false, name)
        }
    }
    fn query_database_inner(&self, input: &DbQueryInput) -> Result<SqlResult> {
        self.read_database(&input.namespace, &input.database, |conn, _, _, _| {
            sql::run_typed(
                conn,
                &input.sql,
                input.params.as_deref().unwrap_or(&[]),
                true,
            )
        })
    }
    fn inspect_database_inner(&self, input: &DbInspectInput) -> Result<DatabaseInspection> {
        self.read_database(
            &input.namespace,
            &input.database,
            |conn, db, staged, name| {
                Ok(DatabaseInspection {
                    name,
                    id: db.id.clone(),
                    snapshot: db.snapshot.clone(),
                    staged,
                    migrations: db.migrations.clone(),
                    schema: sql::inspect_typed(conn)?,
                })
            },
        )
    }
    fn execute_database_inner(&mut self, input: &DbExecuteInput) -> Result<SqlResult> {
        let (key, name, db) = self.selected_database(
            &input.namespace,
            &input.database,
            input.expected_revision.as_deref(),
        )?;
        let conn = self.ensure_working(&key, &name, &db)?;
        let result = sql::run_typed(
            conn,
            &input.sql,
            input.params.as_deref().unwrap_or(&[]),
            false,
        )?;
        self.staged_namespace_mut(&key)?.dirty = true;
        Ok(result)
    }
    fn database_migrations_inner(&self, input: &DbMigrationsInput) -> Result<Migrations> {
        let (_, _, db) = self.selected_database(&input.namespace, &input.database, None)?;
        Ok(Migrations {
            migrations: db.migrations,
        })
    }
    fn migrate_database_inner(&mut self, input: &DbMigrateInput) -> Result<MigrationsApplied> {
        let (key, name, mut db) = self.selected_database(
            &input.namespace,
            &input.database,
            input.expected_revision.as_deref(),
        )?;
        let mut seen = BTreeSet::new();
        let mut pending = Vec::new();
        let mut prefix_position = 0;
        let prefix_mode = input.migrations.first().is_some_and(|first| {
            db.migrations
                .iter()
                .any(|migration| migration.id == first.id)
        });
        for migration in &input.migrations {
            let migration_id = crate::validation::name(&migration.id)?;
            if !seen.insert(migration_id.clone()) {
                return Err(Error::new(
                    crate::ErrorCode::MigrationMismatch,
                    "duplicate migration ID",
                ));
            }
            let checksum = hash(migration.sql.as_bytes());
            if let Some(old) = db.migrations.iter().find(|old| old.id == migration_id) {
                if !prefix_mode
                    || prefix_position >= db.migrations.len()
                    || db.migrations[prefix_position].id != migration_id
                    || old.checksum != checksum
                {
                    return Err(Error::new(
                        crate::ErrorCode::MigrationMismatch,
                        "migration history or checksum differs",
                    ));
                }
                prefix_position += 1;
                continue;
            }
            if prefix_mode && prefix_position < db.migrations.len() {
                return Err(Error::new(
                    crate::ErrorCode::MigrationMismatch,
                    "migration list skipped applied entries",
                ));
            }
            pending.push(migration.sql.as_str());
            db.migrations.push(Migration {
                id: migration_id,
                checksum,
            });
            prefix_position = db.migrations.len();
        }
        if !pending.is_empty() {
            let conn = self.ensure_working(&key, &name, &db)?;
            for sql in &pending {
                sql::batch(conn, sql)?;
            }
            let ns = self.staged_namespace_mut(&key)?;
            ns.manifest.databases.insert(name, db.clone());
            ns.dirty = true;
        }
        Ok(MigrationsApplied {
            applied: pending.len(),
            migrations: db.migrations,
        })
    }
    fn list_functions_inner(&self, input: &FunctionListInput) -> Result<Functions> {
        Ok(Functions {
            functions: self
                .function_declarations(&input.namespace)?
                .map(|record| FunctionSummary {
                    name: record.name.clone(),
                    file: record.file.clone(),
                    symbol: record.symbol.clone(),
                    description: record.description.clone(),
                    input_schema: record.input_schema.clone(),
                    output_schema: record.output_schema.clone(),
                    version: record.version.clone(),
                })
                .collect(),
        })
    }
    fn get_function_inner(&self, input: &FunctionGetInput) -> Result<FunctionDeclaration> {
        self.function_declaration(
            &input.namespace,
            &input.name,
            input.expected_version.as_deref(),
        )
        .cloned()
    }
    fn remove_function_inner(&mut self, input: &FunctionRemoveInput) -> Result<FunctionRemoved> {
        let key =
            self.namespace_for_change(&input.namespace, input.expected_revision.as_deref())?;
        self.function_declaration(&key, &input.name, input.expected_version.as_deref())?;
        let name = name(&input.name)?;
        let ns = self.staged_namespace_mut(&key)?;
        ns.manifest.functions.remove(&name);
        ns.dirty = true;
        Ok(FunctionRemoved { removed: true })
    }
    fn declare_function_inner(
        &mut self,
        input: &FunctionDeclareInput,
    ) -> Result<FunctionPublished> {
        self.publish_function(
            &input.namespace,
            &input.name,
            &input.file,
            &input.symbol,
            input.description.as_deref(),
            input.input_schema.as_ref(),
            input.output_schema.as_ref(),
            input.expected_revision.as_deref(),
            input.expected_version.as_deref(),
        )
    }
    fn update_function_inner(&mut self, input: &FunctionUpdateInput) -> Result<FunctionPublished> {
        self.publish_function(
            &input.namespace,
            &input.name,
            &input.file,
            &input.symbol,
            input.description.as_deref(),
            input.input_schema.as_ref(),
            input.output_schema.as_ref(),
            input.expected_revision.as_deref(),
            input.expected_version.as_deref(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn publish_function(
        &mut self,
        namespace: &str,
        function_name: &str,
        file: &str,
        symbol: &str,
        description: Option<&str>,
        input_schema: Option<&EndpointSchema>,
        output_schema: Option<&EndpointSchema>,
        revision: Option<&str>,
        version: Option<&str>,
    ) -> Result<FunctionPublished> {
        let key = self.namespace_for_change(namespace, revision)?;
        let function_name = name(function_name)?;
        let existing = self
            .staged_namespace(&key)?
            .manifest
            .functions
            .get(&function_name);
        if let Some(version) = version
            && existing.map(|record| record.version.as_str()) != Some(version)
        {
            return Err(Error::new(
                crate::ErrorCode::Conflict,
                "function version does not match",
            ));
        }
        let file = virtual_path(file)?;
        let source_hash = self
            .staged_namespace(&key)?
            .manifest
            .files
            .get(&file)
            .cloned()
            .ok_or_else(|| Error::new(crate::ErrorCode::NotFound, "source file does not exist"))?;
        let source = String::from_utf8(self.bytes(&source_hash)?)
            .map_err(|_| Error::invalid("function source must be UTF-8"))?;
        if symbol.is_empty() {
            return Err(Error::invalid("symbol must be a nonempty string"));
        }
        let mut record = FunctionDeclaration {
            name: function_name.clone(),
            file,
            symbol: symbol.to_owned(),
            description: description.map(str::to_owned),
            input_schema: input_schema.cloned(),
            output_schema: output_schema.cloned(),
            modules: self.python_sources(&key, 1024 * 1024)?,
            source,
            source_hash,
            abi_version: 1,
            version: String::new(),
        };
        // Keep the established canonical metadata hash for pinned declarations.
        record.version = hash(&serde_json::to_vec(&record)?);
        let result = FunctionPublished {
            name: function_name.clone(),
            version: record.version.clone(),
            published: true,
        };
        let ns = self.staged_namespace_mut(&key)?;
        ns.manifest.functions.insert(function_name, record);
        ns.dirty = true;
        Ok(result)
    }
}

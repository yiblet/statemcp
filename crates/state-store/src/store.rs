use crate::sql;
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
}
impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_ARGUMENT", message)
    }
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::new("LIMIT_EXCEEDED", message)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new("IO_ERROR", e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::invalid(e.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        let code = match e.sqlite_error_code() {
            Some(
                rusqlite::ErrorCode::AuthorizationForStatementDenied
                | rusqlite::ErrorCode::PermissionDenied,
            ) => "PERMISSION_DENIED",
            Some(
                rusqlite::ErrorCode::OperationInterrupted
                | rusqlite::ErrorCode::TooBig
                | rusqlite::ErrorCode::DiskFull
                | rusqlite::ErrorCode::OutOfMemory,
            ) => "LIMIT_EXCEEDED",
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                "CONFLICT"
            }
            _ => "SQL_ERROR",
        };
        Self::new(code, e.to_string())
    }
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn required<'a>(args: &'a Value, field: &str) -> Result<&'a str> {
    args.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("{field} must be a string")))
}
fn name(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(Error::invalid(
            "name must contain 1–128 bytes without slash or control characters",
        ));
    }
    Ok(value.to_string())
}
fn virtual_path(value: &str) -> Result<String> {
    if !value.starts_with('/')
        || value.len() > 4096
        || value.chars().any(|c| c == '\0' || c == '\\')
    {
        return Err(Error::invalid(
            "path must be an absolute virtual POSIX path",
        ));
    }
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(Error::invalid("parent traversal is forbidden")),
            _ => parts.push(part),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}
fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[derive(Clone)]
pub struct Store {
    root: Arc<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub request_hash: String,
    pub result: Value,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    files: BTreeMap<String, String>,
    #[serde(default)]
    databases: BTreeMap<String, Database>,
    #[serde(default)]
    functions: BTreeMap<String, Value>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Database {
    id: String,
    snapshot: String,
    #[serde(default)]
    migrations: Vec<Migration>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Migration {
    id: String,
    checksum: String,
}
#[derive(Clone)]
struct Namespace {
    id: String,
    name: String,
    revision: String,
    deleted: bool,
    manifest: Manifest,
    dirty: bool,
}

/// A consistent root invocation. Any failed dispatch poisons publication.
/// Dropping it discards all logical changes; unreachable sealed files are retained.
struct WorkingDatabase {
    connection: Connection,
    path: PathBuf,
}

pub struct Transaction {
    store: Store,
    generation: i64,
    namespaces: BTreeMap<String, Namespace>,
    objects: BTreeMap<String, Vec<u8>>,
    staging: PathBuf,
    poisoned: bool,
    working_databases: BTreeMap<(String, String), WorkingDatabase>,
    receipt: Option<(String, String, Receipt)>,
}
impl Drop for Transaction {
    fn drop(&mut self) {
        self.working_databases.clear();
        let _ = fs::remove_dir_all(&self.staging);
    }
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;
        let root = fs::canonicalize(path.as_ref())?;
        fs::create_dir_all(root.join("snapshots"))?;
        fs::create_dir_all(root.join("staging"))?;
        // Verify durable directory flush support before accepting writes.
        sync_dir(&root)?;
        let store = Self {
            root: Arc::new(root),
        };
        let conn = store.catalog()?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != 0 && version != 1 {
            return Err(Error::new(
                "UNSUPPORTED_FEATURE",
                format!("unsupported catalog format {version}"),
            ));
        }
        conn.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS catalog_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1), generation INTEGER NOT NULL);
            INSERT OR IGNORE INTO catalog_state VALUES(1,0);
            CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY, bytes BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS revisions(id TEXT PRIMARY KEY, parent_id TEXT, manifest TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS namespaces(id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, head_revision TEXT NOT NULL REFERENCES revisions(id), deleted INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS receipts(principal TEXT NOT NULL, key TEXT NOT NULL, request_hash TEXT NOT NULL, result_json TEXT NOT NULL, PRIMARY KEY(principal,key));
            PRAGMA user_version=1; COMMIT;")?;
        sync_dir(&store.root)?;
        Ok(store)
    }
    fn catalog(&self) -> Result<Connection> {
        let conn = Connection::open(self.root.join("catalog.sqlite"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        Ok(conn)
    }
    pub fn begin(&self) -> Result<Transaction> {
        let mut conn = self.catalog()?;
        let tx = conn.transaction()?;
        let generation = tx.query_row(
            "SELECT generation FROM catalog_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let namespaces = {
            let mut stmt=tx.prepare("SELECT n.id,n.name,n.head_revision,n.deleted,r.manifest FROM namespaces n JOIN revisions r ON r.id=n.head_revision")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?;
            let mut namespaces = BTreeMap::new();
            for row in rows {
                let (id, name, revision, deleted, manifest) = row?;
                namespaces.insert(
                    id.clone(),
                    Namespace {
                        id,
                        name,
                        revision,
                        deleted,
                        manifest: serde_json::from_str(&manifest)?,
                        dirty: false,
                    },
                );
            }
            namespaces
        };
        tx.commit()?;
        let staging = self.root.join("staging").join(id());
        fs::create_dir(&staging)?;
        Ok(Transaction {
            store: self.clone(),
            generation,
            namespaces,
            objects: BTreeMap::new(),
            staging,
            poisoned: false,
            working_databases: BTreeMap::new(),
            receipt: None,
        })
    }
    pub fn receipt(&self, principal: &str, key: &str) -> Result<Option<Receipt>> {
        receipt(&self.catalog()?, principal, key)
    }
}
fn receipt(conn: &Connection, principal: &str, key: &str) -> Result<Option<Receipt>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT request_hash,result_json FROM receipts WHERE principal=? AND key=?",
            params![principal, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(request_hash, result)| {
        Ok(Receipt {
            request_hash,
            result: serde_json::from_str(&result)?,
        })
    })
    .transpose()
}
impl Transaction {
    pub fn set_receipt(
        &mut self,
        principal: &str,
        key: &str,
        request_hash: &str,
        result: Value,
    ) -> Result<()> {
        if principal.is_empty() || key.is_empty() || key.len() > 256 {
            self.poisoned = true;
            return Err(Error::invalid(
                "receipt principal/key must be nonempty; key is limited to 256 bytes",
            ));
        }
        if serde_json::to_vec(&result)?.len() > sql::MAX_RESULT {
            self.poisoned = true;
            return Err(Error::limit("receipt exceeds 1 MiB"));
        }
        self.receipt = Some((
            principal.into(),
            key.into(),
            Receipt {
                request_hash: request_hash.into(),
                result,
            },
        ));
        Ok(())
    }
    pub fn dispatch(&mut self, tool: &str, args: Value) -> Result<Value> {
        if self.poisoned {
            return Err(Error::new(
                "TRANSACTION_ABORTED",
                "a prior operation failed",
            ));
        }
        let result = match tool {
            "state_namespace" => self.namespace(&args),
            "state_fs" => self.file(&args),
            "state_db" => self.database(&args),
            "state_function" => self.function(&args),
            _ => Err(Error::new(
                "UNSUPPORTED_FEATURE",
                format!("storage does not handle {tool}"),
            )),
        };
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn namespace_id(&self, selector: &str) -> Result<String> {
        self.namespaces
            .get(selector)
            .filter(|n| !n.deleted)
            .or_else(|| {
                self.namespaces
                    .values()
                    .find(|n| !n.deleted && n.name == selector)
            })
            .map(|n| n.id.clone())
            .ok_or_else(|| Error::new("NOT_FOUND", format!("namespace {selector} does not exist")))
    }
    fn selected(&self, args: &Value) -> Result<String> {
        self.namespace_id(required(args, "namespace")?)
    }
    fn check_revision(&self, ns: &str, args: &Value) -> Result<()> {
        if let Some(expected) = args.get("expected_revision")
            && expected.as_str() != Some(&self.namespaces[ns].revision)
        {
            return Err(Error::new("CONFLICT", "namespace revision does not match"));
        }
        Ok(())
    }
    fn namespace_info(ns: &Namespace) -> Value {
        json!({"id":ns.id,"name":ns.name,"revision":ns.revision,"files":ns.manifest.files.len(),"databases":ns.manifest.databases.len(),"functions":ns.manifest.functions.len()})
    }
    fn namespace(&mut self, args: &Value) -> Result<Value> {
        match required(args, "action")? {
            "list" => Ok(
                json!({"namespaces":self.namespaces.values().filter(|n|!n.deleted).map(Self::namespace_info).collect::<Vec<_>>()}),
            ),
            "create" => {
                let name = name(required(args, "name")?)?;
                self.available(&name)?;
                let ns = Namespace {
                    id: id(),
                    name,
                    revision: String::new(),
                    deleted: false,
                    manifest: Manifest::default(),
                    dirty: true,
                };
                let result = Self::namespace_info(&ns);
                self.namespaces.insert(ns.id.clone(), ns);
                Ok(result)
            }
            action => {
                let key = self.selected(args)?;
                self.check_revision(&key, args)?;
                match action {
                    "get" => Ok(Self::namespace_info(&self.namespaces[&key])),
                    "update" | "rename" => {
                        let new_name = name(required(args, "name")?)?;
                        if self.namespaces[&key].name != new_name {
                            self.available(&new_name)?;
                        }
                        let ns = self.namespaces.get_mut(&key).expect("selected");
                        ns.name = new_name;
                        ns.dirty = true;
                        Ok(Self::namespace_info(ns))
                    }
                    "copy" => {
                        let new_name = name(required(args, "name")?)?;
                        self.available(&new_name)?;
                        let mut ns = self.namespaces[&key].clone();
                        for (database_name, database) in &mut ns.manifest.databases {
                            if let Some(working) = self
                                .working_databases
                                .get(&(key.clone(), database_name.clone()))
                            {
                                let frozen = self.staging.join(format!("{}.sqlite", id()));
                                // All statements have completed and DELETE journals are closed.
                                // Copy the main database while preserving the source connection's temporary state.
                                fs::copy(&working.path, &frozen)?;
                                database.snapshot = self.seal(&frozen)?;
                            }
                        }
                        ns.id = id();
                        ns.name = new_name;
                        // An unchanged committed copy keeps the exact root reference.
                        let result = Self::namespace_info(&ns);
                        ns.dirty = true;
                        self.namespaces.insert(ns.id.clone(), ns);
                        Ok(result)
                    }
                    "delete" => {
                        let ns = self.namespaces.get_mut(&key).expect("selected");
                        ns.deleted = true;
                        ns.dirty = true;
                        Ok(json!({"deleted":true,"id":key}))
                    }
                    _ => Err(Error::invalid("unknown namespace action")),
                }
            }
        }
    }
    fn available(&self, name: &str) -> Result<()> {
        if self
            .namespaces
            .values()
            .any(|n| n.name == name || n.id == name)
        {
            Err(Error::new(
                "ALREADY_EXISTS",
                format!("namespace name {name} is reserved"),
            ))
        } else {
            Ok(())
        }
    }
    fn bytes(&self, hash: &str) -> Result<Vec<u8>> {
        if let Some(bytes) = self.objects.get(hash) {
            return Ok(bytes.clone());
        }
        self.store
            .catalog()?
            .query_row("SELECT bytes FROM objects WHERE hash=?", [hash], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| Error::new("CORRUPT_STORE", "file object is missing"))
    }
    fn put_bytes(&mut self, bytes: Vec<u8>) -> Result<String> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(Error::limit("virtual file exceeds 8 MiB"));
        }
        let key = hash(&bytes);
        self.objects.insert(key.clone(), bytes);
        Ok(key)
    }
    fn file(&mut self, args: &Value) -> Result<Value> {
        let key = self.selected(args)?;
        self.check_revision(&key, args)?;
        let action = required(args, "action")?;
        let path = virtual_path(args.get("path").and_then(Value::as_str).unwrap_or("/"))?;
        let files = &self.namespaces[&key].manifest.files;
        match action {
            "list" => {
                let prefix = if path == "/" {
                    "/".into()
                } else {
                    format!("{path}/")
                };
                let mut entries = BTreeMap::new();
                for (p, hash) in files {
                    if let Some(tail) = p.strip_prefix(&prefix) {
                        let first = tail.split('/').next().expect("split");
                        let full = format!("{prefix}{first}");
                        entries.entry(full.clone()).or_insert_with(||json!({"path":full,"kind":if tail.contains('/') {"directory"} else {"file"},"hash":if tail.contains('/') {Value::Null} else {json!(hash)}}));
                    }
                }
                if path != "/" && entries.is_empty() {
                    return Err(Error::new("NOT_FOUND", "directory does not exist"));
                }
                Ok(json!({"entries":entries.into_values().collect::<Vec<_>>()}))
            }
            "read" | "stat" => {
                if let Some(h) = files.get(&path) {
                    let bytes = self.bytes(h)?;
                    let mut result = json!({"path":path,"kind":"file","hash":h,"size":bytes.len()});
                    if action == "read" {
                        match String::from_utf8(bytes.clone()) {
                            Ok(text) => result["text"] = json!(text),
                            Err(_) => result["base64"] = json!(STANDARD.encode(bytes)),
                        }
                    }
                    Ok(result)
                } else if action == "stat"
                    && (path == "/" || files.keys().any(|p| p.starts_with(&format!("{path}/"))))
                {
                    Ok(json!({"path":path,"kind":"directory"}))
                } else {
                    Err(Error::new("NOT_FOUND", "file does not exist"))
                }
            }
            "write" | "append" => {
                if path == "/"
                    || files.keys().any(|p| {
                        p.starts_with(&format!("{path}/")) || path.starts_with(&format!("{p}/"))
                    })
                {
                    return Err(Error::invalid("file collides with a directory"));
                }
                if args.get("text").is_some() && args.get("base64").is_some() {
                    return Err(Error::invalid("provide text or base64, not both"));
                }
                let mut bytes = if action == "append" {
                    if let Some(h) = files.get(&path) {
                        self.bytes(h)?
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                if let Some(text) = args.get("text") {
                    bytes.extend_from_slice(
                        text.as_str()
                            .ok_or_else(|| Error::invalid("text must be a string"))?
                            .as_bytes(),
                    );
                } else {
                    bytes.extend(
                        STANDARD
                            .decode(required(args, "base64")?)
                            .map_err(|_| Error::invalid("invalid base64"))?,
                    );
                }
                let size = bytes.len();
                let hash = self.put_bytes(bytes)?;
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.files.insert(path.clone(), hash.clone());
                ns.dirty = true;
                Ok(json!({"path":path,"hash":hash,"size":size}))
            }
            "delete" => {
                let mut removed = Vec::new();
                if files.contains_key(&path) {
                    removed.push(path.clone());
                } else if args.get("recursive").and_then(Value::as_bool) == Some(true) {
                    let prefix = if path == "/" {
                        "/".into()
                    } else {
                        format!("{path}/")
                    };
                    removed.extend(files.keys().filter(|p| p.starts_with(&prefix)).cloned());
                }
                if removed.is_empty() {
                    return Err(Error::new(
                        "NOT_FOUND",
                        "file does not exist (directory deletion requires recursive=true)",
                    ));
                }
                let ns = self.namespaces.get_mut(&key).expect("selected");
                for p in &removed {
                    ns.manifest.files.remove(p);
                }
                ns.dirty = true;
                Ok(json!({"deleted":removed.len()}))
            }
            "move" | "copy" => {
                let destination = virtual_path(required(args, "destination")?)?;
                let hash = files
                    .get(&path)
                    .cloned()
                    .ok_or_else(|| Error::new("NOT_FOUND", "file does not exist"))?;
                if destination == "/"
                    || files.contains_key(&destination)
                    || files.keys().any(|p| {
                        p.starts_with(&format!("{destination}/"))
                            || destination.starts_with(&format!("{p}/"))
                    })
                {
                    return Err(Error::new(
                        "ALREADY_EXISTS",
                        "destination exists or collides with directory",
                    ));
                }
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.files.insert(destination.clone(), hash.clone());
                if action == "move" {
                    ns.manifest.files.remove(&path);
                }
                ns.dirty = true;
                Ok(json!({"path":destination,"hash":hash}))
            }
            _ => Err(Error::invalid("unknown file action")),
        }
    }
    fn snapshot_path(&self, snapshot: &str) -> PathBuf {
        self.store
            .root
            .join("snapshots")
            .join(format!("{snapshot}.sqlite"))
    }
    fn working(&self, db: Option<&Database>) -> Result<PathBuf> {
        let path = self.staging.join(format!("{}.sqlite", id()));
        if let Some(db) = db {
            fs::copy(self.snapshot_path(&db.snapshot), &path)?;
        }
        Ok(path)
    }
    fn seal(&self, path: &Path) -> Result<String> {
        if fs::metadata(path)?.len() > sql::MAX_DB {
            return Err(Error::limit("database exceeds 256 MiB disk quota"));
        }
        File::open(path)?.sync_all()?;
        let snapshot = id();
        fs::rename(path, self.snapshot_path(&snapshot))?;
        sync_dir(&self.store.root.join("snapshots"))?;
        Ok(snapshot)
    }
    fn ensure_working(
        &mut self,
        namespace: &str,
        database: &str,
        db: &Database,
    ) -> Result<&Connection> {
        let key = (namespace.to_string(), database.to_string());
        if !self.working_databases.contains_key(&key) {
            let path = self.working(Some(db))?;
            let connection = sql::open(&path, true)?;
            self.working_databases
                .insert(key.clone(), WorkingDatabase { connection, path });
        }
        Ok(&self.working_databases[&key].connection)
    }
    fn database(&mut self, args: &Value) -> Result<Value> {
        let key = self.selected(args)?;
        self.check_revision(&key, args)?;
        let action = required(args, "action")?;
        if action == "list" {
            return Ok(
                json!({"databases":self.namespaces[&key].manifest.databases.iter().map(|(name,db)|json!({"name":name,"id":db.id,"snapshot":db.snapshot,"migrations":db.migrations})).collect::<Vec<_>>()}),
            );
        }
        let db_name = name(required(args, "database")?)?;
        let existing = self.namespaces[&key]
            .manifest
            .databases
            .get(&db_name)
            .cloned();
        if action == "create" {
            if existing.is_some() {
                return Err(Error::new("ALREADY_EXISTS", "database already exists"));
            }
            let path = self.working(None)?;
            let conn = sql::open(&path, true)?;
            conn.execute_batch(
                "CREATE TABLE _state_initialization(x); DROP TABLE _state_initialization;",
            )?;
            conn.close().map_err(|(_, e)| e)?;
            let db = Database {
                id: id(),
                snapshot: self.seal(&path)?,
                migrations: Vec::new(),
            };
            let result = json!({"name":db_name,"id":db.id,"snapshot":db.snapshot});
            let ns = self.namespaces.get_mut(&key).expect("selected");
            ns.manifest.databases.insert(db_name, db);
            ns.dirty = true;
            return Ok(result);
        }
        let mut db = existing
            .ok_or_else(|| Error::new("NOT_FOUND", format!("database {db_name} does not exist")))?;
        match action {
            "query" | "inspect" => {
                let opened;
                let conn =
                    if let Some(working) = self.working_databases.get(&(key, db_name.clone())) {
                        &working.connection
                    } else {
                        opened = sql::open(&self.snapshot_path(&db.snapshot), false)?;
                        &opened
                    };
                if action == "query" {
                    sql::run(
                        conn,
                        required(args, "sql")?,
                        args.get("params").unwrap_or(&json!([])),
                        true,
                    )
                } else {
                    let mut result = sql::inspect(conn)?;
                    result["name"] = json!(db_name);
                    result["id"] = json!(db.id);
                    result["migrations"] = json!(db.migrations);
                    Ok(result)
                }
            }
            "migrations" => Ok(json!({"migrations":db.migrations})),
            "drop" => {
                if self.namespaces[&key].manifest.functions.values().any(|f| {
                    f.get("database_ids")
                        .and_then(Value::as_object)
                        .is_some_and(|m| m.values().any(|v| v.as_str() == Some(&db.id)))
                }) {
                    return Err(Error::new(
                        "CONFLICT",
                        "database is referenced by a declared function",
                    ));
                }
                self.working_databases
                    .remove(&(key.clone(), db_name.clone()));
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.databases.remove(&db_name);
                ns.dirty = true;
                Ok(json!({"dropped":true}))
            }
            "execute" => {
                let conn = self.ensure_working(&key, &db_name, &db)?;
                let result = sql::run(
                    conn,
                    required(args, "sql")?,
                    args.get("params").unwrap_or(&json!([])),
                    false,
                )?;
                self.namespaces.get_mut(&key).expect("selected").dirty = true;
                Ok(result)
            }
            "migrate" => {
                let migrations = args
                    .get("migrations")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::invalid("migrations must be an array"))?;
                let mut seen = BTreeSet::new();
                let mut pending = Vec::new();
                let mut prefix_position = 0;
                let prefix_mode = migrations
                    .first()
                    .and_then(|m| m.get("id"))
                    .and_then(Value::as_str)
                    .is_some_and(|first| db.migrations.iter().any(|m| m.id == first));
                for migration in migrations {
                    let migration_id = name(required(migration, "id")?)?;
                    if !seen.insert(migration_id.clone()) {
                        return Err(Error::new("MIGRATION_MISMATCH", "duplicate migration ID"));
                    }
                    let source = required(migration, "sql")?;
                    let checksum = hash(source.as_bytes());
                    if let Some(old) = db.migrations.iter().find(|m| m.id == migration_id) {
                        if !prefix_mode
                            || prefix_position >= db.migrations.len()
                            || db.migrations[prefix_position].id != migration_id
                            || old.checksum != checksum
                        {
                            return Err(Error::new(
                                "MIGRATION_MISMATCH",
                                "migration history or checksum differs",
                            ));
                        }
                        prefix_position += 1;
                        continue;
                    }
                    if prefix_mode && prefix_position < db.migrations.len() {
                        return Err(Error::new(
                            "MIGRATION_MISMATCH",
                            "migration list skipped applied entries",
                        ));
                    }
                    pending.push(source.to_string());
                    db.migrations.push(Migration {
                        id: migration_id,
                        checksum,
                    });
                    prefix_position = db.migrations.len();
                }
                if !pending.is_empty() {
                    let conn = self.ensure_working(&key, &db_name, &db)?;
                    for source in &pending {
                        sql::batch(conn, source)?;
                    }
                    let ns = self.namespaces.get_mut(&key).expect("selected");
                    ns.manifest.databases.insert(db_name, db.clone());
                    ns.dirty = true;
                }
                Ok(json!({"applied":pending.len(),"migrations":db.migrations}))
            }
            _ => Err(Error::invalid("unknown database action")),
        }
    }
    fn function(&mut self, args: &Value) -> Result<Value> {
        let key = self.selected(args)?;
        self.check_revision(&key, args)?;
        let action = required(args, "action")?;
        if action == "list" {
            return Ok(
                json!({"functions":self.namespaces[&key].manifest.functions.values().cloned().collect::<Vec<_>>()}),
            );
        }
        let function_name = name(required(args, "name")?)?;
        let existing = self.namespaces[&key].manifest.functions.get(&function_name);
        if let Some(expected) = args.get("expected_version")
            && existing.and_then(|f| f.get("version")) != Some(expected)
        {
            return Err(Error::new("CONFLICT", "function version does not match"));
        }
        match action {
            "get" => existing
                .cloned()
                .ok_or_else(|| Error::new("NOT_FOUND", "function does not exist")),
            "remove" => {
                if existing.is_none() {
                    return Err(Error::new("NOT_FOUND", "function does not exist"));
                }
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.functions.remove(&function_name);
                ns.dirty = true;
                Ok(json!({"removed":true}))
            }
            "declare" | "update" => {
                let file = virtual_path(required(args, "file")?)?;
                let source_hash = self.namespaces[&key]
                    .manifest
                    .files
                    .get(&file)
                    .cloned()
                    .ok_or_else(|| Error::new("NOT_FOUND", "source file does not exist"))?;
                let source = String::from_utf8(self.bytes(&source_hash)?)
                    .map_err(|_| Error::invalid("function source must be UTF-8"))?;
                required(args, "symbol")?;
                let mut metadata = args
                    .as_object()
                    .cloned()
                    .ok_or_else(|| Error::invalid("declaration must be an object"))?;
                for field in [
                    "action",
                    "namespace",
                    "expected_version",
                    "expected_revision",
                    "version",
                ] {
                    metadata.remove(field);
                }
                let mut database_ids = serde_json::Map::new();
                if let Some(grants) = args.get("databases") {
                    for (name, mode) in grants.as_object().ok_or_else(|| {
                        Error::invalid("databases must map database names to read/write/migrate")
                    })? {
                        if !matches!(mode.as_str(), Some("read" | "write" | "migrate")) {
                            return Err(Error::invalid(
                                "database grant must be read, write, or migrate",
                            ));
                        }
                        let db = self.namespaces[&key]
                            .manifest
                            .databases
                            .get(name)
                            .ok_or_else(|| {
                                Error::new(
                                    "NOT_FOUND",
                                    format!("database grant {name} does not exist"),
                                )
                            })?;
                        database_ids.insert(name.clone(), json!(db.id));
                    }
                }
                metadata.insert("database_ids".into(), json!(database_ids));
                metadata.insert("file".into(), json!(file));
                metadata.insert("source".into(), json!(source));
                metadata.insert("source_hash".into(), json!(source_hash));
                metadata.insert("abi_version".into(), json!(1));
                let version = hash(&serde_json::to_vec(&metadata)?);
                metadata.insert("version".into(), json!(version));
                let metadata = Value::Object(metadata);
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest
                    .functions
                    .insert(function_name, metadata.clone());
                ns.dirty = true;
                Ok(metadata)
            }
            _ => Err(Error::invalid("unknown function action")),
        }
    }
    pub fn commit(mut self) -> Result<Value> {
        if self.poisoned {
            return Err(Error::new(
                "TRANSACTION_ABORTED",
                "a prior operation failed; no changes were published",
            ));
        }
        for ((namespace, database), working) in std::mem::take(&mut self.working_databases) {
            working.connection.close().map_err(|(_, e)| e)?;
            let snapshot = self.seal(&working.path)?;
            if let Some(db) = self
                .namespaces
                .get_mut(&namespace)
                .and_then(|ns| ns.manifest.databases.get_mut(&database))
            {
                db.snapshot = snapshot;
            }
        }
        let changed = self.namespaces.values().any(|n| n.dirty);
        if !changed && self.receipt.is_none() {
            return Ok(json!({"generation":self.generation,"revisions":{},"changed":false}));
        }
        let mut conn = self.store.catalog()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((principal, key, desired)) = &self.receipt
            && let Some(existing) = receipt(&tx, principal, key)?
        {
            if existing.request_hash != desired.request_hash {
                return Err(Error::new(
                    "IDEMPOTENCY_MISMATCH",
                    "idempotency key was used for another request",
                ));
            }
            return Ok(json!({"replayed":true,"result":existing.result}));
        }
        let current: i64 = tx.query_row(
            "SELECT generation FROM catalog_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if current != self.generation {
            return Err(Error::new(
                "CONFLICT",
                "catalog changed during invocation; retry from a fresh snapshot",
            ));
        }
        for (hash, bytes) in &self.objects {
            tx.execute(
                "INSERT OR IGNORE INTO objects(hash,bytes) VALUES(?,?)",
                params![hash, bytes],
            )?;
        }
        let mut revisions = serde_json::Map::new();
        for ns in self.namespaces.values().filter(|n| n.dirty) {
            let manifest = serde_json::to_string(&ns.manifest)?;
            let previous: Option<String> = if ns.revision.is_empty() {
                None
            } else {
                tx.query_row(
                    "SELECT manifest FROM revisions WHERE id=?",
                    [&ns.revision],
                    |r| r.get(0),
                )
                .optional()?
            };
            let revision = if previous.as_deref() == Some(&manifest) {
                ns.revision.clone()
            } else {
                let revision = id();
                tx.execute(
                    "INSERT INTO revisions(id,parent_id,manifest) VALUES(?,?,?)",
                    params![
                        revision,
                        if ns.revision.is_empty() {
                            None
                        } else {
                            Some(&ns.revision)
                        },
                        manifest
                    ],
                )?;
                revision
            };
            tx.execute("INSERT INTO namespaces(id,name,head_revision,deleted) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,head_revision=excluded.head_revision,deleted=excluded.deleted",params![ns.id,ns.name,revision,ns.deleted])?;
            revisions.insert(ns.id.clone(), json!(revision));
        }
        if let Some((principal, key, receipt)) = &self.receipt {
            tx.execute(
                "INSERT INTO receipts(principal,key,request_hash,result_json) VALUES(?,?,?,?)",
                params![
                    principal,
                    key,
                    receipt.request_hash,
                    serde_json::to_string(&receipt.result)?
                ],
            )?;
        }
        tx.execute(
            "UPDATE catalog_state SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(json!({"generation":current+1,"revisions":revisions,"changed":changed}))
    }
}

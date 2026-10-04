//! Application connections never open or attach the catalog.
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{
    Connection, OpenFlags,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) const MAX_RESULT: usize = 1024 * 1024;
pub(crate) const MAX_DB: u64 = 256 * 1024 * 1024;

// Classify SQLite names at the authorizer boundary without allocating lowercase copies.
#[derive(Clone, Copy)]
enum PragmaPolicy {
    Inspection,
    ScalarRead,
}
impl PragmaPolicy {
    fn parse(name: &str) -> Option<Self> {
        const NAMES: &[(&str, PragmaPolicy)] = &[
            ("table_info", PragmaPolicy::Inspection),
            ("table_xinfo", PragmaPolicy::Inspection),
            ("index_list", PragmaPolicy::Inspection),
            ("index_info", PragmaPolicy::Inspection),
            ("index_xinfo", PragmaPolicy::Inspection),
            ("foreign_key_list", PragmaPolicy::Inspection),
            ("table_list", PragmaPolicy::ScalarRead),
            ("database_list", PragmaPolicy::ScalarRead),
            ("user_version", PragmaPolicy::ScalarRead),
            ("application_id", PragmaPolicy::ScalarRead),
            ("schema_version", PragmaPolicy::ScalarRead),
            ("encoding", PragmaPolicy::ScalarRead),
            ("page_count", PragmaPolicy::ScalarRead),
            ("page_size", PragmaPolicy::ScalarRead),
            ("freelist_count", PragmaPolicy::ScalarRead),
            ("foreign_keys", PragmaPolicy::ScalarRead),
            ("compile_options", PragmaPolicy::ScalarRead),
            ("integrity_check", PragmaPolicy::ScalarRead),
            ("quick_check", PragmaPolicy::ScalarRead),
            ("foreign_key_check", PragmaPolicy::ScalarRead),
        ];
        NAMES
            .iter()
            .find_map(|(wire, policy)| name.eq_ignore_ascii_case(wire).then_some(*policy))
    }
}
#[derive(Clone, Copy)]
enum RestrictedFunction {
    LoadExtension,
    ReadFile,
    WriteFile,
    Fts3Tokenizer,
}
impl RestrictedFunction {
    fn parse(name: &str) -> Option<Self> {
        const NAMES: &[(&str, RestrictedFunction)] = &[
            ("load_extension", RestrictedFunction::LoadExtension),
            ("readfile", RestrictedFunction::ReadFile),
            ("writefile", RestrictedFunction::WriteFile),
            ("fts3_tokenizer", RestrictedFunction::Fts3Tokenizer),
        ];
        NAMES
            .iter()
            .find_map(|(wire, kind)| name.eq_ignore_ascii_case(wire).then_some(*kind))
    }
}
#[derive(Clone, Copy)]
enum VirtualTableModule {
    Fts5,
    Rtree,
    RtreeI32,
}
impl VirtualTableModule {
    fn parse(name: &str) -> Option<Self> {
        const NAMES: &[(&str, VirtualTableModule)] = &[
            ("fts5", VirtualTableModule::Fts5),
            ("rtree", VirtualTableModule::Rtree),
            ("rtree_i32", VirtualTableModule::RtreeI32),
        ];
        NAMES
            .iter()
            .find_map(|(wire, kind)| (name == *wire).then_some(*kind))
    }
}

pub(crate) fn open(path: &Path, writable: bool) -> Result<Connection> {
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA temp_store=MEMORY; PRAGMA temp.page_size=4096; PRAGMA temp.max_page_count=4096;",
    )?;
    if writable {
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA page_size=4096; PRAGMA max_page_count=65536;")?;
    }
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_RESULT as i32)?;
    conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_RESULT as i32)?;
    conn.set_limit(Limit::SQLITE_LIMIT_COLUMN, 256)?;
    conn.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, 100)?;
    conn.set_limit(Limit::SQLITE_LIMIT_VDBE_OP, 100_000)?;
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    conn.set_limit(Limit::SQLITE_LIMIT_WORKER_THREADS, 0)?;
    let start = Instant::now();
    conn.progress_handler(1000, Some(move || start.elapsed() > Duration::from_secs(2)))?;
    conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Unknown { .. } => Authorization::Deny,
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } => {
            let allowed = match PragmaPolicy::parse(pragma_name) {
                Some(PragmaPolicy::Inspection) => true,
                Some(PragmaPolicy::ScalarRead) => pragma_value.is_none(),
                None => false,
            };
            if allowed {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        AuthAction::Function { function_name }
            if RestrictedFunction::parse(function_name).is_some() =>
        {
            Authorization::Deny
        }
        AuthAction::CreateVtable { module_name, .. }
        | AuthAction::DropVtable { module_name, .. }
            if VirtualTableModule::parse(module_name).is_none() =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }))?;
    Ok(conn)
}

fn parameter(value: &Value) -> Result<SqlValue> {
    Ok(match value {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if n.is_u64() {
                return Err(Error::new(
                    "INVALID_ARGUMENT",
                    "integer parameter exceeds SQLite signed 64-bit range",
                ));
            } else {
                SqlValue::Real(n.as_f64().ok_or_else(|| Error::invalid("invalid number"))?)
            }
        }
        Value::String(s) => SqlValue::Text(s.clone()),
        Value::Object(m) if m.len() == 1 && m.contains_key("$base64") => SqlValue::Blob(
            STANDARD
                .decode(
                    m["$base64"]
                        .as_str()
                        .ok_or_else(|| Error::invalid("$base64 must be a string"))?,
                )
                .map_err(|_| Error::invalid("invalid base64 parameter"))?,
        ),
        Value::Object(m) if m.len() == 1 && m.contains_key("$integer") => SqlValue::Integer(
            m["$integer"]
                .as_str()
                .ok_or_else(|| Error::invalid("$integer must be a string"))?
                .parse()
                .map_err(|_| Error::invalid("invalid signed integer parameter"))?,
        ),
        _ => {
            return Err(Error::invalid(
                "SQL parameters must be scalar values or tagged $base64/$integer objects",
            ));
        }
    })
}

fn output(value: ValueRef<'_>) -> Result<Value> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&i) => {
            json!({"$integer":i.to_string()})
        }
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| Error::invalid("non-finite SQL result"))?,
        ValueRef::Text(bytes) => Value::String(
            std::str::from_utf8(bytes)
                .map_err(|_| Error::invalid("SQL text result is not UTF-8"))?
                .to_string(),
        ),
        ValueRef::Blob(bytes) => json!({"$base64":STANDARD.encode(bytes)}),
    })
}

pub(crate) fn run(
    conn: &Connection,
    sql: &str,
    params: &[Value],
    read_only: bool,
) -> Result<Value> {
    reset_deadline(conn)?;
    if sql.len() > MAX_RESULT {
        return Err(Error::limit("SQL exceeds 1 MiB"));
    }
    let params: Vec<SqlValue> = params.iter().map(parameter).collect::<Result<_>>()?;
    let mut stmt = conn.prepare(sql)?;
    if read_only && !stmt.readonly() {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "query requires a read-only SQL statement",
        ));
    }
    let count = stmt.column_count();
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let before = conn.total_changes();
    let mut cursor = stmt.query(rusqlite::params_from_iter(params))?;
    let mut rows = Vec::new();
    let mut bytes = serde_json::to_vec(&columns)?.len();
    while let Some(row) = cursor.next()? {
        let cells: Vec<Value> = (0..count)
            .map(|i| output(row.get_ref(i)?))
            .collect::<Result<_>>()?;
        bytes += serde_json::to_vec(&cells)?.len();
        if rows.len() >= 10_000 || bytes > MAX_RESULT {
            return Err(Error::limit("SQL result exceeds 10,000 rows or 1 MiB"));
        }
        rows.push(cells);
    }
    Ok(json!({"columns":columns,"rows":rows,"rows_affected":conn.total_changes()-before}))
}

pub(crate) fn batch(conn: &Connection, sql: &str) -> Result<()> {
    reset_deadline(conn)?;
    if sql.len() > MAX_RESULT {
        return Err(Error::limit("migration SQL exceeds 1 MiB"));
    }
    conn.execute_batch(sql)?;
    Ok(())
}

pub(crate) fn inspect(conn: &Connection) -> Result<Value> {
    let started = Instant::now();
    let schema = run(
        conn,
        "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type,name",
        &[],
        true,
    )?;
    let mut bytes = serde_json::to_vec(&schema)?.len();
    let mut tables = Vec::new();
    for row in schema["rows"].as_array().expect("query rows") {
        if started.elapsed() > Duration::from_secs(2) {
            return Err(Error::limit("schema inspection exceeds two seconds"));
        }
        if row[0] != "table" && row[0] != "view" {
            continue;
        }
        let name = &row[1];
        let columns = run(
            conn,
            "SELECT cid,name,type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo(?)",
            std::slice::from_ref(name),
            true,
        )?;
        let indexes = run(
            conn,
            "SELECT seq,name,\"unique\",origin,partial FROM pragma_index_list(?)",
            std::slice::from_ref(name),
            true,
        )?;
        bytes += serde_json::to_vec(&columns)?.len();
        inspection_budget(bytes)?;
        let mut index_details = Vec::new();
        for idx in indexes["rows"].as_array().expect("query rows") {
            if started.elapsed() > Duration::from_secs(2) {
                return Err(Error::limit("schema inspection exceeds two seconds"));
            }
            let detail = json!({"name":idx[1], "unique":idx[2], "origin":idx[3], "partial":idx[4], "columns":run(conn,"SELECT seqno,cid,name,desc,coll,key FROM pragma_index_xinfo(?)",std::slice::from_ref(&idx[1]),true)?});
            bytes += serde_json::to_vec(&detail)?.len();
            inspection_budget(bytes)?;
            index_details.push(detail);
        }
        let foreign_keys = run(
            conn,
            "SELECT id,seq,\"table\",\"from\",\"to\",on_update,on_delete,\"match\" FROM pragma_foreign_key_list(?)",
            std::slice::from_ref(name),
            true,
        )?;
        // Check incrementally: waiting until the entire schema is assembled can
        // allocate far beyond the advertised result bound for many wide tables.
        bytes += serde_json::to_vec(&foreign_keys)?.len() + serde_json::to_vec(&row[3])?.len();
        inspection_budget(bytes)?;
        tables.push(json!({"name":name,"type":row[0],"sql":row[3],"columns":columns,"indexes":index_details,"foreign_keys":foreign_keys}));
    }
    let fingerprint = crate::identity::hash(&serde_json::to_vec(&schema)?);
    let result = json!({"schema":schema,"tables":tables,"schema_fingerprint":fingerprint});
    if serde_json::to_vec(&result)?.len() > MAX_RESULT {
        return Err(Error::limit("schema inspection exceeds 1 MiB"));
    }
    Ok(result)
}

fn inspection_budget(bytes: usize) -> Result<()> {
    if bytes > MAX_RESULT {
        return Err(Error::limit("schema inspection exceeds 1 MiB"));
    }
    Ok(())
}

fn reset_deadline(conn: &Connection) -> Result<()> {
    let start = Instant::now();
    conn.progress_handler(1000, Some(move || start.elapsed() > Duration::from_secs(2)))?;
    Ok(())
}

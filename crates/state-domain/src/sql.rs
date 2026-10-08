//! Application connections never open or attach the catalog.
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{
    Connection,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    types::{Value as SqlValue, ValueRef},
};
use state_contract::database::BinaryCell;
use state_contract::database::IntegerCell;
use state_contract::database::SqlCell;
use std::time::{Duration, Instant};

pub(crate) const MAX_RESULT: usize = 1024 * 1024;

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

pub(crate) fn configure(conn: &Connection) -> Result<()> {
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA temp_store=MEMORY; PRAGMA temp.page_size=4096; PRAGMA temp.max_page_count=4096;",
    )?;
    {
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
    Ok(())
}

fn parameter(value: &SqlCell) -> Result<SqlValue> {
    Ok(match value {
        SqlCell::Null(()) => SqlValue::Null,
        SqlCell::Boolean(value) => SqlValue::Integer(i64::from(*value)),
        SqlCell::Integer(value) => SqlValue::Integer(*value),
        SqlCell::Unsigned(value) => {
            SqlValue::Integer(i64::try_from(*value).map_err(|_| {
                Error::invalid("integer parameter exceeds SQLite signed 64-bit range")
            })?)
        }
        SqlCell::Real(value) if value.is_finite() => SqlValue::Real(*value),
        SqlCell::Real(_) => return Err(Error::invalid("non-finite SQL parameter")),
        SqlCell::Text(value) => SqlValue::Text(value.clone()),
        SqlCell::Binary(value) => SqlValue::Blob(
            STANDARD
                .decode(&value.base64)
                .map_err(|_| Error::invalid("invalid base64 parameter"))?,
        ),
        SqlCell::LargeInteger(value) => SqlValue::Integer(
            value
                .integer
                .parse()
                .map_err(|_| Error::invalid("invalid signed integer parameter"))?,
        ),
    })
}
fn output(value: ValueRef<'_>) -> Result<SqlCell> {
    Ok(match value {
        ValueRef::Null => SqlCell::Null(()),
        ValueRef::Integer(integer)
            if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&integer) =>
        {
            SqlCell::LargeInteger(IntegerCell {
                integer: integer.to_string(),
            })
        }
        ValueRef::Integer(integer) => SqlCell::Integer(integer),
        ValueRef::Real(real) if real.is_finite() => SqlCell::Real(real),
        ValueRef::Real(_) => return Err(Error::invalid("non-finite SQL result")),
        ValueRef::Text(bytes) => SqlCell::Text(
            std::str::from_utf8(bytes)
                .map_err(|_| Error::invalid("SQL text result is not UTF-8"))?
                .to_owned(),
        ),
        ValueRef::Blob(bytes) => SqlCell::Binary(BinaryCell {
            base64: STANDARD.encode(bytes),
        }),
    })
}

pub(crate) fn run_typed(
    conn: &Connection,
    sql: &str,
    params: &[SqlCell],
    read_only: bool,
) -> Result<state_contract::database::SqlResult> {
    reset_deadline(conn)?;
    if sql.len() > MAX_RESULT {
        return Err(Error::limit("SQL exceeds 1 MiB"));
    }
    let params: Vec<SqlValue> = params.iter().map(parameter).collect::<Result<_>>()?;
    let mut stmt = conn.prepare(sql)?;
    if read_only && !stmt.readonly() {
        return Err(Error::new(
            crate::ErrorCode::PermissionDenied,
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
        let cells: Vec<SqlCell> = (0..count)
            .map(|i| output(row.get_ref(i)?))
            .collect::<Result<_>>()?;
        bytes += serde_json::to_vec(&cells)?.len();
        if rows.len() >= 10_000 || bytes > MAX_RESULT {
            return Err(Error::limit("SQL result exceeds 10,000 rows or 1 MiB"));
        }
        rows.push(cells);
    }
    Ok(state_contract::database::SqlResult {
        columns,
        rows,
        rows_affected: conn.total_changes() - before,
    })
}

pub(crate) fn batch(conn: &Connection, sql: &str) -> Result<()> {
    reset_deadline(conn)?;
    if sql.len() > MAX_RESULT {
        return Err(Error::limit("migration SQL exceeds 1 MiB"));
    }
    conn.execute_batch(sql)?;
    Ok(())
}

pub(crate) fn inspect_typed(
    conn: &Connection,
) -> Result<state_contract::database::SchemaInspection> {
    use state_contract::database::IndexInspection;
    use state_contract::database::SchemaInspection;
    use state_contract::database::SchemaObjectKind;
    use state_contract::database::TableInspection;
    fn text(value: &SqlCell) -> Result<&str> {
        value.as_text().ok_or_else(|| {
            Error::new(
                crate::ErrorCode::CorruptStore,
                "unexpected SQLite inspection text",
            )
        })
    }
    fn integer(value: &SqlCell) -> Result<i64> {
        value.as_integer().ok_or_else(|| {
            Error::new(
                crate::ErrorCode::CorruptStore,
                "unexpected SQLite inspection integer",
            )
        })
    }
    let started = Instant::now();
    let schema = run_typed(
        conn,
        "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type,name",
        &[],
        true,
    )?;
    let mut bytes = serde_json::to_vec(&schema)?.len();
    let mut tables = Vec::new();
    for row in &schema.rows {
        if started.elapsed() > Duration::from_secs(2) {
            return Err(Error::limit("schema inspection exceeds two seconds"));
        }
        let [kind_cell, name_cell, _table_cell, sql_cell] = row.as_slice() else {
            return Err(Error::new(
                crate::ErrorCode::CorruptStore,
                "unexpected SQLite schema row",
            ));
        };
        let kind = SchemaObjectKind::parse(text(kind_cell)?)?;
        if !matches!(kind, SchemaObjectKind::Table | SchemaObjectKind::View) {
            continue;
        }
        let name = text(name_cell)?.to_owned();
        let columns = run_typed(
            conn,
            "SELECT cid,name,type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo(?)",
            std::slice::from_ref(name_cell),
            true,
        )?;
        let indexes = run_typed(
            conn,
            "SELECT seq,name,\"unique\",origin,partial FROM pragma_index_list(?)",
            std::slice::from_ref(name_cell),
            true,
        )?;
        bytes += serde_json::to_vec(&columns)?.len();
        inspection_budget(bytes)?;
        let mut index_details = Vec::new();
        for idx in &indexes.rows {
            let [_sequence, index_name, unique, origin, partial] = idx.as_slice() else {
                return Err(Error::new(
                    crate::ErrorCode::CorruptStore,
                    "unexpected SQLite index row",
                ));
            };
            if started.elapsed() > Duration::from_secs(2) {
                return Err(Error::limit("schema inspection exceeds two seconds"));
            }
            let detail = IndexInspection {
                name: text(index_name)?.to_owned(),
                unique: integer(unique)?,
                origin: text(origin)?.to_owned(),
                partial: integer(partial)?,
                columns: run_typed(
                    conn,
                    "SELECT seqno,cid,name,desc,coll,key FROM pragma_index_xinfo(?)",
                    std::slice::from_ref(index_name),
                    true,
                )?,
            };
            bytes += serde_json::to_vec(&detail)?.len();
            inspection_budget(bytes)?;
            index_details.push(detail);
        }
        let foreign_keys = run_typed(
            conn,
            "SELECT id,seq,\"table\",\"from\",\"to\",on_update,on_delete,\"match\" FROM pragma_foreign_key_list(?)",
            std::slice::from_ref(name_cell),
            true,
        )?;
        let sql = if matches!(sql_cell, SqlCell::Null(())) {
            None
        } else {
            Some(text(sql_cell)?.to_owned())
        };
        bytes += serde_json::to_vec(&foreign_keys)?.len() + serde_json::to_vec(&sql)?.len();
        inspection_budget(bytes)?;
        tables.push(TableInspection {
            name,
            kind,
            sql,
            columns,
            indexes: index_details,
            foreign_keys,
        });
    }
    let fingerprint = crate::identity::hash(&serde_json::to_vec(&schema)?);
    let result = SchemaInspection {
        schema,
        tables,
        schema_fingerprint: fingerprint,
    };
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

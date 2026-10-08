//! Typed databases operations, shared by controllers and runtime hosts.
use crate::{Context, Result};
use state_contract::database::*;

/// Databases capability; transaction state belongs to the supplied context.
#[derive(Clone, Copy, Default)]
pub struct Databases;
impl Databases {
    /// List the named SQLite databases in a namespace.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn list_databases(
        &self,
        mut input: state_contract::database::DbListInput,
        context: &mut Context,
    ) -> Result<state_contract::database::Databases> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().list_databases(&input)?)
        })
    }
    /// Run one read-only SQL statement with optional bound parameters. Returns columns, rows, and rows_affected.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn query_database(
        &self,
        mut input: state_contract::database::DbQueryInput,
        context: &mut Context,
    ) -> Result<SqlResult> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().query_database(&input)?)
        })
    }
    /// Inspect a database’s tables, columns, indexes, foreign keys, schema, migration history, and storage metadata.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn inspect_database(
        &self,
        mut input: state_contract::database::DbInspectInput,
        context: &mut Context,
    ) -> Result<DatabaseInspection> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().inspect_database(&input)?)
        })
    }
    /// Get the ordered IDs and checksums of applied database migrations.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn database_migrations(
        &self,
        mut input: state_contract::database::DbMigrationsInput,
        context: &mut Context,
    ) -> Result<Migrations> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().database_migrations(&input)?)
        })
    }
    /// Create an empty named SQLite database in a namespace.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn create_database(
        &self,
        mut input: state_contract::database::DbCreateInput,
        context: &mut Context,
    ) -> Result<DatabaseCreated> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().create_database(&input)?)
        })
    }
    /// Drop a named SQLite database. Functions resolve database names within their namespace on each call.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn drop_database(
        &self,
        mut input: state_contract::database::DbDropInput,
        context: &mut Context,
    ) -> Result<DatabaseDropped> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().drop_database(&input)?)
        })
    }
    /// Run one SQL statement with optional bound parameters, including writes and RETURNING. Returns columns, rows, and rows_affected.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn execute_database(
        &self,
        mut input: state_contract::database::DbExecuteInput,
        context: &mut Context,
    ) -> Result<SqlResult> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().execute_database(&input)?)
        })
    }
    /// Apply an ordered batch of SQL migrations atomically. Previously applied IDs must match their stored checksums and order.
    /// Uses the supplied transaction and enforces its scope, shared budgets and sticky failure.
    pub fn migrate_database(
        &self,
        mut input: state_contract::database::DbMigrateInput,
        context: &mut Context,
    ) -> Result<MigrationsApplied> {
        context.operation(|context| {
            input.namespace = context.namespace(&input.namespace)?.as_str().to_owned();
            Ok(context.transaction().migrate_database(&input)?)
        })
    }
}

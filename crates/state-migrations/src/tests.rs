use crate::{Context, Error, ForwardError, Migration, Migrator, Service};
use rusqlite::Connection;
use std::path::Path;

struct SqlMigration {
    descriptor: Migration,
    sql: &'static str,
}
impl Migrator for SqlMigration {
    fn migration(&self) -> Migration {
        self.descriptor.clone()
    }
    fn forward(&self, context: &Context<'_, '_>) -> Result<(), ForwardError> {
        context.catalog().execute_batch(self.sql)?;
        Ok(())
    }
}
fn step(name: &'static str, version: u32, sql: &'static str) -> Result<Box<dyn Migrator>, Error> {
    Ok(Box::new(SqlMigration {
        descriptor: Migration::new(name, version)?,
        sql,
    }))
}

#[test]
fn applies_in_order_and_resumes_without_repeating() -> Result<(), Box<dyn std::error::Error>> {
    let mut catalog = Connection::open_in_memory()?;
    let first = || {
        step(
            "create",
            1,
            "CREATE TABLE items(value INTEGER); INSERT INTO items VALUES(1);",
        )
    };
    Service::new(vec![first()?])?.run_with_fs(
        Path::new("/state"),
        &state_fs::MemoryFs::new(),
        &mut catalog,
    )?;
    let service = Service::new(vec![
        first()?,
        step("append", 3, "INSERT INTO items VALUES(3);")?,
    ])?;
    service.run_with_fs(
        Path::new("/state"),
        &state_fs::MemoryFs::new(),
        &mut catalog,
    )?;
    service.run_with_fs(
        Path::new("/state"),
        &state_fs::MemoryFs::new(),
        &mut catalog,
    )?;
    let values: i64 = catalog.query_row("SELECT SUM(value) FROM items", [], |r| r.get(0))?;
    assert_eq!(values, 4);
    let history = catalog
        .prepare("SELECT name, version FROM migrations ORDER BY version")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(history, vec![("create".into(), 1), ("append".into(), 3)]);
    Ok(())
}

#[test]
fn failure_rolls_back_catalog_and_can_retry() -> Result<(), Box<dyn std::error::Error>> {
    let mut catalog = Connection::open_in_memory()?;
    let first = || step("create", 1, "CREATE TABLE items(value INTEGER);");
    let failed = Service::new(vec![
        first()?,
        step(
            "append",
            2,
            "INSERT INTO items VALUES(2); SELECT * FROM missing;",
        )?,
    ])?;
    assert!(matches!(
        failed.run_with_fs(
            Path::new("/state"),
            &state_fs::MemoryFs::new(),
            &mut catalog
        ),
        Err(Error::Forward { .. })
    ));
    let rows: i64 = catalog.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
    assert_eq!(rows, 0);
    let records: i64 = catalog.query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))?;
    assert_eq!(records, 1);
    Service::new(vec![
        first()?,
        step("append", 2, "INSERT INTO items VALUES(2);")?,
    ])?
    .run_with_fs(
        Path::new("/state"),
        &state_fs::MemoryFs::new(),
        &mut catalog,
    )?;
    Ok(())
}

#[test]
fn rejects_unknown_changed_and_nonprefix_history() -> Result<(), Box<dyn std::error::Error>> {
    let mut catalog = Connection::open_in_memory()?;
    Service::new(vec![step("first", 1, "")?, step("second", 2, "")?])?.run_with_fs(
        Path::new("/state"),
        &state_fs::MemoryFs::new(),
        &mut catalog,
    )?;
    for definitions in [
        vec![],
        vec![step("changed", 1, "")?, step("second", 2, "")?],
    ] {
        assert!(matches!(
            Service::new(definitions)?.run_with_fs(
                Path::new("/state"),
                &state_fs::MemoryFs::new(),
                &mut catalog
            ),
            Err(Error::HistoryMismatch)
        ));
    }
    catalog.execute("DELETE FROM migrations WHERE version=1", [])?;
    assert!(matches!(
        Service::new(vec![step("first", 1, "")?, step("second", 2, "")?])?.run_with_fs(
            Path::new("/state"),
            &state_fs::MemoryFs::new(),
            &mut catalog
        ),
        Err(Error::HistoryMismatch)
    ));
    Ok(())
}

#[test]
fn rejects_invalid_definitions() -> Result<(), Box<dyn std::error::Error>> {
    assert!(Migration::new("", 1).is_err());
    assert!(Migration::new("first", 0).is_err());
    assert!(Service::new(vec![step("same", 1, "")?, step("same", 2, "")?]).is_err());
    assert!(Service::new(vec![step("first", 2, "")?, step("second", 1, "")?]).is_err());
    assert!(Service::new(vec![step("first", 1, "")?, step("second", 1, "")?]).is_err());
    Ok(())
}

#[test]
fn migration_cannot_commit_or_rollback_service_transaction()
-> Result<(), Box<dyn std::error::Error>> {
    for command in ["COMMIT", "ROLLBACK", "BEGIN", "SAVEPOINT nested"] {
        let mut catalog = Connection::open_in_memory()?;
        let service = Service::new(vec![
            step("create", 1, "CREATE TABLE items(value INTEGER);")?,
            step("control", 2, command)?,
        ])?;
        assert!(matches!(
            service.run_with_fs(
                Path::new("/state"),
                &state_fs::MemoryFs::new(),
                &mut catalog
            ),
            Err(Error::Forward { .. })
        ));
        assert!(catalog.is_autocommit());
        let records: i64 =
            catalog.query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))?;
        assert_eq!(records, 1);
    }
    Ok(())
}

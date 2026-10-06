use crate::{DatabaseName, Error, Store};
use state_migrations::{Context, ForwardError, Migration, Migrator, Service};

struct Step {
    descriptor: Migration,
    sql: &'static str,
}
impl Migrator for Step {
    fn migration(&self) -> Migration {
        self.descriptor.clone()
    }
    fn forward(&self, context: &Context<'_, '_>) -> Result<(), ForwardError> {
        context.catalog().execute_batch(self.sql)?;
        Ok(())
    }
}
fn step(
    name: &'static str,
    version: u32,
    sql: &'static str,
) -> Result<Box<dyn Migrator>, state_migrations::Error> {
    Ok(Box::new(Step {
        descriptor: Migration::new(name, version)?,
        sql,
    }))
}

#[test]
fn startup_migrations_publish_history_and_data_and_skip_applied()
-> Result<(), Box<dyn std::error::Error>> {
    let fs = crate::fs::MemoryFs::new();
    let service = Service::new(vec![step(
        "initialize",
        1,
        "CREATE TABLE items(value INTEGER); INSERT INTO items VALUES(1);",
    )?])?;
    let store = Store::builder()
        .with_fs(fs.clone())
        .open_with_migrations("/state", &service)?;
    let reader = store.begin_read()?;
    let name = DatabaseName::parse("catalog.sqlite")?;
    let catalog = reader.connect(&name)?;
    let history: (String, u32) =
        catalog.query_row("SELECT name, version FROM migrations", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    assert_eq!(history, ("initialize".into(), 1));
    assert!(matches!(store.migrate(&service), Err(Error::Busy)));
    drop(catalog);
    reader.finish()?;
    drop(store);
    // Re-running CREATE TABLE would fail, so reopening demonstrates it is skipped.
    Store::builder()
        .with_fs(fs.clone())
        .open_with_migrations("/state", &service)?;
    Ok(())
}

#[test]
fn failed_batch_discards_all_pending_changes_and_history() -> Result<(), Box<dyn std::error::Error>>
{
    let fs = crate::fs::MemoryFs::new();
    let initial = || step("initialize", 1, "CREATE TABLE items(value INTEGER);");
    let store = Store::builder()
        .with_fs(fs.clone())
        .open_with_migrations("/state", &Service::new(vec![initial()?])?)?;
    let failed = Service::new(vec![
        initial()?,
        step("append", 2, "INSERT INTO items VALUES(2);")?,
        step(
            "failure",
            3,
            "CREATE TABLE transient(value); SELECT * FROM missing;",
        )?,
    ])?;
    assert!(matches!(
        store.migrate(&failed),
        Err(Error::Migration(state_migrations::Error::Forward { .. }))
    ));
    let reader = store.begin_read()?;
    let catalog = reader.connect(&DatabaseName::parse("catalog.sqlite")?)?;
    let rows: i64 = catalog.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
    let records: i64 = catalog.query_row("SELECT COUNT(*) FROM migrations", [], |r| r.get(0))?;
    let transient: i64 = catalog.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name='transient'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!((rows, records, transient), (0, 1, 0));
    drop(catalog);
    reader.finish()?;
    let repaired = Service::new(vec![
        initial()?,
        step("append", 2, "INSERT INTO items VALUES(2);")?,
        step("failure", 3, "INSERT INTO items VALUES(3);")?,
    ])?;
    store.migrate(&repaired)?;
    let reader = store.begin_read()?;
    let catalog = reader.connect(&DatabaseName::parse("catalog.sqlite")?)?;
    let total: i64 = catalog.query_row("SELECT SUM(value) FROM items", [], |r| r.get(0))?;
    assert_eq!(total, 5);
    Ok(())
}

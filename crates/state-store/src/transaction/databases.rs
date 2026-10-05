//! Named application databases, SQL routing, and ordered migration history.
use super::Transaction;
use crate::identity::hash;
use crate::{DatabaseAction, DatabaseRequest};
use crate::{
    Error, Result,
    identity::id,
    model::{Database, Migration},
    sql,
    validation::{name, required},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

impl Transaction {
    pub(super) fn database(&mut self, args: &DatabaseRequest) -> Result<Value> {
        let action = args.action;
        let key = self.selected(&args.namespace)?;
        self.check_revision(&key, args.expected_revision.as_deref())?;
        if action == DatabaseAction::List {
            return Ok(
                json!({"databases":self.namespaces[&key].manifest.databases.iter().map(|(name,db)|json!({"name":name,"id":db.id,"snapshot":db.snapshot,"migrations":db.migrations})).collect::<Vec<_>>()}),
            );
        }
        let db_name = name(required(args.database.as_deref(), "database")?)?;
        let existing = self.namespaces[&key]
            .manifest
            .databases
            .get(&db_name)
            .cloned();
        if action == DatabaseAction::Create {
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
            DatabaseAction::Query | DatabaseAction::Inspect => {
                let opened;
                let conn = if let Some(working) =
                    self.working_databases.get(&(key.clone(), db_name.clone()))
                {
                    &working.connection
                } else {
                    opened = sql::open(&self.snapshot_path(&db.snapshot), false)?;
                    &opened
                };
                if action == DatabaseAction::Query {
                    sql::run(
                        conn,
                        required(args.sql.as_deref(), "sql")?,
                        &args.params,
                        true,
                    )
                } else {
                    let mut result = sql::inspect(conn)?;
                    result["name"] = json!(db_name);
                    result["id"] = json!(db.id);
                    result["snapshot"] = json!(db.snapshot);
                    result["staged"] =
                        json!(self.working_databases.contains_key(&(key, db_name.clone())));
                    result["migrations"] = json!(db.migrations);
                    Ok(result)
                }
            }
            DatabaseAction::Migrations => Ok(json!({"migrations":db.migrations})),
            DatabaseAction::Drop => {
                self.working_databases
                    .remove(&(key.clone(), db_name.clone()));
                let ns = self.namespaces.get_mut(&key).expect("selected");
                ns.manifest.databases.remove(&db_name);
                ns.dirty = true;
                Ok(json!({"dropped":true}))
            }
            DatabaseAction::Execute => {
                let conn = self.ensure_working(&key, &db_name, &db)?;
                let result = sql::run(
                    conn,
                    required(args.sql.as_deref(), "sql")?,
                    &args.params,
                    false,
                )?;
                self.namespaces.get_mut(&key).expect("selected").dirty = true;
                Ok(result)
            }
            DatabaseAction::Migrate => {
                let migrations = args
                    .migrations
                    .as_ref()
                    .ok_or_else(|| Error::invalid("migrations must be an array"))?;
                let mut seen = BTreeSet::new();
                let mut pending = Vec::new();
                let mut prefix_position = 0;
                let prefix_mode = migrations
                    .first()
                    .map(|m| m.id.as_str())
                    .is_some_and(|first| db.migrations.iter().any(|m| m.id == first));
                for migration in migrations {
                    let migration_id = name(&migration.id)?;
                    if !seen.insert(migration_id.clone()) {
                        return Err(Error::new("MIGRATION_MISMATCH", "duplicate migration ID"));
                    }
                    let source = migration.sql.as_str();
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
            DatabaseAction::List | DatabaseAction::Create => unreachable!("handled above"),
        }
    }
}

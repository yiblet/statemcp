//! Named application databases, SQL routing, and ordered migration history.
use super::Transaction;
use crate::identity::hash;
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
    pub(super) fn database(&mut self, args: &Value) -> Result<Value> {
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
}

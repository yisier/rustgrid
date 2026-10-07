//! Oracle backup metadata assembly, row streaming and restore.

use rustgrid_core::{BackupObjectKind, Error, ObjectDump, Result, RoutineKind};

use crate::connection::{OracleConnection, map_query_error, run_text_public};
use crate::helpers::{qualify, quote_identifier, render_cell_literal};

/// Rows per `INSERT ALL` statement while restoring a table. `INSERT ALL` is used rather than a
/// multi-row `VALUES` list because the latter only works on Oracle 23ai and later (table value
/// constructor).
const RESTORE_INSERT_BATCH_ROWS: usize = 500;

pub(crate) async fn backup_object_metadata(
    connection: &OracleConnection,
    database: &str,
    kind: BackupObjectKind,
    name: &str,
) -> Result<ObjectDump> {
    match kind {
        BackupObjectKind::Table => {
            let schema_def = crate::schema::table_schema(connection, database, name).await?;
            let (schema, bare) = connection.resolve(name);
            let ddl = crate::schema::table_schema_sql(&schema, &bare, None, &schema_def);
            let fields = schema_def
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect();
            let trigger_ddl = connection
                .with_conn(move |raw| crate::schema::trigger_ddl(raw, &schema, &bare))
                .await
                .unwrap_or_default();
            Ok(ObjectDump {
                name: name.to_string(),
                kind,
                ddl,
                fields,
                trigger_ddl,
                rows: Vec::new(),
            })
        }
        BackupObjectKind::View => Ok(ObjectDump {
            name: name.to_string(),
            kind,
            ddl: crate::view::view_details(connection, database, name)
                .await?
                .definition,
            fields: Vec::new(),
            trigger_ddl: Vec::new(),
            rows: Vec::new(),
        }),
        BackupObjectKind::Function => {
            let definition = match crate::routine::routine_details(
                connection,
                database,
                RoutineKind::Procedure,
                name,
            )
            .await
            {
                Ok(details) => details.definition,
                Err(_) => {
                    crate::routine::routine_details(
                        connection,
                        database,
                        RoutineKind::Function,
                        name,
                    )
                    .await?
                    .definition
                }
            };
            Ok(ObjectDump {
                name: name.to_string(),
                kind,
                ddl: definition,
                fields: Vec::new(),
                trigger_ddl: Vec::new(),
                rows: Vec::new(),
            })
        }
        BackupObjectKind::Event => Ok(ObjectDump {
            name: name.to_string(),
            kind,
            ddl: String::new(),
            fields: Vec::new(),
            trigger_ddl: Vec::new(),
            rows: Vec::new(),
        }),
    }
}

/// Stream a table's rows through a bounded channel so the whole table is never buffered: the
/// blocking task renders one tuple at a time and hands it to the async side.
pub(crate) async fn stream_table_rows(
    connection: &OracleConnection,
    _database: &str,
    table: &str,
    on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
) -> Result<u64> {
    let (schema, bare) = connection.resolve(table);
    let qualified = qualify(&schema, &bare);
    let pool = connection.pool();

    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Result<String>>(64);
    let handle = tokio::task::spawn_blocking(move || {
        let raw = pool.acquire().map_err(map_query_error)?;
        let cursor = raw
            .query(&format!("SELECT * FROM {qualified}"), &[])
            .map_err(map_query_error)?;
        for row in cursor {
            let row = row.map_err(map_query_error)?;
            let count = row.columns().len();
            let mut tuple = String::from("(");
            for index in 0..count {
                if index > 0 {
                    tuple.push_str(", ");
                }
                tuple.push_str(&render_cell_literal(&row, index));
            }
            tuple.push(')');
            // A closed receiver means the caller cancelled: stop fetching.
            if sender.blocking_send(Ok(tuple)).is_err() {
                break;
            }
        }
        Ok::<_, Error>(())
    });

    let mut count = 0u64;
    while let Some(item) = receiver.recv().await {
        let tuple = item?;
        on_row(&tuple)?;
        count += 1;
    }
    handle
        .await
        .map_err(|error| Error::Query(format!("oracle worker panicked: {error}")))??;
    Ok(count)
}

pub(crate) async fn restore_object(
    connection: &OracleConnection,
    _database: &str,
    object: &ObjectDump,
) -> Result<()> {
    let (schema, bare) = connection.resolve(&object.name);
    let object = object.clone();
    connection
        .with_conn(move |raw| {
            let qualified = qualify(&schema, &bare);
            match object.kind {
                BackupObjectKind::Table => {
                    let _ = raw.execute(&format!("DROP TABLE {qualified}"), &[]);
                    let ddl = rewrite_identity(object.ddl.trim().trim_end_matches(';').trim());
                    if ddl.is_empty() {
                        return Err(Error::Query(format!(
                            "no CREATE TABLE statement for {}",
                            object.name
                        )));
                    }
                    run_text_public(raw, &ddl)?;

                    if !object.fields.is_empty() && !object.rows.is_empty() {
                        let columns = object
                            .fields
                            .iter()
                            .map(|field| quote_identifier(field))
                            .collect::<Vec<_>>()
                            .join(", ");
                        for chunk in object.rows.chunks(RESTORE_INSERT_BATCH_ROWS) {
                            // One `INSERT ALL` per batch (works on every Oracle version), instead
                            // of a multi-row `VALUES` list that only 23ai+ accepts.
                            let mut sql = String::from("INSERT ALL");
                            for tuple in chunk {
                                sql.push_str("\nINTO ");
                                sql.push_str(&qualified);
                                sql.push_str(" (");
                                sql.push_str(&columns);
                                sql.push_str(") VALUES ");
                                sql.push_str(tuple);
                            }
                            sql.push_str("\nSELECT 1 FROM dual");
                            raw.execute(&sql, &[]).map_err(map_query_error)?;
                        }
                        let _ = raw.commit();
                    }

                    for trigger in &object.trigger_ddl {
                        run_text_public(raw, trigger)?;
                    }

                    reset_identity(raw, &schema, &bare, &qualified);
                }
                BackupObjectKind::View => {
                    let _ = raw.execute(&format!("DROP VIEW {qualified}"), &[]);
                    if !object.ddl.trim().is_empty() {
                        run_text_public(raw, &object.ddl)?;
                    }
                }
                BackupObjectKind::Function => {
                    if !object.ddl.trim().is_empty() {
                        if let Some(drop) = routine_drop_sql(&schema, &bare, &object.ddl) {
                            let _ = raw.execute(&drop, &[]);
                        }
                        run_text_public(raw, &object.ddl)?;
                    }
                }
                BackupObjectKind::Event => {}
            }
            Ok(())
        })
        .await
}

/// Rewrite `GENERATED ALWAYS AS IDENTITY` to `GENERATED BY DEFAULT AS IDENTITY` so a restore can
/// supply explicit key values.
fn rewrite_identity(ddl: &str) -> String {
    ddl.replace(
        "GENERATED ALWAYS AS IDENTITY",
        "GENERATED BY DEFAULT AS IDENTITY",
    )
    .replace(
        "generated always as identity",
        "GENERATED BY DEFAULT AS IDENTITY",
    )
}

/// The `DROP` statement for a routine's DDL, when its kind can be told from the text.
fn routine_drop_sql(schema: &str, name: &str, ddl: &str) -> Option<String> {
    let upper = ddl.to_ascii_uppercase();
    let keyword = if upper.contains(" PACKAGE ") || upper.starts_with("PACKAGE ") {
        "PACKAGE"
    } else if upper.contains(" PROCEDURE ") {
        "PROCEDURE"
    } else if upper.contains(" FUNCTION ") {
        "FUNCTION"
    } else if upper.contains(" TRIGGER ") {
        "TRIGGER"
    } else {
        return None;
    };
    Some(format!("DROP {keyword} {}", qualify(schema, name)))
}

/// Reset each identity column so subsequent inserts do not collide, tolerating empty tables.
fn reset_identity(raw: &oracledb::Connection, schema: &str, table: &str, qualified: &str) {
    let binds: Vec<String> = vec![schema.to_string(), table.to_string()];
    let refs = crate::connection::bind_refs(&binds);
    let Ok(cursor) = raw.query(
        "SELECT column_name FROM all_tab_columns \
         WHERE owner = :1 AND table_name = :2 AND identity_column = 'YES'",
        &refs,
    ) else {
        return;
    };
    let mut columns = Vec::new();
    for row in cursor {
        let Ok(row) = row else { break };
        if let Ok(Some(name)) = row.get::<Option<String>>(0) {
            columns.push(name);
        }
    }
    for column in columns {
        let sql = format!(
            "ALTER TABLE {qualified} MODIFY ({} GENERATED BY DEFAULT AS IDENTITY (START WITH LIMIT VALUE))",
            quote_identifier(&column)
        );
        let _ = raw.execute(&sql, &[]);
    }
}

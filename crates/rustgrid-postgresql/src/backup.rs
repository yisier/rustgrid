//! PostgreSQL backup metadata assembly, row streaming and restore.

use futures_util::TryStreamExt;
use rustgrid_core::{BackupObjectKind, Error, ObjectDump, Result, RoutineKind};
use sqlx::{AssertSqlSafe, Row};

use crate::connection::{PostgresConnection, map_query_error};
use crate::helpers::{qualify, quote_identifier, render_literal};
use rustgrid_core::Connection;

/// Rows per `INSERT` while restoring a table.
const RESTORE_INSERT_BATCH_ROWS: usize = 1_000;
/// Byte budget per `INSERT` while restoring a table.
const RESTORE_INSERT_BATCH_BYTES: usize = 4 * 1024 * 1024;

pub(crate) async fn backup_object_metadata(
    connection: &PostgresConnection,
    database: &str,
    kind: BackupObjectKind,
    name: &str,
) -> Result<ObjectDump> {
    match kind {
        BackupObjectKind::Table => {
            let schema = connection.table_schema(database, name).await?;
            let ddl = connection.table_schema_sql(database, name, None, &schema);
            let fields = connection
                .columns(database, name)
                .await?
                .into_iter()
                .map(|column| column.name)
                .collect();
            let trigger_ddl = schema
                .triggers
                .iter()
                .map(|trigger| trigger.statement.clone())
                .collect();
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
            ddl: connection.view_details(database, name).await?.definition,
            fields: Vec::new(),
            trigger_ddl: Vec::new(),
            rows: Vec::new(),
        }),
        BackupObjectKind::Function => {
            let definition = match connection
                .routine_details(database, RoutineKind::Procedure, name)
                .await
            {
                Ok(details) => details.definition,
                Err(_) => {
                    connection
                        .routine_details(database, RoutineKind::Function, name)
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

pub(crate) async fn stream_table_rows(
    connection: &PostgresConnection,
    database: &str,
    table: &str,
    on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
) -> Result<u64> {
    let (schema, bare) = connection.resolve_object(database, table).await?;
    let qualified = qualify(&schema, &bare);
    let pool = connection.pool_for(database).await?;
    let mut pinned = pool.acquire().await.map_err(map_query_error)?;
    // The simple query protocol streams text values, which render directly to literals.
    let mut stream =
        sqlx::raw_sql(AssertSqlSafe(format!("SELECT * FROM {qualified}"))).fetch(&mut *pinned);
    let mut count = 0u64;
    let mut column_count = 0usize;
    while let Some(item) = stream.try_next().await.map_err(map_query_error)? {
        if column_count == 0 {
            column_count = item.columns().len();
        }
        let mut tuple = String::from("(");
        for index in 0..column_count {
            if index > 0 {
                tuple.push_str(", ");
            }
            tuple.push_str(&render_literal(&crate::helpers::decode_text_cell(
                &item, index,
            )));
        }
        tuple.push(')');
        on_row(&tuple)?;
        count += 1;
    }
    Ok(count)
}

pub(crate) async fn restore_object(
    connection: &PostgresConnection,
    database: &str,
    object: &ObjectDump,
) -> Result<()> {
    let (schema, bare) = connection
        .resolve_object(database, &object.name)
        .await
        .unwrap_or_else(|_| ("public".to_string(), object.name.clone()));
    let qualified = qualify(&schema, &bare);
    let ddl = object.ddl.trim().trim_end_matches(';').trim().to_string();

    match object.kind {
        BackupObjectKind::Table => {
            if ddl.is_empty() {
                return Err(Error::Query(format!(
                    "no CREATE TABLE statement for {}",
                    object.name
                )));
            }
            connection
                .run_text(Some(database), &format!("DROP TABLE IF EXISTS {qualified}"))
                .await?;
            connection.run_text(Some(database), &ddl).await?;

            let sequences = identity_columns(connection, database, &schema, &bare).await?;
            if !object.fields.is_empty() && !object.rows.is_empty() {
                let columns = object
                    .fields
                    .iter()
                    .map(|field| quote_identifier(field))
                    .collect::<Vec<_>>()
                    .join(", ");
                let overriding = if sequences.iter().any(|(_, identity)| *identity) {
                    " OVERRIDING SYSTEM VALUE"
                } else {
                    ""
                };
                let mut batch: Vec<&str> = Vec::new();
                let mut bytes = 0usize;
                for row in &object.rows {
                    if !batch.is_empty()
                        && (batch.len() >= RESTORE_INSERT_BATCH_ROWS
                            || bytes + row.len() + 2 > RESTORE_INSERT_BATCH_BYTES)
                    {
                        insert_batch(
                            connection, database, &qualified, &columns, overriding, &batch,
                        )
                        .await?;
                        batch.clear();
                        bytes = 0;
                    }
                    bytes += row.len() + 2;
                    batch.push(row);
                }
                if !batch.is_empty() {
                    insert_batch(
                        connection, database, &qualified, &columns, overriding, &batch,
                    )
                    .await?;
                }
            }

            for trigger in &object.trigger_ddl {
                let trigger = trigger.trim().trim_end_matches(';').trim();
                if !trigger.is_empty() {
                    connection.run_text(Some(database), trigger).await?;
                }
            }

            reset_sequences(connection, database, &qualified, &sequences).await?;
        }
        BackupObjectKind::View => {
            connection
                .run_text(Some(database), &format!("DROP VIEW IF EXISTS {qualified}"))
                .await?;
            if !ddl.is_empty() {
                connection.run_text(Some(database), &ddl).await?;
            }
        }
        BackupObjectKind::Function => {
            if !ddl.is_empty() {
                let keyword = if ddl.to_ascii_uppercase().contains(" PROCEDURE ") {
                    "PROCEDURE"
                } else {
                    "FUNCTION"
                };
                // The identity (including argument types) is embedded in `object.name`.
                let identity = routine_identity(&schema, &bare, &ddl);
                let _ = connection
                    .run_text(
                        Some(database),
                        &format!("DROP {keyword} IF EXISTS {identity}"),
                    )
                    .await;
                connection.run_text(Some(database), &ddl).await?;
            }
        }
        BackupObjectKind::Event => {}
    }
    Ok(())
}

/// A `schema.name(args)` identity derived from a routine's DDL, for a best-effort `DROP`.
fn routine_identity(schema: &str, name: &str, ddl: &str) -> String {
    if let Some(open) = ddl.find('(')
        && let Some(close) = ddl[open..].find(')')
    {
        let args = ddl[open + 1..open + close].trim();
        if !args.is_empty() {
            return format!("{}({args})", qualify(schema, name));
        }
    }
    qualify(schema, name)
}

async fn insert_batch(
    connection: &PostgresConnection,
    database: &str,
    qualified: &str,
    columns: &str,
    overriding: &str,
    rows: &[&str],
) -> Result<()> {
    let sql = format!(
        "INSERT INTO {qualified} ({columns}){overriding} VALUES {}",
        rows.join(", ")
    );
    connection.run_text(Some(database), &sql).await.map(|_| ())
}

/// The serial/identity columns of a table: `(column, is_identity)`.
async fn identity_columns(
    connection: &PostgresConnection,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<(String, bool)>> {
    let pool = connection.pool_for(database).await?;
    let rows = sqlx::query(
        "SELECT a.attname, a.attidentity <> '' AS is_identity, \
                pg_get_expr(d.adbin, d.adrelid) LIKE 'nextval(%' AS is_serial \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE n.nspname = $1 AND c.relname = $2 \
           AND a.attnum > 0 AND NOT a.attisdropped",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let name: String = row.try_get(0).ok()?;
            let identity: bool = row.try_get(1).unwrap_or(false);
            let serial: bool = row.try_get(2).unwrap_or(false);
            (identity || serial).then_some((name, identity))
        })
        .collect())
}

/// Reset each sequence so subsequent inserts do not collide, tolerating empty tables.
async fn reset_sequences(
    connection: &PostgresConnection,
    database: &str,
    qualified: &str,
    sequences: &[(String, bool)],
) -> Result<()> {
    for (column, _identity) in sequences {
        let sequence = format!(
            "pg_get_serial_sequence({}, {})",
            crate::helpers::quote_literal(qualified),
            crate::helpers::quote_literal(column)
        );
        let sql = format!(
            "SELECT setval({sequence}, COALESCE(m, 1), m IS NOT NULL) \
             FROM (SELECT max({column}) AS m FROM {qualified}) sub",
            column = quote_identifier(column)
        );
        // A column with no owned sequence (or a non-numeric one) simply skips the reset.
        let _ = connection.run_text(Some(database), &sql).await;
    }
    Ok(())
}

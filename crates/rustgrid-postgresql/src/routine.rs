//! PostgreSQL stored-routine (function/procedure) catalog access.
//!
//! PostgreSQL identifies a routine by `(name, argument types)`, so the driver encodes the schema
//! and the identity arguments into [`rustgrid_core::RoutineInfo::name`], e.g.
//! `public.get_user(integer)`. `list_routines` and `list_routine_infos` return the same string so
//! the backup tree and the Functions tab agree.

use rustgrid_core::{Error, Result, RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};
use sqlx::Row;

use crate::connection::{PostgresConnection, map_query_error};
use crate::helpers::{qualify, split_qualified};

/// A parsed routine identity: `schema.name(args)`.
struct RoutineRef {
    schema: String,
    name: String,
    args: String,
}

/// Parse a `schema.name(args)` / `name(args)` identity into its parts.
fn parse_routine(identity: &str) -> RoutineRef {
    let (head, args) = match identity.split_once('(') {
        Some((head, rest)) => (head.trim(), rest.trim_end_matches(')').trim().to_string()),
        None => (identity.trim(), String::new()),
    };
    let (schema, name) = split_qualified(head)
        .map(|(schema, name)| (schema.to_string(), name.to_string()))
        .unwrap_or_else(|| ("public".to_string(), head.to_string()));
    RoutineRef { schema, name, args }
}

/// The `schema.name(args)` identity for a routine.
fn routine_identity(schema: &str, name: &str, args: &str) -> String {
    format!("{schema}.{name}({args})")
}

/// The `"schema"."name"(args)` SQL name, used in `DROP`.
fn routine_sql_name(reference: &RoutineRef) -> String {
    if reference.args.is_empty() {
        qualify(&reference.schema, &reference.name)
    } else {
        format!(
            "{}({})",
            qualify(&reference.schema, &reference.name),
            reference.args
        )
    }
}

async fn routine_rows(
    connection: &PostgresConnection,
    database: &str,
) -> Result<Vec<(String, RoutineKind, String, String, String, String, String)>> {
    let pool = connection.pool_for(database).await?;
    let rows = sqlx::query(
        "SELECT n.nspname, p.proname, p.prokind, \
                pg_get_function_identity_arguments(p.oid), \
                pg_get_function_result(p.oid), \
                COALESCE(obj_description(p.oid, 'pg_proc'), ''), \
                p.provolatile, p.prosecdef \
         FROM pg_proc p \
         JOIN pg_namespace n ON n.oid = p.pronamespace \
         WHERE p.prokind IN ('f','p') \
           AND n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema' \
         ORDER BY n.nspname, p.proname",
    )
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;

    Ok(rows
        .iter()
        .map(|row| {
            let schema: String = row.try_get(0).unwrap_or_default();
            let name: String = row.try_get(1).unwrap_or_default();
            let prokind: String = row.try_get(2).unwrap_or_default();
            let args: String = row.try_get(3).unwrap_or_default();
            let result: String = row.try_get(4).unwrap_or_default();
            let comment: String = row.try_get(5).unwrap_or_default();
            let volatile: String = row.try_get(6).unwrap_or_default();
            let secdef: bool = row.try_get(7).unwrap_or(false);
            let kind = if prokind == "p" {
                RoutineKind::Procedure
            } else {
                RoutineKind::Function
            };
            (
                routine_identity(&schema, &name, &args),
                kind,
                result,
                comment,
                volatile,
                if secdef {
                    "DEFINER".to_string()
                } else {
                    "INVOKER".to_string()
                },
                schema,
            )
        })
        .collect())
}

pub(crate) async fn list_routines(
    connection: &PostgresConnection,
    database: &str,
) -> Result<Vec<String>> {
    Ok(routine_rows(connection, database)
        .await?
        .into_iter()
        .map(|row| row.0)
        .collect())
}

pub(crate) async fn list_routine_infos(
    connection: &PostgresConnection,
    database: &str,
) -> Result<Vec<RoutineInfo>> {
    Ok(routine_rows(connection, database)
        .await?
        .into_iter()
        .map(
            |(name, kind, result, comment, volatile, security, _)| RoutineInfo {
                name,
                kind,
                comment,
                return_type: if kind == RoutineKind::Function {
                    result
                } else {
                    String::new()
                },
                definer: String::new(),
                deterministic: volatile == "i",
                data_access: String::new(),
                security_type: security,
                created: None,
                modified: None,
            },
        )
        .collect())
}

pub(crate) async fn routine_details(
    connection: &PostgresConnection,
    database: &str,
    kind: RoutineKind,
    name: &str,
) -> Result<RoutineDetails> {
    let reference = parse_routine(name);
    let pool = connection.pool_for(database).await?;
    let row = sqlx::query(
        "SELECT pg_get_functiondef(p.oid) \
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = $1 AND p.proname = $2 \
           AND ($3 = '' OR pg_get_function_identity_arguments(p.oid) = $3) \
         ORDER BY p.oid LIMIT 1",
    )
    .bind(&reference.schema)
    .bind(&reference.name)
    .bind(&reference.args)
    .fetch_optional(&pool)
    .await
    .map_err(map_query_error)?;
    let definition = row
        .and_then(|row| row.try_get::<String, _>(0).ok())
        .ok_or_else(|| Error::Query(format!("routine {name} not found")))?;
    Ok(RoutineDetails {
        info: RoutineInfo {
            name: name.to_string(),
            kind,
            ..Default::default()
        },
        definition,
        sql_mode: String::new(),
        character_set_client: String::new(),
        collation_connection: String::new(),
        database_collation: String::new(),
    })
}

pub(crate) fn routine_sql(original: Option<(&str, RoutineKind)>, edit: &RoutineEdit) -> String {
    let mut statements = Vec::new();
    if let Some((name, kind)) = original {
        let reference = parse_routine(name);
        statements.push(format!(
            "DROP {} IF EXISTS {}",
            kind.sql_name(),
            routine_sql_name(&reference)
        ));
    }
    let definition = edit.definition.trim().trim_end_matches(';').trim();
    if !definition.is_empty() {
        statements.push(definition.to_string());
    }
    statements.join(";\n")
}

pub(crate) async fn save_routine(
    connection: &PostgresConnection,
    database: &str,
    original: Option<(&str, RoutineKind)>,
    edit: &RoutineEdit,
) -> Result<()> {
    if let Some((name, kind)) = original {
        let reference = parse_routine(name);
        let drop = format!(
            "DROP {} IF EXISTS {}",
            kind.sql_name(),
            routine_sql_name(&reference)
        );
        connection.run_text(Some(database), &drop).await?;
    }
    let definition = edit.definition.trim().trim_end_matches(';').trim();
    if !definition.is_empty() {
        connection.run_text(Some(database), definition).await?;
    }
    Ok(())
}

pub(crate) async fn drop_routine(
    connection: &PostgresConnection,
    database: &str,
    kind: RoutineKind,
    name: &str,
) -> Result<()> {
    let reference = parse_routine(name);
    let sql = format!("DROP {} {}", kind.sql_name(), routine_sql_name(&reference));
    connection.run_text(Some(database), &sql).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qualified_routine_identities() {
        let reference = parse_routine("public.get_user(integer)");
        assert_eq!(reference.schema, "public");
        assert_eq!(reference.name, "get_user");
        assert_eq!(reference.args, "integer");
        assert_eq!(
            routine_sql_name(&reference),
            "\"public\".\"get_user\"(integer)"
        );

        let bare = parse_routine("f()");
        assert_eq!(bare.schema, "public");
        assert_eq!(bare.name, "f");
        assert_eq!(routine_sql_name(&bare), "\"public\".\"f\"");
    }
}

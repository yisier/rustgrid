//! PostgreSQL view catalog access and `CREATE`/`DROP` generation.

use rustgrid_core::{Error, Result, ViewDetails, ViewEdit, ViewInfo};
use sqlx::Row;

use crate::connection::{PostgresConnection, map_query_error};
use crate::helpers::{leading_keyword, qualify, split_qualified};

/// The bare `SELECT` definition of a view (`pg_get_viewdef`), or `None` when it does not exist.
pub(crate) async fn view_definition(
    connection: &PostgresConnection,
    database: &str,
    name: &str,
) -> Result<Option<String>> {
    let (schema, bare) = connection.resolve_object(database, name).await?;
    let pool = connection.pool_for(database).await?;
    let row = sqlx::query(
        "SELECT pg_get_viewdef(c.oid, true) \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('v','m')",
    )
    .bind(&schema)
    .bind(&bare)
    .fetch_optional(&pool)
    .await
    .map_err(map_query_error)?;
    Ok(row.and_then(|row| row.try_get::<String, _>(0).ok()))
}

/// Build the full `CREATE VIEW` statement for a view.
pub(crate) fn create_view_statement(schema: &str, name: &str, definition: &str) -> String {
    format!(
        "CREATE VIEW {} AS\n{}",
        qualify(schema, name),
        definition.trim()
    )
}

pub(crate) async fn view_details(
    connection: &PostgresConnection,
    database: &str,
    name: &str,
) -> Result<ViewDetails> {
    let (schema, bare) = connection.resolve_object(database, name).await?;
    let definition = view_definition(connection, database, name)
        .await?
        .ok_or_else(|| Error::Query(format!("view {name} not found")))?;

    let pool = connection.pool_for(database).await?;
    let updatable = sqlx::query(
        "SELECT is_updatable FROM information_schema.views \
         WHERE table_schema = $1 AND table_name = $2",
    )
    .bind(&schema)
    .bind(&bare)
    .fetch_optional(&pool)
    .await
    .map_err(map_query_error)?
    .and_then(|row| row.try_get::<String, _>(0).ok())
    .is_some_and(|value| value.eq_ignore_ascii_case("YES"));

    Ok(ViewDetails {
        info: ViewInfo {
            name: name.to_string(),
            updatable,
            ..Default::default()
        },
        definition: create_view_statement(&schema, &bare, &definition),
        character_set_client: String::new(),
        collation_connection: String::new(),
    })
}

/// The view name declared by a `CREATE [OR REPLACE] [MATERIALIZED] VIEW <name>` statement, as a
/// `schema.name` string when qualified.
pub(crate) fn view_identity(definition: &str) -> Option<String> {
    let upper = definition.to_ascii_uppercase();
    let index = upper.find("VIEW")?;
    let rest = definition[index + 4..].trim_start();
    let mut name = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(&character) = chars.peek() {
        if character == '"' {
            chars.next();
            for inner in chars.by_ref() {
                if inner == '"' {
                    break;
                }
                name.push(inner);
            }
        } else if character.is_alphanumeric() || character == '_' || character == '.' {
            name.push(character);
            chars.next();
        } else {
            break;
        }
    }
    (!name.is_empty()).then_some(name)
}

pub(crate) fn view_sql(original: Option<&str>, edit: &ViewEdit) -> String {
    let mut statements = Vec::new();
    if let Some(original) = original
        && let Some((schema, bare)) = split_qualified(original)
    {
        statements.push(format!("DROP VIEW IF EXISTS {}", qualify(schema, bare)));
    }
    let definition = edit.definition.trim().trim_end_matches(';').trim();
    if !definition.is_empty() {
        statements.push(definition.to_string());
    }
    statements.join(";\n")
}

pub(crate) async fn save_view(
    connection: &PostgresConnection,
    database: &str,
    original: Option<&str>,
    edit: &ViewEdit,
) -> Result<()> {
    let definition = edit
        .definition
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string();
    if definition.is_empty() {
        return Ok(());
    }
    if leading_keyword(&definition) != "CREATE" {
        // Not a `CREATE VIEW` statement; run it as-is.
        return connection
            .run_text(Some(database), &definition)
            .await
            .map(|_| ());
    }

    // `CREATE OR REPLACE VIEW` only works when the column set is unchanged. Try it, and fall back
    // to dropping the view first when PostgreSQL rejects the replacement.
    if let Err(_first) = connection.run_text(Some(database), &definition).await {
        let name = original
            .map(str::to_string)
            .or_else(|| view_identity(&definition));
        if let Some(name) = name {
            let (schema, bare) = connection
                .resolve_object(database, &name)
                .await
                .unwrap_or_else(|_| ("public".to_string(), name.clone()));
            let drop = format!("DROP VIEW IF EXISTS {}", qualify(&schema, &bare));
            connection.run_text(Some(database), &drop).await?;
        }
        connection.run_text(Some(database), &definition).await?;
    }
    Ok(())
}

pub(crate) async fn drop_view(
    connection: &PostgresConnection,
    database: &str,
    name: &str,
) -> Result<()> {
    let (schema, bare) = connection.resolve_object(database, name).await?;
    let sql = format!("DROP VIEW {}", qualify(&schema, &bare));
    connection.run_text(Some(database), &sql).await.map(|_| ())
}

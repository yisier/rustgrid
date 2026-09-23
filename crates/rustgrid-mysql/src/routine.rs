//! MySQL stored routine (function / procedure) administration, backing the Functions main tab and
//! its editor.
//!
//! Routines are listed from `information_schema.routines`; the full definition comes from
//! `SHOW CREATE FUNCTION|PROCEDURE`. Saves replay `DROP ... IF EXISTS` plus the routine's own
//! `CREATE` statement through the text protocol, because MySQL has no `CREATE OR REPLACE` for
//! routines.

use rustgrid_core::{Error, Result, RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};
use sqlx::mysql::MySqlRow;
use sqlx::{AssertSqlSafe, MySqlPool};

use crate::connection::{column_text, map_query_error, quote_identifier};

/// The routine columns the list and 信息 tab read.
const ROUTINE_COLUMNS: &str = "routine_name, routine_type, routine_comment, data_type, definer, \
     is_deterministic, sql_data_access, security_type, created, last_altered";

/// Every stored routine of a database, ordered by name.
pub(crate) async fn list_routine_infos(
    pool: &MySqlPool,
    database: &str,
) -> Result<Vec<RoutineInfo>> {
    let sql = format!(
        "SELECT {ROUTINE_COLUMNS} FROM information_schema.routines \
         WHERE routine_schema = ? ORDER BY routine_name"
    );
    let rows = sqlx::query(AssertSqlSafe(sql))
        .bind(database)
        .fetch_all(pool)
        .await
        .map_err(map_query_error)?;
    Ok(rows.iter().map(read_routine_info).collect())
}

/// One routine's metadata.
async fn routine_info(
    pool: &MySqlPool,
    database: &str,
    kind: RoutineKind,
    name: &str,
) -> Result<Option<RoutineInfo>> {
    let sql = format!(
        "SELECT {ROUTINE_COLUMNS} FROM information_schema.routines \
         WHERE routine_schema = ? AND routine_name = ? AND routine_type = ?"
    );
    let row = sqlx::query(AssertSqlSafe(sql))
        .bind(database)
        .bind(name)
        .bind(kind.sql_name())
        .fetch_optional(pool)
        .await
        .map_err(map_query_error)?;
    Ok(row.as_ref().map(read_routine_info))
}

/// A routine's full definition and session settings.
pub(crate) async fn routine_details(
    pool: &MySqlPool,
    database: &str,
    kind: RoutineKind,
    name: &str,
) -> Result<RoutineDetails> {
    let verb = kind.sql_name();
    let sql = format!(
        "SHOW CREATE {verb} {}.{}",
        quote_identifier(database),
        quote_identifier(name)
    );
    // `SHOW` is not supported by the prepared-statement protocol, so this must go through the
    // text protocol on an acquired connection.
    let mut connection = pool.acquire().await.map_err(map_query_error)?;
    let row = sqlx::raw_sql(AssertSqlSafe(sql))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_query_error)?
        .ok_or_else(|| Error::Query(format!("{verb} {name} does not exist")))?;
    drop(connection);

    let definition =
        column_text(&row, &["Create Function", "Create Procedure", "Create"]).unwrap_or_default();
    let info = routine_info(pool, database, kind, name)
        .await?
        .unwrap_or_else(|| RoutineInfo {
            name: name.to_string(),
            kind,
            ..RoutineInfo::default()
        });

    Ok(RoutineDetails {
        info,
        definition,
        sql_mode: column_text(&row, &["sql_mode", "Sql Mode"]).unwrap_or_default(),
        character_set_client: column_text(&row, &["character_set_client"]).unwrap_or_default(),
        collation_connection: column_text(&row, &["collation_connection"]).unwrap_or_default(),
        database_collation: column_text(&row, &["Database Collation"]).unwrap_or_default(),
    })
}

/// The SQL script [`save_routine`] runs, for the editor's SQL preview.
pub(crate) fn routine_sql(
    database: &str,
    original: Option<(&str, RoutineKind)>,
    edit: &RoutineEdit,
) -> String {
    let mut script = String::new();
    if let Some((old_name, old_kind)) = original {
        script.push_str(&drop_statement(database, old_kind, old_name));
        script.push('\n');
    }
    let definition = edit.definition.trim().trim_end_matches(';');
    script.push_str(definition);
    script.push(';');
    script
}

/// Create or replace a routine: drop the original (MySQL cannot replace in place) and run the
/// editor's `CREATE` statement in the database's context.
pub(crate) async fn save_routine(
    pool: &MySqlPool,
    database: &str,
    original: Option<(&str, RoutineKind)>,
    edit: &RoutineEdit,
) -> Result<()> {
    let definition = edit.definition.trim().trim_end_matches(';').trim();
    if definition.is_empty() {
        return Err(Error::Query("the routine definition is empty".to_string()));
    }
    if let Some((old_name, old_kind)) = original {
        exec(pool, &drop_statement(database, old_kind, old_name)).await?;
    }
    // A single script keeps `USE` and the `CREATE` on the same pinned connection.
    let script = format!("USE {};\n{definition}", quote_identifier(database));
    exec(pool, &script).await
}

/// Drop one stored routine.
pub(crate) async fn drop_routine(
    pool: &MySqlPool,
    database: &str,
    kind: RoutineKind,
    name: &str,
) -> Result<()> {
    exec(pool, &drop_statement(database, kind, name)).await
}

fn drop_statement(database: &str, kind: RoutineKind, name: &str) -> String {
    format!(
        "DROP {} IF EXISTS {}.{};",
        kind.sql_name(),
        quote_identifier(database),
        quote_identifier(name)
    )
}

async fn exec(pool: &MySqlPool, sql: &str) -> Result<()> {
    sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
        .execute(pool)
        .await
        .map_err(map_query_error)?;
    Ok(())
}

// ----- Reading -------------------------------------------------------------------------------

fn read_routine_info(row: &MySqlRow) -> RoutineInfo {
    let kind = column_text(row, &["routine_type"])
        .and_then(|value| RoutineKind::from_sql_name(&value))
        .unwrap_or_default();
    RoutineInfo {
        name: column_text(row, &["routine_name"]).unwrap_or_default(),
        kind,
        comment: column_text(row, &["routine_comment"]).unwrap_or_default(),
        return_type: column_text(row, &["data_type"]).unwrap_or_default(),
        definer: column_text(row, &["definer"]).unwrap_or_default(),
        deterministic: column_text(row, &["is_deterministic"])
            .is_some_and(|value| value.eq_ignore_ascii_case("YES")),
        data_access: column_text(row, &["sql_data_access"]).unwrap_or_default(),
        security_type: column_text(row, &["security_type"]).unwrap_or_default(),
        created: column_text(row, &["created"]),
        modified: column_text(row, &["last_altered", "modified"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_drops_the_original_then_creates_the_edit() {
        let edit = RoutineEdit {
            kind: RoutineKind::Procedure,
            name: "p".to_string(),
            definition: "CREATE PROCEDURE `p`()\nBEGIN\nEND".to_string(),
        };
        let sql = routine_sql("db", Some(("old", RoutineKind::Procedure)), &edit);
        assert!(sql.contains("DROP PROCEDURE IF EXISTS `db`.`old`"));
        assert!(sql.contains("CREATE PROCEDURE `p`()"));

        let sql = routine_sql("db", None, &edit);
        assert!(!sql.contains("DROP"));
        assert!(sql.ends_with(';'));
    }
}

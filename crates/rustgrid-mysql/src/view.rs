//! MySQL view administration, backing the Views main tab and its designer.
//!
//! A view's full definition comes from `SHOW CREATE VIEW`; its creation attributes live in
//! `information_schema.views`. Saves replay `DROP VIEW IF EXISTS` plus the view's own `CREATE`
//! statement through the text protocol, so renaming a view keeps working.

use rustgrid_core::{Error, Result, ViewDetails, ViewEdit, ViewInfo};
use sqlx::{AssertSqlSafe, MySqlPool};

use crate::connection::{column_text, map_query_error, quote_identifier};

/// One view's full definition and creation settings.
pub(crate) async fn view_details(
    pool: &MySqlPool,
    database: &str,
    name: &str,
) -> Result<ViewDetails> {
    let sql = format!(
        "SHOW CREATE VIEW {}.{}",
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
        .ok_or_else(|| Error::Query(format!("view {name} does not exist")))?;
    drop(connection);

    let definition = column_text(&row, &["Create View", "Create"]).unwrap_or_default();
    let mut info = view_info(pool, database, name)
        .await?
        .unwrap_or_else(|| ViewInfo {
            name: name.to_string(),
            ..ViewInfo::default()
        });
    if info.algorithm.is_empty() {
        info.algorithm = parse_algorithm(&definition);
    }

    Ok(ViewDetails {
        info,
        definition,
        character_set_client: column_text(&row, &["character_set_client"]).unwrap_or_default(),
        collation_connection: column_text(&row, &["collation_connection"]).unwrap_or_default(),
    })
}

/// One view's metadata from `information_schema.views` plus its timestamps.
async fn view_info(pool: &MySqlPool, database: &str, name: &str) -> Result<Option<ViewInfo>> {
    let row = sqlx::query(
        "SELECT v.definer, v.security_type, v.check_option, v.is_updatable, \
                t.create_time, t.update_time \
         FROM information_schema.views AS v \
         LEFT JOIN information_schema.tables AS t \
           ON t.table_schema = v.table_schema AND t.table_name = v.table_name \
         WHERE v.table_schema = ? AND v.table_name = ?",
    )
    .bind(database)
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(map_query_error)?;
    Ok(row.as_ref().map(|row| ViewInfo {
        name: name.to_string(),
        definer: column_text(row, &["definer"]).unwrap_or_default(),
        security_type: column_text(row, &["security_type"]).unwrap_or_default(),
        algorithm: String::new(),
        check_option: column_text(row, &["check_option"]).unwrap_or_default(),
        updatable: column_text(row, &["is_updatable"])
            .is_some_and(|value| value.eq_ignore_ascii_case("YES")),
        created: column_text(row, &["create_time", "created"]),
        modified: column_text(row, &["update_time", "modified"]),
    }))
}

/// The SQL script [`save_view`] runs, for the designer's SQL 预览 page.
pub(crate) fn view_sql(database: &str, original: Option<&str>, edit: &ViewEdit) -> String {
    let mut script = String::new();
    if let Some(old_name) = original {
        script.push_str(&drop_view_statement(database, old_name));
        script.push('\n');
    }
    let definition = edit.definition.trim().trim_end_matches(';');
    script.push_str(definition);
    script.push(';');
    script
}

/// Create or replace a view: drop the original (so a rename also takes effect) and run the
/// designer's `CREATE` statement in the database's context.
pub(crate) async fn save_view(
    pool: &MySqlPool,
    database: &str,
    original: Option<&str>,
    edit: &ViewEdit,
) -> Result<()> {
    let definition = edit.definition.trim().trim_end_matches(';').trim();
    if definition.is_empty() {
        return Err(Error::Query("the view definition is empty".to_string()));
    }
    if let Some(old_name) = original {
        exec(pool, &drop_view_statement(database, old_name)).await?;
    }
    // A single script keeps `USE` and the `CREATE` on the same pinned connection.
    let script = format!("USE {};\n{definition}", quote_identifier(database));
    exec(pool, &script).await
}

/// Drop one view.
pub(crate) async fn drop_view(pool: &MySqlPool, database: &str, name: &str) -> Result<()> {
    exec(pool, &drop_view_statement(database, name)).await
}

fn drop_view_statement(database: &str, name: &str) -> String {
    format!(
        "DROP VIEW IF EXISTS {}.{};",
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

/// The `ALGORITHM` of a `CREATE [ALGORITHM=...] ... VIEW` statement, defaulting to `UNDEFINED`
/// when the server omits it.
fn parse_algorithm(definition: &str) -> String {
    let upper = definition.to_ascii_uppercase();
    let Some(position) = upper.find("ALGORITHM") else {
        return "UNDEFINED".to_string();
    };
    let rest = &definition[position + "ALGORITHM".len()..];
    let rest = rest.trim_start().trim_start_matches('=').trim_start();
    let algorithm: String = rest
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect();
    if algorithm.is_empty() {
        "UNDEFINED".to_string()
    } else {
        algorithm.to_ascii_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_drops_the_original_then_creates_the_edit() {
        let edit = ViewEdit {
            name: "v".to_string(),
            definition: "CREATE VIEW `v` AS SELECT 1".to_string(),
        };
        let sql = view_sql("db", Some("old"), &edit);
        assert!(sql.contains("DROP VIEW IF EXISTS `db`.`old`"));
        assert!(sql.contains("CREATE VIEW `v` AS SELECT 1"));

        let sql = view_sql("db", None, &edit);
        assert!(!sql.contains("DROP"));
        assert!(sql.ends_with(';'));
    }

    #[test]
    fn reads_the_algorithm_from_the_definition() {
        assert_eq!(
            parse_algorithm(
                "CREATE ALGORITHM=MERGE DEFINER=`root`@`localhost` VIEW `v` AS SELECT 1"
            ),
            "MERGE"
        );
        assert_eq!(parse_algorithm("CREATE VIEW `v` AS SELECT 1"), "UNDEFINED");
    }
}

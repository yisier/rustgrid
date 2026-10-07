//! Oracle view catalog access and `CREATE`/`DROP` generation.

use rustgrid_core::{Error, Result, ViewDetails, ViewEdit, ViewInfo};

use crate::connection::{OracleConnection, fetch_ddl, run_text_public};
use crate::helpers::{qualify, quote_identifier, split_qualified};

/// The view name declared by a `CREATE [OR REPLACE] [FORCE | NO FORCE] VIEW [schema.]name`
/// statement, as a `schema.name` string when qualified.
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
        } else if character.is_alphanumeric()
            || character == '_'
            || character == '.'
            || character == '$'
        {
            name.push(character);
            chars.next();
        } else {
            break;
        }
    }
    (!name.is_empty()).then_some(name)
}

pub(crate) async fn view_details(
    connection: &OracleConnection,
    _database: &str,
    name: &str,
) -> Result<ViewDetails> {
    let (schema, bare) = connection.resolve(name);
    let name = name.to_string();
    connection
        .with_conn(move |raw| {
            let definition = fetch_ddl(raw, "VIEW", &bare, &schema)?
                .ok_or_else(|| Error::Query(format!("view {name} not found")))?;
            Ok(ViewDetails {
                info: ViewInfo {
                    name: name.clone(),
                    updatable: false,
                    ..Default::default()
                },
                definition,
                character_set_client: String::new(),
                collation_connection: String::new(),
            })
        })
        .await
}

pub(crate) fn view_sql(original: Option<&str>, edit: &ViewEdit) -> String {
    let mut statements = Vec::new();
    if let Some(original) = original {
        let (schema, bare) = match split_qualified(original) {
            Some((schema, bare)) => (schema.to_string(), bare.to_string()),
            None => (String::new(), original.to_string()),
        };
        let qualified = if schema.is_empty() {
            quote_identifier(&bare)
        } else {
            qualify(&schema, &bare)
        };
        statements.push(format!("DROP VIEW {qualified}"));
    }
    let definition = edit.definition.trim().trim_end_matches(';').trim();
    if !definition.is_empty() {
        statements.push(definition.to_string());
    }
    statements.join(";\n")
}

pub(crate) async fn save_view(
    connection: &OracleConnection,
    _database: &str,
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
    let original = original.map(str::to_string);
    connection
        .with_conn(move |raw| {
            // Oracle can create or replace a view in place as long as the column set is compatible.
            // Only when that is rejected does the existing view need dropping first — but a
            // genuine error (syntax, permissions) must not cost the original view, so its current
            // definition is captured and restored when the replacement cannot be created.
            if run_text_public(raw, &definition).is_ok() {
                return Ok(());
            }
            let name = original
                .clone()
                .or_else(|| view_identity(&definition))
                .ok_or_else(|| {
                    Error::Query("cannot determine the view name to replace".to_string())
                })?;
            let (schema, bare) = match split_qualified(&name) {
                Some((schema, bare)) => (schema.to_string(), bare.to_string()),
                None => (String::new(), name.clone()),
            };
            let qualified = if schema.is_empty() {
                quote_identifier(&bare)
            } else {
                qualify(&schema, &bare)
            };
            let previous = fetch_ddl(raw, "VIEW", &bare, &schema).ok().flatten();
            let _ = raw.execute(&format!("DROP VIEW {qualified}"), &[]);
            match run_text_public(raw, &definition) {
                Ok(()) => Ok(()),
                Err(error) => {
                    if let Some(previous) = previous {
                        let _ = run_text_public(raw, &previous);
                    }
                    Err(error)
                }
            }
        })
        .await
}

pub(crate) async fn drop_view(
    connection: &OracleConnection,
    _database: &str,
    name: &str,
) -> Result<()> {
    let (schema, bare) = connection.resolve(name);
    let sql = format!("DROP VIEW {}", qualify(&schema, &bare));
    connection
        .with_conn(move |raw| run_text_public(raw, &sql))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_view_identity() {
        assert_eq!(
            view_identity("CREATE OR REPLACE VIEW \"APP\".\"V\" AS SELECT 1 FROM dual"),
            Some("APP.V".to_string())
        );
        assert_eq!(
            view_identity("CREATE VIEW V AS SELECT 1 FROM dual"),
            Some("V".to_string())
        );
        assert_eq!(view_identity("SELECT 1 FROM dual"), None);
    }

    #[test]
    fn builds_drop_and_create_preview() {
        let edit = ViewEdit {
            name: "APP.V".to_string(),
            definition: "CREATE OR REPLACE VIEW \"APP\".\"V\" AS SELECT 1 FROM dual".to_string(),
        };
        let sql = view_sql(Some("APP.V"), &edit);
        assert!(sql.starts_with("DROP VIEW \"APP\".\"V\";"));
        assert!(sql.contains("CREATE OR REPLACE VIEW"));
    }
}

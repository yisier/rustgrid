//! Oracle stored-routine catalog access. Oracle identifies a routine by `(owner, name)`, so the
//! driver encodes the schema into [`RoutineInfo::name`] as `OWNER.NAME`. `list_routines` and
//! `list_routine_infos` return the same names so the backup tree and the Functions tab agree.

use rustgrid_core::{Error, Result, RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};

use crate::connection::{OracleConnection, fetch_ddl, map_query_error};
use crate::helpers::{qualify, quote_identifier, split_qualified};

/// The Oracle object types the Functions tab lists (packages and triggers included).
const ROUTINE_TYPES: &str = "('FUNCTION','PROCEDURE','PACKAGE','TRIGGER')";

/// Map an `ALL_OBJECTS.OBJECT_TYPE` to the core routine kind (packages behave like functions).
fn kind_of(object_type: &str) -> RoutineKind {
    if object_type.eq_ignore_ascii_case("PROCEDURE") || object_type.eq_ignore_ascii_case("TRIGGER")
    {
        RoutineKind::Procedure
    } else {
        RoutineKind::Function
    }
}

/// The `ALL_OBJECTS.OBJECT_TYPE`s present for `owner.name`, in the order the driver renders them.
fn object_types(connection: &oracledb::Connection, owner: &str, name: &str) -> Result<Vec<String>> {
    let binds: Vec<String> = vec![owner.to_string(), name.to_string()];
    let refs = crate::connection::bind_refs(&binds);
    let cursor = connection
        .query(
            "SELECT DISTINCT object_type FROM all_objects \
             WHERE owner = :1 AND object_name = :2 \
               AND object_type IN ('FUNCTION','PROCEDURE','PACKAGE','PACKAGE BODY','TRIGGER')",
            &refs,
        )
        .map_err(map_query_error)?;
    let mut types = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        if let Ok(Some(value)) = row.get::<Option<String>>(0) {
            types.push(value);
        }
    }
    // Render the spec before its body, and functions/procedures before packages.
    types.sort_by_key(|value| match value.as_str() {
        "FUNCTION" => 0,
        "PROCEDURE" => 1,
        "PACKAGE" => 2,
        "PACKAGE BODY" => 3,
        "TRIGGER" => 4,
        _ => 5,
    });
    Ok(types)
}

/// The `DBMS_METADATA` object-type string for an `ALL_OBJECTS` type.
fn metadata_type(object_type: &str) -> &'static str {
    match object_type.to_ascii_uppercase().as_str() {
        "FUNCTION" => "FUNCTION",
        "PROCEDURE" => "PROCEDURE",
        "PACKAGE" => "PACKAGE",
        "PACKAGE BODY" => "PACKAGE_BODY",
        "TRIGGER" => "TRIGGER",
        _ => "FUNCTION",
    }
}

/// The `DROP` keyword for an object type.
fn drop_keyword(object_type: &str) -> &'static str {
    match object_type.to_ascii_uppercase().as_str() {
        "PROCEDURE" => "PROCEDURE",
        "PACKAGE" | "PACKAGE BODY" => "PACKAGE",
        "TRIGGER" => "TRIGGER",
        _ => "FUNCTION",
    }
}

/// Every routine's `OWNER.NAME` plus its kind, for the list and the backup tree.
fn routine_rows(connection: &oracledb::Connection) -> Result<Vec<(String, String, RoutineKind)>> {
    let cursor = connection
        .query(
            &format!(
                "SELECT owner, object_name, object_type FROM all_objects \
                 WHERE object_type IN {ROUTINE_TYPES} \
                 ORDER BY owner, object_name"
            ),
            &[],
        )
        .map_err(map_query_error)?;
    let mut rows = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        let owner = row
            .get::<Option<String>>(0)
            .ok()
            .flatten()
            .unwrap_or_default();
        let name = row
            .get::<Option<String>>(1)
            .ok()
            .flatten()
            .unwrap_or_default();
        let object_type = row
            .get::<Option<String>>(2)
            .ok()
            .flatten()
            .unwrap_or_default();
        if owner.is_empty() || name.is_empty() {
            continue;
        }
        rows.push((format!("{owner}.{name}"), owner, kind_of(&object_type)));
    }
    Ok(rows)
}

pub(crate) async fn list_routines(
    connection: &OracleConnection,
    _database: &str,
) -> Result<Vec<String>> {
    connection
        .with_conn(|raw| Ok(routine_rows(raw)?.into_iter().map(|row| row.0).collect()))
        .await
}

pub(crate) async fn list_routine_infos(
    connection: &OracleConnection,
    _database: &str,
) -> Result<Vec<RoutineInfo>> {
    connection
        .with_conn(|raw| {
            Ok(routine_rows(raw)?
                .into_iter()
                .map(|(name, _owner, kind)| RoutineInfo {
                    name,
                    kind,
                    return_type: String::new(),
                    ..Default::default()
                })
                .collect())
        })
        .await
}

pub(crate) async fn routine_details(
    connection: &OracleConnection,
    _database: &str,
    kind: RoutineKind,
    name: &str,
) -> Result<RoutineDetails> {
    let (owner, bare) = match split_qualified(name) {
        Some((owner, bare)) => (owner.to_string(), bare.to_string()),
        None => (
            connection.default_schema_name().to_string(),
            name.to_string(),
        ),
    };
    let name = name.to_string();
    connection
        .with_conn(move |raw| {
            let types = object_types(raw, &owner, &bare)?;
            let mut parts = Vec::new();
            for object_type in &types {
                if let Some(ddl) = fetch_ddl(raw, metadata_type(object_type), &bare, &owner)? {
                    parts.push(ddl);
                }
            }
            if parts.is_empty() {
                return Err(Error::Query(format!("routine {name} not found")));
            }
            Ok(RoutineDetails {
                info: RoutineInfo {
                    name: name.clone(),
                    kind,
                    ..Default::default()
                },
                definition: parts.join(";\n"),
                sql_mode: String::new(),
                character_set_client: String::new(),
                collation_connection: String::new(),
                database_collation: String::new(),
            })
        })
        .await
}

pub(crate) fn routine_sql(original: Option<(&str, RoutineKind)>, edit: &RoutineEdit) -> String {
    let mut statements = Vec::new();
    if let Some((name, kind)) = original {
        let (owner, bare) = match split_qualified(name) {
            Some((owner, bare)) => (owner.to_string(), bare.to_string()),
            None => (String::new(), name.to_string()),
        };
        let qualified = if owner.is_empty() {
            quote_identifier(&bare)
        } else {
            qualify(&owner, &bare)
        };
        // `DROP` is unconditional: Oracle has no `DROP ... IF EXISTS`.
        statements.push(format!("DROP {} {qualified}", kind.sql_name()));
    }
    let definition = edit.definition.trim().trim_end_matches(';').trim();
    if !definition.is_empty() {
        statements.push(definition.to_string());
    }
    statements.join(";\n")
}

pub(crate) async fn save_routine(
    connection: &OracleConnection,
    _database: &str,
    original: Option<(&str, RoutineKind)>,
    edit: &RoutineEdit,
) -> Result<()> {
    let _ = original;
    let (owner, bare) = match split_qualified(&edit.name) {
        Some((owner, bare)) => (owner.to_string(), bare.to_string()),
        None => (
            connection.default_schema_name().to_string(),
            edit.name.clone(),
        ),
    };
    let definition = edit
        .definition
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string();
    connection
        .with_conn(move |raw| {
            // Drop every existing object of that name (package spec + body included) before
            // replaying the definition, so a changed signature/column set does not collide.
            for object_type in object_types(raw, &owner, &bare).unwrap_or_default() {
                let drop = format!(
                    "DROP {} {}",
                    drop_keyword(&object_type),
                    qualify(&owner, &bare)
                );
                let _ = raw.execute(&drop, &[]);
            }
            if !definition.is_empty() {
                crate::connection::run_text_public(raw, &definition)?;
            }
            Ok(())
        })
        .await
}

pub(crate) async fn drop_routine(
    connection: &OracleConnection,
    _database: &str,
    kind: RoutineKind,
    name: &str,
) -> Result<()> {
    let (owner, bare) = match split_qualified(name) {
        Some((owner, bare)) => (owner.to_string(), bare.to_string()),
        None => (
            connection.default_schema_name().to_string(),
            name.to_string(),
        ),
    };
    connection
        .with_conn(move |raw| {
            let types = object_types(raw, &owner, &bare)?;
            let keyword = types
                .first()
                .map(|object_type| drop_keyword(object_type))
                .unwrap_or(kind.sql_name());
            let sql = format!("DROP {keyword} {}", qualify(&owner, &bare));
            crate::connection::run_text_public(raw, &sql).map(|_| ())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_kinds() {
        assert_eq!(kind_of("PROCEDURE"), RoutineKind::Procedure);
        assert_eq!(kind_of("TRIGGER"), RoutineKind::Procedure);
        assert_eq!(kind_of("FUNCTION"), RoutineKind::Function);
        assert_eq!(kind_of("PACKAGE"), RoutineKind::Function);
    }

    #[test]
    fn builds_drop_and_create_preview() {
        let edit = RoutineEdit {
            kind: RoutineKind::Function,
            name: "APP.F".to_string(),
            definition:
                "CREATE OR REPLACE FUNCTION \"APP\".\"F\" RETURN NUMBER AS BEGIN RETURN 1; END;"
                    .to_string(),
        };
        let sql = routine_sql(Some(("APP.F", RoutineKind::Function)), &edit);
        assert!(sql.starts_with("DROP FUNCTION \"APP\".\"F\";"));
        assert!(sql.contains("CREATE OR REPLACE FUNCTION"));
        assert!(!sql.trim_end().ends_with(';'));
    }
}

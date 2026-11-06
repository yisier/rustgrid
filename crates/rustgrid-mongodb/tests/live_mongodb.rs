//! Live MongoDB integration test (ignored by default).
//!
//! Set `RUSTGRID_MONGODB_PASSWORD` (and optionally `RUSTGRID_MONGODB_HOST`/`PORT`/`USER`) and run:
//!
//! ```text
//! cargo test -p rustgrid-mongodb -- --ignored
//! ```
//!
//! The round-trip test always uses and drops its own `rustgrid_live` database; the read-only type
//! test reads `RUSTGRID_MONGODB_DATABASE` (default `test`).

use std::collections::BTreeMap;

use mongodb::bson::{Bson, doc};
use rustgrid_core::{
    Connection, ConnectionConfig, ConnectionOptions, Driver, FilterCondition, FilterConjunction,
    FilterNode, FilterOperator, PageRequest, RowInsert, RowUpdate, SortColumn,
};
use rustgrid_mongodb::MongoDriver;

fn env(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

async fn connect() -> Box<dyn Connection> {
    let mut options = BTreeMap::new();
    options.insert(
        rustgrid_mongodb::OPT_AUTH_SOURCE.to_string(),
        "admin".to_string(),
    );
    let config = ConnectionConfig {
        driver: "mongodb".into(),
        host: env("RUSTGRID_MONGODB_HOST", "127.0.0.1"),
        port: env("RUSTGRID_MONGODB_PORT", "27017").parse().unwrap(),
        username: env("RUSTGRID_MONGODB_USER", "root"),
        password: Some(env("RUSTGRID_MONGODB_PASSWORD", "Root123!")),
        database: Some("admin".to_string()),
        options,
        settings: ConnectionOptions::default(),
    };
    MongoDriver::new()
        .connect(&config)
        .await
        .expect("connect to MongoDB")
}

#[tokio::test]
#[ignore = "requires a live MongoDB server (RUSTGRID_MONGODB_PASSWORD)"]
async fn mongodb_round_trip() {
    let connection = connect().await;
    // Always use a dedicated database: the first step drops it, so it must never point at a
    // database a user seeded.
    let database = "rustgrid_live".to_string();

    // Start from a clean database.
    let _ = connection.drop_database(&database).await;

    let version = connection.server_version().await.unwrap();
    assert!(!version.is_empty(), "server_version");

    // Write through the query tab: create the collection, then insert documents.
    connection
        .execute_query(Some(&database), r#"{ "create": "people" }"#)
        .await
        .unwrap();
    connection
        .execute_query(
            Some(&database),
            r#"{ "insert": "people", "documents": [
                { "_id": 1, "name": "Ada", "age": 36, "tags": ["math"], "address": { "city": "London" } },
                { "_id": 2, "name": "Linus", "age": 55, "tags": [] },
                { "_id": 3, "name": "Grace", "age": 45, "active": true }
            ] }"#,
        )
        .await
        .unwrap();

    let databases = connection.list_databases().await.unwrap();
    assert!(databases.iter().any(|info| info.name == database));

    let tables = connection.list_tables(&database).await.unwrap();
    assert!(tables.iter().any(|table| table.name == "people"));

    // The sampled field set must include `_id` as the key and the nested/absent-optional fields.
    let columns = connection.columns(&database, "people").await.unwrap();
    assert_eq!(columns[0].name, "_id");
    assert!(columns[0].primary_key);
    assert!(columns.iter().any(|column| column.name == "address"));

    // Page 0 with a filter and a sort.
    let filter = vec![FilterNode::Condition(FilterCondition {
        column: "age".into(),
        operator: FilterOperator::GreaterThan,
        value: "40".into(),
        value2: String::new(),
        conjunction: FilterConjunction::And,
        enabled: true,
    })];
    let page = connection
        .fetch_page(
            &database,
            "people",
            PageRequest::new(0, 10)
                .with_filter(filter)
                .with_order_by(vec![SortColumn {
                    column: "age".into(),
                    descending: true,
                }]),
        )
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(2));
    assert_eq!(page.rows.len(), 2);

    // Insert through the grid path.
    connection
        .insert_rows(
            &database,
            "people",
            &[RowInsert {
                values: vec![
                    ("_id".into(), Some("4".into())),
                    ("name".into(), Some("Margaret".into())),
                    ("age".into(), Some("87".into())),
                    ("tags".into(), Some("[\"apollo\"]".into())),
                ],
            }],
        )
        .await
        .unwrap();
    connection.columns(&database, "people").await.unwrap();

    // Update one row and delete another, keyed by `_id`.
    connection
        .update_rows(
            &database,
            "people",
            &[RowUpdate {
                set: vec![("age".into(), Some("88".into()))],
                keys: vec![("_id".into(), Some("4".into()))],
            }],
        )
        .await
        .unwrap();
    connection
        .delete_rows(
            &database,
            "people",
            &[vec![("_id".into(), Some("3".into()))]],
        )
        .await
        .unwrap();

    let result = connection
        .execute_query(
            Some(&database),
            r#"{ "find": "people", "filter": { "_id": 4 }, "limit": 1 }"#,
        )
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    let age = result
        .columns
        .iter()
        .position(|column| column.name == "age")
        .expect("age column");
    assert_eq!(result.rows[0][age].as_display(), "88");

    let aggregated = connection
        .execute_query(
            Some(&database),
            r#"{ "aggregate": "people", "pipeline": [ { "$group": { "_id": null, "total": { "$sum": "$age" } } } ] }"#,
        )
        .await
        .unwrap();
    assert_eq!(aggregated.rows.len(), 1);

    let counted = connection
        .execute_query(
            Some(&database),
            r#"{ "count": "people", "query": { "age": { "$gte": 40 } } }"#,
        )
        .await
        .unwrap();
    assert_eq!(counted.rows[0][0].as_display(), "2");

    let status = connection.table_status(&database, "people").await.unwrap();
    assert_eq!(status.rows, Some(3));

    // Rename and drop.
    connection
        .rename_table(&database, "people", "people_renamed")
        .await
        .unwrap();
    let tables = connection.list_tables(&database).await.unwrap();
    assert!(tables.iter().any(|table| table.name == "people_renamed"));
    connection
        .drop_table(&database, "people_renamed")
        .await
        .unwrap();

    connection.drop_database(&database).await.unwrap();
    connection.close().await.unwrap();
}

/// The decoded BSON types must survive the grid round-trip (ObjectId, date, decimal, embedded
/// document), which is what the row form's JSON editor relies on.
#[tokio::test]
#[ignore = "requires a live MongoDB server (RUSTGRID_MONGODB_PASSWORD)"]
async fn mongodb_types_survive_the_grid() {
    let connection = connect().await;
    let database = env("RUSTGRID_MONGODB_DATABASE", "test");

    let result = connection
        .execute_query(
            Some(&database),
            r#"{ "find": "type_showcase", "limit": 1 }"#,
        )
        .await
        .unwrap();
    let preview = result
        .rows
        .first()
        .map(|row| {
            result
                .columns
                .iter()
                .zip(row)
                .map(|(column, cell)| (column.name.clone(), cell.as_display()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let nested = preview
        .iter()
        .find(|(name, _)| name == "nested")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    assert!(
        nested.contains("\"level1\""),
        "embedded documents are Extended JSON, got {nested:?}"
    );

    // A JSON document with wrapped BSON types parses back to the same field names.
    let parsed =
        rustgrid_mongodb::value::parse_json(r#"{"a": {"$oid": "65f0c0ffee00000000000001"}}"#)
            .unwrap();
    assert!(matches!(parsed, Bson::Document(_)));
    let document = parsed.as_document().unwrap();
    assert!(matches!(document.get("a"), Some(Bson::ObjectId(_))));
}

/// A small sanity check that a raw command document comes back as a grid row.
#[tokio::test]
#[ignore = "requires a live MongoDB server (RUSTGRID_MONGODB_PASSWORD)"]
async fn mongodb_run_command_returns_a_row() {
    let connection = connect().await;
    let result = connection
        .execute_query(Some("admin"), r#"{ "ping": 1 }"#)
        .await
        .unwrap();
    assert!(result.has_result_set);
    assert_eq!(result.rows.len(), 1);
}

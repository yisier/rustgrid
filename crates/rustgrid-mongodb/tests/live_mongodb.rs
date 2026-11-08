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
    Connection, ConnectionConfig, ConnectionOptions, DatabaseOptions, Driver, FilterCondition,
    FilterConjunction, FilterNode, FilterOperator, PageRequest, RowInsert, RowUpdate, SortColumn,
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

    // The row form's JSON view: fetch the whole document, replace it (adding a field), and refuse
    // a changed `_id`.
    let keys = vec![("_id".to_string(), Some("4".to_string()))];
    let document = connection
        .row_document(&database, "people", &keys)
        .await
        .unwrap()
        .expect("document 4 exists");
    assert!(document.contains("Margaret"));
    let mut value: serde_json::Value = serde_json::from_str(&document).unwrap();
    value["note"] = serde_json::json!("edited");
    value["age"] = serde_json::json!({ "$numberInt": "90" });
    connection
        .replace_row_document(
            &database,
            "people",
            &keys,
            &serde_json::to_string(&value).unwrap(),
        )
        .await
        .unwrap();
    let replaced = connection
        .row_document(&database, "people", &keys)
        .await
        .unwrap()
        .expect("document 4 still exists");
    assert!(replaced.contains("edited"), "the new field is stored");
    assert!(replaced.contains("90"), "the edited field is stored");
    value["_id"] = serde_json::json!({ "$numberInt": "5" });
    assert!(
        connection
            .replace_row_document(
                &database,
                "people",
                &keys,
                &serde_json::to_string(&value).unwrap(),
            )
            .await
            .is_err(),
        "changing _id must be rejected"
    );

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

/// The New Database dialog's driver surface: MongoDB has no empty databases, so creating one
/// creates its first collection together with it.
#[tokio::test]
#[ignore = "requires a live MongoDB server (RUSTGRID_MONGODB_PASSWORD)"]
async fn mongodb_create_database_with_first_collection() {
    let connection = connect().await;
    let database = "rustgrid_live_create";
    let _ = connection.drop_database(database).await;

    // Without a collection there is nothing to create.
    assert!(
        connection
            .create_database(database, &DatabaseOptions::default())
            .await
            .is_err(),
        "an empty database name must be rejected"
    );

    connection
        .create_database(
            database,
            &DatabaseOptions {
                first_collection: "widgets".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let tables = connection.list_tables(database).await.unwrap();
    assert!(tables.iter().any(|table| table.name == "widgets"));

    connection.drop_database(database).await.unwrap();
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

/// The grid's 复制为 INSERT output is a JSON `insert` command whose cells are Extended JSON
/// (`{"$oid": …}`, `{"$numberDecimal": …}`, `{"$date": …}`), not SQL. Run exactly that shape so
/// the two sides cannot drift apart.
#[tokio::test]
#[ignore = "requires a live MongoDB server (RUSTGRID_MONGODB_PASSWORD)"]
async fn mongodb_copied_insert_command_runs() {
    let connection = connect().await;
    let database = "rustgrid_live_copy";
    let _ = connection.drop_database(database).await;
    connection
        .execute_query(Some(database), r#"{ "create": "orders" }"#)
        .await
        .unwrap();
    connection
        .execute_query(
            Some(database),
            r#"{
                "insert": "orders",
                "documents": [
                    {
                        "_id": { "$oid": "65f0c0ffee00000000000042" },
                        "order_no": "ORD-1",
                        "amount": { "$numberDecimal": "1943.80" },
                        "item_count": { "$numberInt": "1" },
                        "items": [
                            { "sku": "SKU-1", "qty": { "$numberInt": "1" } }
                        ],
                        "paid_at": null,
                        "created_at": { "$date": "2026-04-04T00:00:00.000Z" }
                    }
                ]
            }"#,
        )
        .await
        .unwrap();
    let result = connection
        .execute_query(Some(database), r#"{ "find": "orders", "limit": 1 }"#)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    let document = connection
        .row_document(
            database,
            "orders",
            &[(
                "_id".to_string(),
                Some("65f0c0ffee00000000000042".to_string()),
            )],
        )
        .await
        .unwrap()
        .expect("the copied document exists");
    assert!(document.contains("ORD-1"));
    assert!(document.contains("1943.80"));
    assert!(document.contains("SKU-1"));
    connection.drop_database(database).await.unwrap();
}

/// The 新建/设计集合 dialog's driver surface: create, read the definition, set a validator, rename
/// and clear it again.
#[tokio::test]
#[ignore = "requires a live MongoDB server (RUSTGRID_MONGODB_PASSWORD)"]
async fn mongodb_collection_definition_round_trip() {
    use rustgrid_core::CollectionDefinition;

    let connection = connect().await;
    let database = "rustgrid_live_collection";
    let _ = connection.drop_database(database).await;

    connection
        .create_collection(
            database,
            &CollectionDefinition {
                name: "widgets".into(),
                ..CollectionDefinition::default()
            },
        )
        .await
        .unwrap();
    let definition = connection
        .collection_definition(database, "widgets")
        .await
        .unwrap()
        .expect("the new collection has a definition");
    assert_eq!(definition.name, "widgets");

    let validated = CollectionDefinition {
        name: "widgets".into(),
        validator: r#"{"$jsonSchema":{"bsonType":"object","required":["name"]}}"#.into(),
        validation_level: "strict".into(),
        validation_action: "error".into(),
    };
    connection
        .alter_collection(database, "widgets", &validated)
        .await
        .unwrap();
    let definition = connection
        .collection_definition(database, "widgets")
        .await
        .unwrap()
        .expect("the collection still exists");
    assert!(definition.validator.contains("$jsonSchema"));
    assert_eq!(definition.validation_level, "strict");
    // A document that breaks the validator is rejected.
    assert!(
        connection
            .execute_query(
                Some(database),
                r#"{ "insert": "widgets", "documents": [ { "_id": 1 } ] }"#,
            )
            .await
            .is_err()
    );

    // A rename through the definition moves the collection.
    let renamed = CollectionDefinition {
        name: "gadgets".into(),
        ..validated
    };
    connection
        .alter_collection(database, "widgets", &renamed)
        .await
        .unwrap();
    let tables = connection.list_tables(database).await.unwrap();
    assert!(tables.iter().any(|table| table.name == "gadgets"));
    assert!(!tables.iter().any(|table| table.name == "widgets"));

    connection.drop_database(database).await.unwrap();
}

//! Live ClickHouse integration test. Ignored by default because it needs a real server.
//!
//! Enable it by setting `RUSTGRID_CLICKHOUSE_PASSWORD` (and optionally
//! `RUSTGRID_CLICKHOUSE_HOST` / `PORT` / `USER` / `DATABASE`) and running:
//!
//! ```text
//! cargo test -p rustgrid-clickhouse -- --ignored
//! ```
//!
//! A throwaway server (the `default` user has no password by default, so set one):
//!
//! ```text
//! docker run -d --name rg-ch -p 8123:8123 -p 9000:9000 \
//!   -e CLICKHOUSE_PASSWORD=clickhouse clickhouse/clickhouse-server
//! ```

use rustgrid_clickhouse::ClickHouseDriver;
use rustgrid_core::{
    ColumnDef, ConnectionConfig, DatabaseOptions, Driver, DriverId, FilterCondition, FilterNode,
    FilterOperator, ObjectKind, PageRequest, RowInsert, RowUpdate, SortColumn, TableSchema,
};

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

fn config() -> ConnectionConfig {
    ConnectionConfig {
        driver: DriverId::new("clickhouse"),
        host: env("RUSTGRID_CLICKHOUSE_HOST", "localhost"),
        port: env("RUSTGRID_CLICKHOUSE_PORT", "8123")
            .parse()
            .expect("RUSTGRID_CLICKHOUSE_PORT must be a number"),
        username: env("RUSTGRID_CLICKHOUSE_USER", "default"),
        password: Some(env("RUSTGRID_CLICKHOUSE_PASSWORD", "")),
        database: Some(env("RUSTGRID_CLICKHOUSE_DATABASE", "default")),
        options: Default::default(),
        settings: Default::default(),
    }
}

fn condition(column: &str, operator: FilterOperator, value: &str) -> FilterNode {
    let mut condition = FilterCondition::new(column);
    condition.operator = operator;
    condition.value = value.to_string();
    FilterNode::Condition(condition)
}

#[tokio::test]
#[ignore = "requires a live ClickHouse (RUSTGRID_CLICKHOUSE_*)"]
async fn round_trips_a_live_clickhouse() {
    let config = config();
    let connection = ClickHouseDriver::new()
        .connect(&config)
        .await
        .expect("connect");
    let database = "rustgrid_live";

    connection
        .execute_query(None, &format!("DROP DATABASE IF EXISTS {database}"))
        .await
        .expect("drop stale database");
    connection
        .execute_query(None, &format!("CREATE DATABASE {database}"))
        .await
        .expect("create database");

    let databases = connection.list_databases().await.unwrap();
    assert!(databases.iter().any(|entry| entry.name == database));

    connection
        .execute_query(
            Some(database),
            "CREATE TABLE people (\
                id UInt64, \
                name Nullable(String), \
                amount Decimal(20, 6), \
                created DateTime64(3), \
                flag Bool\
             ) ENGINE = MergeTree ORDER BY id",
        )
        .await
        .expect("create table");

    connection
        .insert_rows(
            database,
            "people",
            &[
                RowInsert {
                    values: vec![
                        ("id".to_string(), Some("1".to_string())),
                        ("name".to_string(), Some("ann".to_string())),
                        ("amount".to_string(), Some("12.340000".to_string())),
                        (
                            "created".to_string(),
                            Some("2024-01-02 03:04:05.000".to_string()),
                        ),
                        ("flag".to_string(), Some("true".to_string())),
                    ],
                },
                RowInsert {
                    values: vec![
                        ("id".to_string(), Some("2".to_string())),
                        ("name".to_string(), None),
                        ("amount".to_string(), Some("-0.500000".to_string())),
                        (
                            "created".to_string(),
                            Some("2024-01-03 00:00:00.000".to_string()),
                        ),
                        ("flag".to_string(), Some("false".to_string())),
                    ],
                },
            ],
        )
        .await
        .expect("insert rows");

    let tables = connection.list_tables(database).await.unwrap();
    assert!(
        tables
            .iter()
            .any(|table| table.name == "people" && table.kind == ObjectKind::Table)
    );

    let columns = connection.columns(database, "people").await.unwrap();
    let id = columns.iter().find(|column| column.name == "id").unwrap();
    assert_eq!(id.data_type, "UInt64");
    let name = columns.iter().find(|column| column.name == "name").unwrap();
    assert!(name.nullable);
    let amount = columns
        .iter()
        .find(|column| column.name == "amount")
        .unwrap();
    assert_eq!(amount.data_type, "Decimal(20, 6)");

    let page = PageRequest::new(0, 10)
        .with_filter(vec![
            condition("id", FilterOperator::GreaterOrEqual, "1"),
            condition("name", FilterOperator::Contains, "an"),
        ])
        .with_order_by(vec![SortColumn {
            column: "id".to_string(),
            descending: true,
        }]);
    let page = connection
        .fetch_page(database, "people", page)
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(1));
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0][0], rustgrid_core::CellValue::Uint(1));
    assert_eq!(
        page.rows[0][2],
        rustgrid_core::CellValue::Text("12.340000".to_string())
    );

    let count = connection
        .count_rows(database, "people", &[])
        .await
        .unwrap();
    assert_eq!(count, Some(2));

    let result = connection
        .execute_query(Some(database), "SELECT count() AS n FROM people")
        .await
        .unwrap();
    assert!(result.has_result_set);
    assert_eq!(result.rows[0][0], rustgrid_core::CellValue::Uint(2));

    let results = connection
        .execute_query_many(Some(database), "SELECT 1; SELECT 2")
        .await
        .unwrap();
    assert_eq!(results.len(), 2);

    let ddl = connection
        .object_ddl(database, "people", false)
        .await
        .unwrap()
        .expect("table DDL");
    assert!(ddl.contains("CREATE TABLE"));

    let status = connection.table_status(database, "people").await.unwrap();
    assert_eq!(status.engine.as_deref(), Some("MergeTree"));

    assert!(!connection.server_version().await.unwrap().is_empty());

    // The grid is read-only, so both row-oriented writes are refused with a clear message.
    let update = connection
        .update_rows(
            database,
            "people",
            &[RowUpdate {
                set: vec![("name".to_string(), Some("bob".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await;
    assert!(update.is_err());
    let delete = connection
        .delete_rows(
            database,
            "people",
            &[vec![("id".to_string(), Some("1".to_string()))]],
        )
        .await;
    assert!(delete.is_err());

    connection
        .execute_query(Some(database), "RENAME TABLE people TO people_renamed")
        .await
        .unwrap();
    let tables = connection.list_tables(database).await.unwrap();
    assert!(tables.iter().any(|table| table.name == "people_renamed"));

    connection
        .truncate_table(database, "people_renamed")
        .await
        .unwrap();
    let count = connection
        .count_rows(database, "people_renamed", &[])
        .await
        .unwrap();
    assert_eq!(count, Some(0));

    connection
        .drop_table(database, "people_renamed")
        .await
        .unwrap();
    connection.drop_database(database).await.unwrap();
    connection.close().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live ClickHouse (RUSTGRID_CLICKHOUSE_*)"]
async fn exercises_catalog_views_sessions_and_the_import_path() {
    let connection = ClickHouseDriver::new()
        .connect(&config())
        .await
        .expect("connect");
    let database = "rustgrid_live_extra";

    connection
        .execute_query(None, &format!("DROP DATABASE IF EXISTS {database}"))
        .await
        .expect("drop stale database");
    connection
        .create_database(database, &DatabaseOptions::default())
        .await
        .expect("create database");
    let databases = connection.list_databases().await.unwrap();
    assert!(databases.iter().any(|entry| entry.name == database));
    assert!(
        !databases
            .iter()
            .any(|entry| matches!(entry.name.as_str(), "system" | "INFORMATION_SCHEMA"))
    );

    connection
        .execute_query(
            Some(database),
            "CREATE TABLE events (id UInt64, name String) ENGINE = MergeTree ORDER BY id",
        )
        .await
        .expect("create table");
    connection
        .execute_query(
            Some(database),
            "CREATE VIEW event_names AS SELECT id, name FROM events",
        )
        .await
        .expect("create view");
    let tables = connection.list_tables(database).await.unwrap();
    assert!(
        tables
            .iter()
            .any(|table| table.name == "events" && table.kind == ObjectKind::Table)
    );
    assert!(
        tables
            .iter()
            .any(|table| table.name == "event_names" && table.kind == ObjectKind::View)
    );
    let view_columns = connection.columns(database, "event_names").await.unwrap();
    assert_eq!(view_columns.len(), 2);
    assert!(view_columns.iter().any(|column| column.name == "name"));
    let view_ddl = connection
        .object_ddl(database, "event_names", true)
        .await
        .unwrap()
        .expect("view DDL");
    assert!(view_ddl.contains("VIEW"));

    let statuses = connection.table_statuses(database).await.unwrap();
    let events = statuses
        .iter()
        .find(|(name, _)| name == "events")
        .map(|(_, status)| status)
        .expect("events status");
    assert_eq!(events.engine.as_deref(), Some("MergeTree"));
    let view = statuses
        .iter()
        .find(|(name, _)| name == "event_names")
        .map(|(_, status)| status)
        .expect("view status");
    assert_eq!(view.engine.as_deref(), Some("View"));
    assert!(connection.session_count().await.unwrap() >= 1);

    // The query tab's session keeps `SET` and `USE` across its statements.
    let mut session = connection
        .script_session(Some(database))
        .await
        .unwrap()
        .expect("ClickHouse script session");
    session.execute("SET max_threads = 1").await.unwrap();
    let setting = session
        .execute("SELECT getSetting('max_threads') AS value")
        .await
        .unwrap();
    assert_eq!(setting.rows[0][0], rustgrid_core::CellValue::Uint(1));
    session.execute("USE default").await.unwrap();
    let current = session
        .execute("SELECT currentDatabase() AS database")
        .await
        .unwrap();
    assert_eq!(
        current.rows[0][0],
        rustgrid_core::CellValue::Text("default".to_string())
    );
    session.execute(&format!("USE {database}")).await.unwrap();
    let current = session
        .execute("SELECT currentDatabase() AS database")
        .await
        .unwrap();
    assert_eq!(
        current.rows[0][0],
        rustgrid_core::CellValue::Text(database.to_string())
    );

    // The Import Wizard's CREATE path: MySQL-shaped inferred columns become ClickHouse types, and
    // decimal/text/nullable values round-trip through the batched INSERT.
    let schema = TableSchema {
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                nullable: false,
                primary_key: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "label".to_string(),
                data_type: "varchar".to_string(),
                length: "32".to_string(),
                nullable: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "amount".to_string(),
                data_type: "decimal".to_string(),
                length: "20".to_string(),
                decimals: "6".to_string(),
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "created".to_string(),
                data_type: "datetime".to_string(),
                ..ColumnDef::default()
            },
        ],
        ..TableSchema::default()
    };
    let create = connection.table_schema_sql(database, "imported", None, &schema);
    assert!(create.contains("ENGINE = MergeTree"));
    connection
        .execute_query(Some(database), &create)
        .await
        .expect("import CREATE");
    connection
        .insert_rows(
            database,
            "imported",
            &[RowInsert {
                values: vec![
                    ("id".to_string(), Some("7".to_string())),
                    ("label".to_string(), None),
                    ("amount".to_string(), Some("12.34".to_string())),
                    (
                        "created".to_string(),
                        Some("2024-05-06 07:08:09".to_string()),
                    ),
                ],
            }],
        )
        .await
        .expect("insert imported row");
    let imported = connection
        .fetch_page(database, "imported", PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(imported.total_rows, Some(1));
    assert_eq!(imported.rows[0][0], rustgrid_core::CellValue::Int(7));
    assert_eq!(imported.rows[0][1], rustgrid_core::CellValue::Null);
    assert_eq!(
        imported.rows[0][2],
        rustgrid_core::CellValue::Text("12.340000".to_string())
    );
    assert_eq!(
        imported.rows[0][3],
        rustgrid_core::CellValue::Text("2024-05-06 07:08:09".to_string())
    );
    connection.empty_table(database, "imported").await.unwrap();
    assert_eq!(
        connection
            .count_rows(database, "imported", &[])
            .await
            .unwrap(),
        Some(0)
    );

    // A read-only connection refuses writes in the driver and on the server.
    let mut read_only_config = config();
    read_only_config.settings.read_only = true;
    let read_only = ClickHouseDriver::new()
        .connect(&read_only_config)
        .await
        .expect("read-only connect");
    assert!(read_only.truncate_table(database, "events").await.is_err());
    assert!(
        read_only
            .execute_query(Some(database), "CREATE TABLE nope (x Int8) ENGINE = Memory")
            .await
            .is_err()
    );
    read_only.close().await.unwrap();

    connection
        .execute_query(Some(database), "DROP VIEW event_names")
        .await
        .unwrap();
    connection
        .execute_query(Some(database), "DROP TABLE imported")
        .await
        .unwrap();
    connection
        .execute_query(Some(database), "DROP TABLE events")
        .await
        .unwrap();
    connection.drop_database(database).await.unwrap();
    connection.close().await.unwrap();
}

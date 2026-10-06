//! Live PostgreSQL integration test. Ignored by default because it needs a real server.
//!
//! Enable it by setting `RUSTGRID_PG_PASSWORD` (and optionally
//! `RUSTGRID_PG_HOST` / `PORT` / `USER` / `DATABASE`) and running:
//!
//! ```text
//! cargo test -p rustgrid-postgresql -- --ignored
//! ```
//!
//! A throwaway server:
//!
//! ```text
//! docker run -d --name rg-pg -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:17
//! ```

use rustgrid_core::{
    BackupObjectKind, ConnectionConfig, Driver, DriverId, FilterCondition, FilterNode,
    FilterOperator, ObjectKind, PageRequest, RowInsert, RowUpdate, SortColumn,
};
use rustgrid_postgresql::PostgresDriver;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

fn config() -> ConnectionConfig {
    ConnectionConfig {
        driver: DriverId::new("postgresql"),
        host: env("RUSTGRID_PG_HOST", "localhost"),
        port: env("RUSTGRID_PG_PORT", "5432")
            .parse()
            .expect("RUSTGRID_PG_PORT must be a number"),
        username: env("RUSTGRID_PG_USER", "postgres"),
        password: Some(env("RUSTGRID_PG_PASSWORD", "")),
        database: Some(env("RUSTGRID_PG_DATABASE", "postgres")),
        options: Default::default(),
        settings: Default::default(),
    }
}

#[tokio::test]
#[ignore = "requires a live PostgreSQL (RUSTGRID_PG_*)"]
async fn round_trips_a_live_postgresql() {
    let config = config();
    let database = config
        .database
        .clone()
        .unwrap_or_else(|| "postgres".to_string());
    let connection = PostgresDriver::new()
        .connect(&config)
        .await
        .expect("connect");

    // Catalog: databases and schemas.
    let databases = connection.list_databases().await.unwrap();
    assert!(databases.iter().any(|entry| entry.name == database));
    let schemas = connection.list_schemas(&database).await.unwrap();
    assert!(schemas.contains(&"public".to_string()));
    assert!(!schemas.iter().any(|name| name.starts_with("pg_")));

    // Strong typing table with the tricky column types.
    let table = "public.rustgrid_live_users";
    let _ = connection
        .execute_query(
            Some(&database),
            &format!("DROP TABLE IF EXISTS {table} CASCADE"),
        )
        .await;
    connection
        .execute_query(
            Some(&database),
            &format!(
                "CREATE TABLE {table} (\
                    id serial PRIMARY KEY, \
                    name varchar(100) NOT NULL, \
                    age integer NULL, \
                    active boolean NULL, \
                    amount numeric(12,2) NULL, \
                    created timestamptz NULL, \
                    tags text[] NULL, \
                    doc jsonb NULL)"
            ),
        )
        .await
        .expect("create table");
    connection
        .execute_query(
            Some(&database),
            &format!(
                "INSERT INTO {table} (name, age, active, amount, created, tags, doc) VALUES \
                 ('ann', 30, true, 12.30, '2024-01-02 03:04:05+00', ARRAY['a','b'], '{{\"k\":1}}'), \
                 ('bob', 25, false, 99.99, NULL, NULL, NULL)"
            ),
        )
        .await
        .expect("insert rows");

    let tables = connection.list_tables(&database).await.unwrap();
    assert!(
        tables
            .iter()
            .any(|entry| entry.name == table && entry.kind == ObjectKind::Table)
    );
    let columns = connection.columns(&database, table).await.unwrap();
    assert!(
        columns
            .iter()
            .any(|column| column.name == "id" && column.primary_key)
    );
    assert!(
        columns
            .iter()
            .any(|column| column.name == "amount" && column.data_type.contains("numeric"))
    );

    // Paging with a filter on a strongly-typed column (binary protocol path).
    let page = connection
        .fetch_page(
            &database,
            table,
            PageRequest::new(0, 10)
                .with_order_by(vec![SortColumn {
                    column: "id".to_string(),
                    descending: false,
                }])
                .with_filter(vec![FilterNode::Condition(FilterCondition {
                    column: "age".to_string(),
                    operator: FilterOperator::GreaterOrEqual,
                    value: "26".to_string(),
                    value2: String::new(),
                    conjunction: Default::default(),
                    enabled: true,
                })]),
        )
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(1));
    assert_eq!(page.rows.len(), 1);
    // `numeric`/array/json values stay readable text on the binary path.
    let amount_index = columns.iter().position(|c| c.name == "amount").unwrap();
    assert_eq!(page.rows[0][amount_index].as_display(), "12.30");

    // Editing across the strongly-typed columns.
    connection
        .update_rows(
            &database,
            table,
            &[RowUpdate {
                set: vec![
                    ("age".to_string(), Some("31".to_string())),
                    ("active".to_string(), Some("false".to_string())),
                    ("amount".to_string(), Some("1000.50".to_string())),
                    (
                        "created".to_string(),
                        Some("2024-06-07 08:09:10+00".to_string()),
                    ),
                ],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .expect("update rows");
    connection
        .insert_rows(
            &database,
            table,
            &[RowInsert {
                values: vec![
                    ("name".to_string(), Some("carol".to_string())),
                    ("age".to_string(), Some("40".to_string())),
                    ("active".to_string(), Some("true".to_string())),
                    ("amount".to_string(), Some("7.25".to_string())),
                ],
            }],
        )
        .await
        .expect("insert rows");

    // Arbitrary SQL: multi-statement, a dollar-quoted function and a real transaction.
    let many = connection
        .execute_query_many(
            Some(&database),
            "BEGIN; INSERT INTO public.rustgrid_live_users (name) VALUES ('dave'); COMMIT;",
        )
        .await
        .unwrap();
    assert!(many.len() >= 3);
    let count: i64 = connection
        .execute_query(
            Some(&database),
            "SELECT count(*) FROM public.rustgrid_live_users",
        )
        .await
        .unwrap()
        .rows[0][0]
        .as_display()
        .parse()
        .unwrap();
    assert_eq!(count, 3);

    connection
        .execute_query(
            Some(&database),
            "CREATE OR REPLACE FUNCTION public.rustgrid_live_add(a integer, b integer) \
             RETURNS integer LANGUAGE sql AS $$ SELECT a + b; $$",
        )
        .await
        .expect("create function");
    let sum = connection
        .execute_query(Some(&database), "SELECT public.rustgrid_live_add(2, 3)")
        .await
        .unwrap();
    assert_eq!(sum.rows[0][0].as_display(), "5");

    // A zero-row SELECT still reports its columns (prepare fallback).
    let empty = connection
        .execute_query(
            Some(&database),
            "SELECT id, name FROM public.rustgrid_live_users WHERE id = -1",
        )
        .await
        .unwrap();
    assert!(empty.has_result_set);
    assert_eq!(empty.columns.len(), 2);

    // Schema management.
    let _ = connection
        .execute_query(
            Some(&database),
            "DROP SCHEMA IF EXISTS rustgrid_live_schema",
        )
        .await;
    connection
        .create_schema(&database, "rustgrid_live_schema")
        .await
        .expect("create schema");
    assert!(
        connection
            .list_schemas(&database)
            .await
            .unwrap()
            .contains(&"rustgrid_live_schema".to_string())
    );
    connection
        .drop_schema(&database, "rustgrid_live_schema")
        .await
        .expect("drop schema");

    // Table designer read + a view.
    let schema = connection.table_schema(&database, table).await.unwrap();
    assert!(
        schema
            .columns
            .iter()
            .any(|column| column.name == "id" && column.auto_increment)
    );
    let _ = connection
        .execute_query(
            Some(&database),
            "DROP VIEW IF EXISTS public.rustgrid_live_view",
        )
        .await;
    connection
        .execute_query(
            Some(&database),
            "CREATE VIEW public.rustgrid_live_view AS SELECT id, name FROM public.rustgrid_live_users",
        )
        .await
        .expect("create view");
    let views = connection.list_tables(&database).await.unwrap();
    assert!(
        views.iter().any(
            |entry| entry.name == "public.rustgrid_live_view" && entry.kind == ObjectKind::View
        )
    );
    // Opening a view must not fail on the missing `ctid`.
    connection
        .fetch_page(
            &database,
            "public.rustgrid_live_view",
            PageRequest::new(0, 10),
        )
        .await
        .expect("page a view");

    // Routines and roles.
    let routines = connection.list_routine_infos(&database).await.unwrap();
    assert!(
        routines
            .iter()
            .any(|routine| routine.name.starts_with("public.rustgrid_live_add("))
    );
    let users = connection.list_users().await.unwrap();
    assert!(!users.is_empty());

    // Backup metadata + restore (including sequence reset).
    let dump = connection
        .backup_object_metadata(&database, BackupObjectKind::Table, table)
        .await
        .unwrap();
    let mut rows: Vec<String> = Vec::new();
    connection
        .stream_table_rows(&database, table, &mut |row| {
            rows.push(row.to_string());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    let mut restored = dump;
    restored.rows = rows;
    connection
        .restore_object(&database, &restored)
        .await
        .unwrap();
    // A subsequent insert must not collide with the restored sequence.
    connection
        .insert_rows(
            &database,
            table,
            &[RowInsert {
                values: vec![("name".to_string(), Some("erin".to_string()))],
            }],
        )
        .await
        .expect("insert after restore");

    let _ = connection
        .execute_query(
            Some(&database),
            "DROP VIEW IF EXISTS public.rustgrid_live_view",
        )
        .await;
    let _ = connection
        .execute_query(
            Some(&database),
            "DROP FUNCTION IF EXISTS public.rustgrid_live_add(integer, integer)",
        )
        .await;
    let _ = connection
        .execute_query(
            Some(&database),
            "DROP TABLE IF EXISTS public.rustgrid_live_users CASCADE",
        )
        .await;
    connection.close().await.unwrap();
}

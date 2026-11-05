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
    // Two initial rows, the `carol` insert and the transaction's `dave`.
    assert_eq!(count, 4);

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
    let table_ddl = connection
        .object_ddl(&database, table, false)
        .await
        .unwrap()
        .expect("the table's DDL");
    assert!(
        table_ddl.to_ascii_uppercase().contains("CREATE TABLE"),
        "{table_ddl}"
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
    let view_ddl = connection
        .object_ddl(&database, "public.rustgrid_live_view", true)
        .await
        .unwrap()
        .expect("the view's DDL");
    assert!(
        view_ddl.to_ascii_uppercase().contains("CREATE VIEW"),
        "{view_ddl}"
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

    // The view depends on the table, so drop it before the table is restored (PostgreSQL refuses
    // to drop a table another object depends on).
    let _ = connection
        .execute_query(
            Some(&database),
            "DROP VIEW IF EXISTS public.rustgrid_live_view",
        )
        .await;

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
    assert_eq!(rows.len(), 4);
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

/// The table designer's write path: create a table from a `TableSchema`, read it back, alter it and
/// read it again — including a per-column `DESC` in the index.
#[tokio::test]
#[ignore = "requires a live PostgreSQL (RUSTGRID_PG_*)"]
async fn designer_round_trips_a_table() {
    use rustgrid_core::{ColumnDef, IndexColumnDef, IndexDef, TableSchema};

    let config = config();
    let database = config
        .database
        .clone()
        .unwrap_or_else(|| "postgres".to_string());
    let connection = PostgresDriver::new()
        .connect(&config)
        .await
        .expect("connect");

    let table = "public.rustgrid_designer";
    let _ = connection.drop_table(&database, table).await;

    let mut schema = TableSchema {
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                nullable: false,
                primary_key: true,
                auto_increment: true,
                identity: "ALWAYS".to_string(),
                identity_start: "1000".to_string(),
                identity_increment: "2".to_string(),
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "code".to_string(),
                data_type: "character varying".to_string(),
                length: "64".to_string(),
                nullable: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "amount".to_string(),
                data_type: "numeric".to_string(),
                length: "10".to_string(),
                decimals: "2".to_string(),
                nullable: true,
                comment: "the amount".to_string(),
                ..ColumnDef::default()
            },
        ],
        ..TableSchema::default()
    };
    schema.indexes.push(IndexDef {
        name: "ix_code".to_string(),
        columns: vec![
            IndexColumnDef {
                name: "code".to_string(),
                descending: true,
                prefix_length: String::new(),
            },
            IndexColumnDef::new("amount"),
        ],
        unique: false,
        primary: false,
        index_type: String::new(),
        comment: String::new(),
    });
    connection
        .save_table_schema(&database, table, None, &schema)
        .await
        .expect("create the table");

    let loaded = connection
        .table_schema(&database, table)
        .await
        .expect("read the table back");
    assert!(
        loaded.columns.iter().any(|column| {
            column.name == "id"
                && column.primary_key
                && column.auto_increment
                && column.identity == "ALWAYS"
                && column.identity_start == "1000"
                && column.identity_increment == "2"
        }),
        "the identity column did not round trip: {:?}",
        loaded.columns
    );
    assert!(
        loaded
            .columns
            .iter()
            .any(|column| column.name == "amount" && column.comment == "the amount"),
        "the column comment did not round trip: {:?}",
        loaded.columns
    );
    let index = loaded
        .indexes
        .iter()
        .find(|index| index.name == "ix_code")
        .unwrap_or_else(|| panic!("ix_code is missing: {:?}", loaded.indexes));
    assert_eq!(index.columns.len(), 2, "{:?}", index.columns);
    assert!(
        index.columns[0].descending,
        "the descending sort did not round trip: {:?}",
        index.columns
    );
    assert_eq!(index.columns[1].name, "amount");

    // Alter: a new column, and the index back to an ascending list.
    let mut modified = loaded.clone();
    modified.columns.push(ColumnDef {
        name: "note".to_string(),
        data_type: "character varying".to_string(),
        length: "50".to_string(),
        nullable: true,
        ..ColumnDef::default()
    });
    let index = modified
        .indexes
        .iter_mut()
        .find(|index| index.name == "ix_code")
        .expect("ix_code");
    index.columns = vec![IndexColumnDef::new("code"), IndexColumnDef::new("amount")];
    connection
        .save_table_schema(&database, table, Some(&loaded), &modified)
        .await
        .expect("alter the table");

    let reloaded = connection
        .table_schema(&database, table)
        .await
        .expect("read the altered table back");
    assert!(reloaded.columns.iter().any(|column| column.name == "note"));
    let index = reloaded
        .indexes
        .iter()
        .find(|index| index.name == "ix_code")
        .expect("ix_code after the alter");
    assert!(
        !index.columns[0].descending,
        "the ascending sort did not round trip: {:?}",
        index.columns
    );

    connection
        .drop_table(&database, table)
        .await
        .expect("drop the scratch table");
    connection.close().await.unwrap();
}

/// Regression for the query editor showing no result grid on PostgreSQL: the pinned script
/// session must report result sets, not only row counts.
#[tokio::test]
#[ignore = "requires a live PostgreSQL (RUSTGRID_PG_*)"]
async fn script_session_returns_result_sets() {
    let config = config();
    let database = config
        .database
        .clone()
        .unwrap_or_else(|| "postgres".to_string());
    let connection = PostgresDriver::new()
        .connect(&config)
        .await
        .expect("connect");
    let mut session = connection
        .script_session(Some(&database))
        .await
        .expect("open the script session")
        .expect("pin a connection");

    let results = session
        .execute_many("SELECT 1 AS n, 'x'::text AS t")
        .await
        .expect("run the select");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].has_result_set,
        "a SELECT produced no result set: {results:?}"
    );
    assert_eq!(results[0].columns.len(), 2);
    assert_eq!(results[0].rows.len(), 1);
    assert_eq!(results[0].statement, "SELECT 1 AS n, 'x'::text AS t");

    // A zero-row result set still exposes its columns, so the grid can show an empty table.
    let empty = session
        .execute_many("SELECT 1 AS n WHERE false")
        .await
        .expect("run the empty select");
    assert_eq!(empty.len(), 1);
    assert!(empty[0].has_result_set);
    assert_eq!(empty[0].columns.len(), 1);
    assert!(empty[0].rows.is_empty());

    // Return the pinned connection before closing the pool; `close` waits for checked-out ones.
    drop(session);
    connection.close().await.unwrap();
}

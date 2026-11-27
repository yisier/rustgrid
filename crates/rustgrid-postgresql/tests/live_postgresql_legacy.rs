//! Live PostgreSQL **9.x** compatibility test. Ignored by default because it needs a real server.
//!
//! The driver supports servers before PostgreSQL 10, where `attidentity`, `pg_sequence`,
//! `prokind`, `rolbypassrls` and `WITH ORDINALITY` do not exist yet. Run this against a 9.x
//! server (for example `docker run -d --name rg-pg93 -e POSTGRES_PASSWORD=postgres -p 5433:5432
//! postgres:9.3`):
//!
//! ```text
//! RUSTGRID_PG_PORT=5433 RUSTGRID_PG_PASSWORD=postgres \
//!   cargo test -p rustgrid-postgresql --test live_postgresql_legacy -- --ignored
//! ```
//!
//! On a modern server the test prints that it is skipped, so running the whole ignored suite
//! against PostgreSQL 17 remains possible. The main `live_postgresql.rs` suite uses identity
//! columns and `jsonb` and therefore requires PostgreSQL 10+.

use rustgrid_core::{
    BackupObjectKind, ColumnDef, ConnectionConfig, Driver, DriverId, IndexColumnDef, IndexDef,
    PageRequest, RowInsert, TableSchema,
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
#[ignore = "requires a live PostgreSQL 9.x (RUSTGRID_PG_*); skips on 10+"]
async fn legacy_catalog_and_serial_paths() {
    let config = config();
    let database = config
        .database
        .clone()
        .unwrap_or_else(|| "postgres".to_string());
    let connection = PostgresDriver::new()
        .connect(&config)
        .await
        .expect("connect");

    let version = connection.server_version().await.expect("server version");
    let major = version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    if major >= 10 {
        eprintln!("skipping the pre-10 compatibility test: server is {version}");
        connection.close().await.unwrap();
        return;
    }

    let table = "public.rustgrid_live_legacy";
    let view = "public.rustgrid_live_legacy_view";
    let function = "public.rustgrid_legacy_add";
    let _ = connection
        .execute_query(
            Some(&database),
            &format!(
                "DROP TABLE IF EXISTS {table} CASCADE; \
                 DROP VIEW IF EXISTS {view} CASCADE; \
                 DROP FUNCTION IF EXISTS {function}(integer, integer); \
                 DROP SEQUENCE IF EXISTS public.rustgrid_live_legacy_seq_seq; \
                 DROP SEQUENCE IF EXISTS public.rustgrid_live_legacy_extra_seq"
            ),
        )
        .await;

    // The designer's write path: a non-null primary key and a nullable auto-increment column.
    // Before PostgreSQL 10 the first is `bigserial` and the second a sequence/default pair.
    let mut schema = TableSchema {
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                nullable: false,
                primary_key: true,
                auto_increment: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "seq".to_string(),
                data_type: "integer".to_string(),
                nullable: true,
                auto_increment: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "code".to_string(),
                data_type: "character varying".to_string(),
                length: "32".to_string(),
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
        name: "ix_legacy_code".to_string(),
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
        .expect("create the designer table");

    let ddl = connection
        .object_ddl(&database, table, false)
        .await
        .unwrap()
        .expect("the table's DDL");
    assert!(ddl.contains("\"id\" bigserial NOT NULL"), "{ddl}");
    assert!(ddl.contains("CREATE SEQUENCE"), "{ddl}");
    assert!(!ddl.contains("GENERATED"), "{ddl}");

    // Catalog reads: the columns and the descending index come back.
    let columns = connection.columns(&database, table).await.expect("columns");
    assert!(
        columns
            .iter()
            .any(|column| column.name == "id" && column.auto_increment),
        "{columns:?}"
    );
    let loaded = connection
        .table_schema(&database, table)
        .await
        .expect("table schema");
    assert!(
        loaded
            .columns
            .iter()
            .any(|column| column.name == "seq" && column.auto_increment),
        "{:?}",
        loaded.columns
    );
    assert!(
        loaded
            .columns
            .iter()
            .any(|column| column.name == "amount" && column.comment == "the amount"),
        "{:?}",
        loaded.columns
    );
    let index = loaded
        .indexes
        .iter()
        .find(|index| index.name == "ix_legacy_code")
        .expect("ix_legacy_code");
    assert!(index.columns[0].descending, "{:?}", index.columns);
    assert_eq!(index.columns[1].name, "amount");

    // Insert through the serial/sequence defaults, then page the rows.
    connection
        .insert_rows(
            &database,
            table,
            &[RowInsert {
                values: vec![("code".to_string(), Some("a".to_string()))],
            }],
        )
        .await
        .expect("insert a row");
    let page = connection
        .fetch_page(&database, table, PageRequest::new(0, 10))
        .await
        .expect("page the table");
    assert_eq!(page.rows.len(), 1);
    assert!(matches!(
        &page.rows[0][0],
        rustgrid_core::CellValue::Int(_) | rustgrid_core::CellValue::Float(_)
    ));

    // Adding another nullable auto-increment column uses a sequence on a pre-10 server.
    let mut modified = loaded.clone();
    modified.columns.push(ColumnDef {
        name: "extra".to_string(),
        data_type: "integer".to_string(),
        nullable: true,
        auto_increment: true,
        ..ColumnDef::default()
    });
    connection
        .save_table_schema(&database, table, Some(&loaded), &modified)
        .await
        .expect("add a nullable auto-increment column");
    assert!(
        connection
            .table_schema(&database, table)
            .await
            .unwrap()
            .columns
            .iter()
            .any(|column| column.name == "extra" && column.auto_increment)
    );

    // Functions, views, users and collations (their catalog columns differ before 10/11).
    connection
        .execute_query(
            Some(&database),
            &format!(
                "CREATE OR REPLACE FUNCTION {function}(a integer, b integer) RETURNS integer \
                 LANGUAGE sql AS $$ SELECT a + b; $$"
            ),
        )
        .await
        .expect("create a function");
    let routines = connection
        .list_routine_infos(&database)
        .await
        .expect("list routines");
    let routine = routines
        .iter()
        .find(|routine| routine.name.starts_with(function))
        .expect("the function is listed");
    assert_eq!(routine.kind, rustgrid_core::RoutineKind::Function);
    let details = connection
        .routine_details(&database, routine.kind, &routine.name)
        .await
        .expect("routine details");
    assert!(
        details.definition.contains("FUNCTION"),
        "{}",
        details.definition
    );

    connection
        .execute_query(
            Some(&database),
            &format!("CREATE VIEW {view} AS SELECT id, code FROM {table}"),
        )
        .await
        .expect("create a view");
    let view_details = connection
        .view_details(&database, view)
        .await
        .expect("view details");
    assert!(view_details.definition.contains("VIEW"));

    let users = connection.list_users().await.expect("list users");
    assert!(!users.is_empty());
    connection
        .user_details(&config.username, "")
        .await
        .expect("user details");
    let collations = connection.collations().await.expect("collations");
    assert!(!collations.is_empty());

    // Backup metadata, row streaming and restore (including the sequence reset).
    let dump = connection
        .backup_object_metadata(&database, BackupObjectKind::Table, table)
        .await
        .expect("backup metadata");
    let mut rows = Vec::new();
    connection
        .stream_table_rows(&database, table, &mut |row| {
            rows.push(row.to_string());
            Ok(())
        })
        .await
        .expect("stream rows");
    assert_eq!(rows.len(), 1);
    let mut restored = dump;
    restored.rows = rows;
    connection
        .restore_object(&database, &restored)
        .await
        .expect("restore the table");
    connection
        .insert_rows(
            &database,
            table,
            &[RowInsert {
                values: vec![("code".to_string(), Some("b".to_string()))],
            }],
        )
        .await
        .expect("insert after restore");

    connection
        .execute_query(
            Some(&database),
            &format!(
                "DROP VIEW IF EXISTS {view} CASCADE; \
                 DROP FUNCTION IF EXISTS {function}(integer, integer); \
                 DROP TABLE IF EXISTS {table} CASCADE"
            ),
        )
        .await
        .expect("drop the scratch objects");
    connection.close().await.unwrap();
}

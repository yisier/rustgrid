//! Live tests against a **real** UDAL/TeleDB DBProxy (the open-source `TeleDB-DBProxy`
//! from `gitee.com/teledb/teledb-dbproxy`), not the Node emulator.
//!
//! Prerequisites:
//! - A DBProxy listening on `RUSTGRID_UDAL_PORT` (default 8066) with a logical schema `TEST`
//!   mapped to a backend database, and a `users(id INT PRIMARY KEY, name VARCHAR(50))` table.
//! - An account whose password is in `RUSTGRID_UDAL_PASSWORD` (user `RUSTGRID_UDAL_USER`,
//!   default `root`).
//!
//! The open-source DBProxy refuses every `CREATE`/`DROP` (`UDAL - CREATE command denied`), so
//! these tests never create tables; the schema and `users` table are part of the lab setup.
//!
//! Run with:
//! ```text
//! RUSTGRID_UDAL_PASSWORD=123456 cargo test -p rustgrid-mysql --test live_udal -- --ignored
//! ```

use rustgrid_core::{
    CellValue, Connection, ConnectionConfig, Driver, DriverId, Error, PageRequest, RowInsert,
    RowUpdate,
};
use std::collections::BTreeMap;

fn config(database: Option<&str>) -> ConnectionConfig {
    let host = std::env::var("RUSTGRID_UDAL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("RUSTGRID_UDAL_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8066);
    let username = std::env::var("RUSTGRID_UDAL_USER").unwrap_or_else(|_| "root".to_string());
    let password = std::env::var("RUSTGRID_UDAL_PASSWORD")
        .expect("set RUSTGRID_UDAL_PASSWORD to the DBProxy account's password");

    ConnectionConfig {
        driver: DriverId::new("mysql"),
        host,
        port,
        username,
        password: Some(password),
        database: database.map(str::to_string),
        options: BTreeMap::new(),
        settings: Default::default(),
    }
}

async fn connect(database: Option<&str>) -> Box<dyn Connection> {
    rustgrid_mysql::MysqlDriver::new()
        .connect(&config(database))
        .await
        .expect("connect to the UDAL DBProxy")
}

fn text(value: &CellValue) -> Option<&str> {
    match value {
        CellValue::Text(value) => Some(value),
        _ => None,
    }
}

/// The regression for issue #56: a prepared page query must not turn every later text query
/// into the DBProxy's binary rows. The DBProxy sets a session-wide `prepared` flag on the first
/// `COM_STMT_PREPARE` and never clears it; the driver detects UDAL and uses the text protocol
/// from the start, like Navicat.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real UDAL/TeleDB DBProxy (set RUSTGRID_UDAL_PASSWORD)"]
async fn live_udal_catalog_page_and_show_statements() {
    let connection = connect(Some("TEST")).await;

    let databases = connection.list_databases().await.expect("list databases");
    assert!(
        databases.iter().any(|database| database.name == "TEST"),
        "expected the logical TEST schema, got {:?}",
        databases.iter().map(|d| &d.name).collect::<Vec<_>>()
    );
    assert!(
        !databases
            .iter()
            .any(|database| database.name.eq_ignore_ascii_case("information_schema")),
        "a real UDAL only lists its logical schemas: {:?}",
        databases.iter().map(|d| &d.name).collect::<Vec<_>>()
    );

    let tables = connection.list_tables("TEST").await.expect("list tables");
    assert!(
        tables.iter().any(|table| table.name == "users"),
        "expected users, got {tables:?}"
    );

    let columns = connection.columns("TEST", "users").await.expect("columns");
    let id = columns
        .iter()
        .find(|column| column.name == "id")
        .expect("id column");
    assert!(id.primary_key);
    assert!(id.data_type.starts_with("int"), "id type: {}", id.data_type);
    assert!(
        columns
            .iter()
            .any(|column| column.name == "name" && column.data_type.starts_with("varchar")),
        "columns: {columns:?}"
    );

    // A prepared statement (sqlx binds): this is what used to poison the session.
    let page = connection
        .fetch_page("TEST", "users", PageRequest::new(0, 10))
        .await
        .expect("fetch page");
    assert_eq!(page.columns.len(), 2);
    assert!(page.rows.len() >= 2, "page: {page:?}");

    // Every later text statement must still decode as a text result set.
    let show_tables = connection
        .execute_query(Some("TEST"), "SHOW FULL TABLES")
        .await
        .expect("SHOW FULL TABLES");
    assert_eq!(show_tables.columns.len(), 2);
    assert!(
        show_tables.rows.iter().any(|row| {
            row.first().and_then(text) == Some("users")
                && row.get(1).and_then(text) == Some("BASE TABLE")
        }),
        "SHOW FULL TABLES rows: {:?}",
        show_tables.rows
    );

    let show_columns = connection
        .execute_query(Some("TEST"), "SHOW FULL COLUMNS FROM users")
        .await
        .expect("SHOW FULL COLUMNS");
    assert_eq!(show_columns.columns.len(), 9);
    assert!(show_columns.rows.len() >= 2);
    let id_row = show_columns
        .rows
        .iter()
        .find(|row| row.first().and_then(text) == Some("id"))
        .expect("id row");
    assert_eq!(id_row.get(1).and_then(text), Some("int(11)"));
    assert_eq!(id_row.get(4).and_then(text), Some("PRI"));

    let show_status = connection
        .execute_query(Some("TEST"), "SHOW TABLE STATUS")
        .await
        .expect("SHOW TABLE STATUS");
    assert!(show_status.columns.len() >= 19);
    let users_row = show_status
        .rows
        .iter()
        .find(|row| row.first().and_then(text) == Some("users"))
        .expect("users row");
    assert_eq!(users_row.get(1).and_then(text), Some("InnoDB"));
}

/// A proxy session without a current database only answers a narrow whitelist of statements.
/// Connecting must still work (the version probe is on that whitelist) and selecting a database
/// per operation must recover the rest.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real UDAL/TeleDB DBProxy (set RUSTGRID_UDAL_PASSWORD)"]
async fn live_udal_no_database_session() {
    let connection = connect(None).await;

    let version = connection
        .execute_query(None, "SELECT VERSION()")
        .await
        .expect("SELECT VERSION() without a database");
    let version = version
        .rows
        .first()
        .and_then(|row| row.first())
        .and_then(text)
        .expect("version row");
    assert!(version.contains("UDAL"), "version: {version}");

    connection
        .execute_query(None, "SELECT 1")
        .await
        .expect("SELECT 1 without a database");
    let databases = connection.list_databases().await.expect("list databases");
    assert!(databases.iter().any(|database| database.name == "TEST"));

    // A constant SELECT with an alias is not on UDAL's whitelist.
    let error = connection
        .execute_query(None, "SELECT 1 AS one")
        .await
        .expect_err("UDAL refuses an aliased SELECT without a database");
    let message = error.to_string();
    assert!(
        message.contains("1046") && message.contains("No database selected"),
        "unexpected error: {message}"
    );

    // Selecting the database for the operation makes the same statement work.
    let result = connection
        .execute_query(Some("TEST"), "SELECT 1 AS one")
        .await
        .expect("aliased SELECT with a database");
    assert_eq!(result.rows.len(), 1);
}

/// INSERT/UPDATE/DELETE go through the text protocol on UDAL (the open-source DBProxy forbids
/// CREATE/DROP, so this test uses the lab's `users` table).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real UDAL/TeleDB DBProxy (set RUSTGRID_UDAL_PASSWORD)"]
async fn live_udal_writes_round_trip() {
    let connection = connect(Some("TEST")).await;
    let id = "90";
    let _ = connection
        .delete_rows(
            "TEST",
            "users",
            &[vec![("id".to_string(), Some(id.to_string()))]],
        )
        .await;

    connection
        .insert_rows(
            "TEST",
            "users",
            &[RowInsert {
                values: vec![
                    ("id".to_string(), Some(id.to_string())),
                    ("name".to_string(), Some("before".to_string())),
                ],
            }],
        )
        .await
        .expect("insert through the text protocol");

    connection
        .update_rows(
            "TEST",
            "users",
            &[RowUpdate {
                set: vec![("name".to_string(), Some("after".to_string()))],
                keys: vec![("id".to_string(), Some(id.to_string()))],
            }],
        )
        .await
        .expect("update through the text protocol");

    let result = connection
        .execute_query(Some("TEST"), "SELECT name FROM users WHERE id = 90")
        .await
        .expect("read the updated row");
    assert_eq!(
        result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(text),
        Some("after")
    );

    connection
        .delete_rows(
            "TEST",
            "users",
            &[vec![("id".to_string(), Some(id.to_string()))]],
        )
        .await
        .expect("delete through the text protocol");
    let result = connection
        .execute_query(Some("TEST"), "SELECT COUNT(*) FROM users WHERE id = 90")
        .await
        .expect("count the deleted row");
    match result.rows.first().and_then(|row| row.first()) {
        Some(CellValue::Int(0)) | Some(CellValue::Uint(0)) => {}
        other => panic!("expected the row to be gone, got {other:?}"),
    }
}

/// Reading the table designer's metadata exercises the text `information_schema` / `SHOW TABLE
/// STATUS` paths that used to fail with `3013` and `buffer exhausted`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real UDAL/TeleDB DBProxy (set RUSTGRID_UDAL_PASSWORD)"]
async fn live_udal_table_designer_reads_metadata() {
    let connection = connect(Some("TEST")).await;

    let schema = connection
        .table_schema("TEST", "users")
        .await
        .expect("load the table schema");
    let id = schema
        .columns
        .iter()
        .find(|column| column.name == "id")
        .expect("id column");
    assert!(id.primary_key);
    assert!(id.data_type.starts_with("int"));
    assert!(
        schema
            .columns
            .iter()
            .any(|column| column.name == "name" && column.data_type.starts_with("varchar"))
    );
    assert!(
        schema
            .indexes
            .iter()
            .any(|index| index.name.eq_ignore_ascii_case("PRIMARY")),
        "indexes: {:?}",
        schema.indexes
    );

    let status = connection
        .table_status("TEST", "users")
        .await
        .expect("table status");
    assert_eq!(status.engine.as_deref(), Some("InnoDB"));
    assert!(status.rows.is_some());

    let ddl = connection
        .object_ddl("TEST", "users", false)
        .await
        .expect("SHOW CREATE TABLE")
        .expect("SHOW CREATE TABLE returns a definition");
    assert!(
        ddl.to_ascii_uppercase().contains("CREATE TABLE"),
        "ddl: {ddl}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real UDAL/TeleDB DBProxy (set RUSTGRID_UDAL_PASSWORD)"]
async fn live_udal_auth_failure_maps_to_authentication() {
    let mut config = config(Some("TEST"));
    config.password = Some("definitely-wrong-password".to_string());

    let error = match rustgrid_mysql::MysqlDriver::new().connect(&config).await {
        Ok(_) => panic!("connect should fail with a wrong password"),
        Err(error) => error,
    };

    assert!(
        matches!(error, Error::Authentication(_)),
        "expected an authentication error, got: {error}"
    );
}

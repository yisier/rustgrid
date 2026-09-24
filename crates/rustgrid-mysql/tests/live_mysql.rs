use std::collections::BTreeMap;

use rustgrid_core::{ConnectionConfig, Driver, DriverId, PageRequest};

fn config() -> ConnectionConfig {
    let host = std::env::var("RUSTGRID_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("RUSTGRID_MYSQL_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3306);
    let username = std::env::var("RUSTGRID_MYSQL_USER").unwrap_or_else(|_| "root".to_string());
    let password = std::env::var("RUSTGRID_MYSQL_PASSWORD").ok();
    let database = std::env::var("RUSTGRID_MYSQL_DATABASE").ok();

    ConnectionConfig {
        driver: DriverId::new("mysql"),
        host,
        port,
        username,
        password,
        database,
        options: BTreeMap::new(),
        settings: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_catalog_and_page() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let databases = connection.list_databases().await.expect("list databases");
    assert!(
        databases
            .iter()
            .any(|database| database.name == "information_schema"),
        "expected information_schema to be listed"
    );

    let tables = connection
        .list_tables("information_schema")
        .await
        .expect("list tables");
    assert!(
        tables
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("TABLES")),
        "expected information_schema.TABLES to be listed"
    );

    let columns = connection
        .columns("information_schema", "TABLES")
        .await
        .expect("load columns");
    assert!(!columns.is_empty());

    let page = connection
        .fetch_page("information_schema", "TABLES", PageRequest::new(0, 10))
        .await
        .expect("fetch page");
    assert_eq!(page.columns.len(), columns.len());
    assert!(page.rows.len() <= 10);
    assert!(page.total_rows.unwrap_or(0) > 0);
    assert!(
        page.rows
            .iter()
            .flatten()
            .any(|cell| !matches!(cell, rustgrid_core::CellValue::Null)),
        "expected at least one non-null cell"
    );

    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_multi_statement_returns_every_result_set() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    // Two result sets, a zero-row SELECT (whose columns must still come back) and a session
    // variable set by an earlier statement in the same script.
    let results = connection
        .execute_query_many(
            None,
            "SET @rustgrid_probe = 41; \
             SELECT 1 AS a; \
             SELECT 2 AS b, 3 AS c; \
             SELECT @rustgrid_probe + 1 AS d WHERE 1 = 0; \
             SELECT @rustgrid_probe + 1 AS e",
        )
        .await
        .expect("run a multi-statement script");

    assert_eq!(results.len(), 5, "one result per statement");
    assert!(!results[0].has_result_set);
    assert!(results[0].statement.starts_with("SET @rustgrid_probe"));

    assert!(results[1].has_result_set);
    assert_eq!(results[1].columns.len(), 1);
    assert_eq!(results[1].columns[0].name, "a");
    assert_eq!(results[1].rows.len(), 1);
    assert_eq!(results[1].statement, "SELECT 1 AS a");

    assert!(results[2].has_result_set);
    assert_eq!(results[2].columns.len(), 2);

    // A zero-row SELECT still exposes its columns (via `describe_columns`).
    assert!(results[3].has_result_set);
    assert_eq!(results[3].columns[0].name, "d");
    assert!(results[3].rows.is_empty());

    // Session state set by the first statement is visible to the last.
    assert!(results[4].has_result_set);
    assert!(matches!(
        results[4].rows.first().and_then(|row| row.first()),
        Some(rustgrid_core::CellValue::Int(42))
    ));

    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server"]
async fn live_auth_failure_maps_to_authentication() {
    let mut config = config();
    config.password = Some("definitely-wrong-password".to_string());

    let driver = rustgrid_mysql::MysqlDriver::new();
    let error = match driver.connect(&config).await {
        Ok(_) => panic!("connect should fail with a wrong password"),
        Err(error) => error,
    };

    assert!(
        matches!(error, rustgrid_core::Error::Authentication(_)),
        "expected an authentication error, got: {error}"
    );
}

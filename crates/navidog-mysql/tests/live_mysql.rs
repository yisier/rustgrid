use std::collections::BTreeMap;

use navidog_core::{ConnectionConfig, Driver, DriverId, PageRequest};

fn config() -> ConnectionConfig {
    let host = std::env::var("NAVIDOG_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("NAVIDOG_MYSQL_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3306);
    let username = std::env::var("NAVIDOG_MYSQL_USER").unwrap_or_else(|_| "root".to_string());
    let password = std::env::var("NAVIDOG_MYSQL_PASSWORD").ok();
    let database = std::env::var("NAVIDOG_MYSQL_DATABASE").ok();

    ConnectionConfig {
        driver: DriverId::new("mysql"),
        host,
        port,
        username,
        password,
        database,
        options: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set NAVIDOG_MYSQL_PASSWORD)"]
async fn live_catalog_and_page() {
    let driver = navidog_mysql::MysqlDriver::new();
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
            .any(|cell| !matches!(cell, navidog_core::CellValue::Null)),
        "expected at least one non-null cell"
    );

    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server"]
async fn live_auth_failure_maps_to_authentication() {
    let mut config = config();
    config.password = Some("definitely-wrong-password".to_string());

    let driver = navidog_mysql::MysqlDriver::new();
    let error = match driver.connect(&config).await {
        Ok(_) => panic!("connect should fail with a wrong password"),
        Err(error) => error,
    };

    assert!(
        matches!(error, navidog_core::Error::Authentication(_)),
        "expected an authentication error, got: {error}"
    );
}

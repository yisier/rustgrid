//! Live verification of the MySQL-protocol drivers against the server saved in the RustGrid
//! config (falling back to `RUSTGRID_MYSQL_*`). It exercises the catalog, a scratch-database
//! create/insert/update/select/schema/delete lifecycle, and — when the server is actually a
//! MariaDB — the same lifecycle through the MariaDB driver (which is what hits the MariaDB-only
//! `SET SESSION max_statement_time` path).
//!
//! Run with:
//!   cargo test -p rustgrid-app --test live_mariadb -- --ignored --nocapture

use std::collections::BTreeMap;

use rustgrid_core::{ConnectionConfig, Driver, DriverId, PageRequest, RowInsert, RowUpdate};
use rustgrid_mysql::{MariaDbDriver, MysqlDriver};

fn saved_config(driver: &str) -> ConnectionConfig {
    if let Ok(password) = std::env::var("RUSTGRID_MYSQL_PASSWORD") {
        return ConnectionConfig {
            driver: DriverId::new(driver),
            host: std::env::var("RUSTGRID_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: std::env::var("RUSTGRID_MYSQL_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3306),
            username: std::env::var("RUSTGRID_MYSQL_USER").unwrap_or_else(|_| "root".to_string()),
            password: Some(password),
            database: None,
            options: BTreeMap::new(),
            settings: Default::default(),
        };
    }

    let store = rustgrid_config::ConfigStore::new().expect("config dir");
    let profiles = store.load_profiles().expect("saved profiles");
    let secrets = store.load_secrets().unwrap_or_default();
    let profile = profiles
        .first()
        .expect("a saved RustGrid connection (or set RUSTGRID_MYSQL_PASSWORD)");
    ConnectionConfig {
        driver: DriverId::new(driver),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password: secrets.get(&profile.id).cloned(),
        // The saved settings carry `query_timeout`, which is what makes the MariaDB driver run
        // `SET SESSION max_statement_time` on connect.
        settings: profile.settings.clone(),
        database: None,
        options: BTreeMap::new(),
    }
}

/// A scratch-database lifecycle shared by both drivers.
async fn lifecycle(driver: &dyn Driver) {
    let connection = driver
        .connect(&saved_config(driver.id().as_str()))
        .await
        .expect("connect");

    let version = connection.server_version().await.expect("server version");
    println!(
        "{} -> connected, server version: {version}",
        driver.display_name()
    );

    let databases = connection.list_databases().await.expect("list databases");
    assert!(
        databases
            .iter()
            .any(|database| database.name == "information_schema"),
        "expected information_schema in {databases:?}"
    );

    let database = format!("rustgrid_probe_{}", driver.id());
    let quoted = format!("`{database}`");
    let _ = connection
        .execute_query(None, &format!("DROP DATABASE IF EXISTS {quoted}"))
        .await;
    connection
        .execute_query(None, &format!("CREATE DATABASE {quoted}"))
        .await
        .expect("create scratch database");
    connection
        .execute_query(
            Some(&database),
            "CREATE TABLE t (id INT PRIMARY KEY, name VARCHAR(32), age INT)",
        )
        .await
        .expect("create table");

    connection
        .insert_rows(
            &database,
            "t",
            &[RowInsert {
                values: vec![
                    ("id".to_string(), Some("1".to_string())),
                    ("name".to_string(), Some("ann".to_string())),
                    ("age".to_string(), Some("30".to_string())),
                ],
            }],
        )
        .await
        .expect("insert");
    connection
        .update_rows(
            &database,
            "t",
            &[RowUpdate {
                set: vec![("age".to_string(), Some("31".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .expect("update");

    let page = connection
        .fetch_page(&database, "t", PageRequest::new(0, 10))
        .await
        .expect("fetch page");
    assert_eq!(page.rows.len(), 1);

    let schema = connection
        .table_schema(&database, "t")
        .await
        .expect("table schema");
    assert_eq!(schema.columns.len(), 3);

    let query = connection
        .execute_query(Some(&database), "SELECT name FROM t")
        .await
        .expect("select");
    assert!(query.has_result_set);

    connection
        .delete_rows(
            &database,
            "t",
            &[vec![("id".to_string(), Some("1".to_string()))]],
        )
        .await
        .expect("delete");
    connection
        .execute_query(None, &format!("DROP DATABASE {quoted}"))
        .await
        .expect("drop scratch database");
    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL/MariaDB server (uses the saved RustGrid connection)"]
async fn live_saved_server() {
    // Identify the server with the MySQL driver first, then run its lifecycle.
    let probe = MysqlDriver::new()
        .connect(&saved_config("mysql"))
        .await
        .expect("connect (mysql)");
    let version = probe.server_version().await.expect("server version");
    probe.close().await.expect("close");
    println!("server version: {version}");
    assert!(!version.is_empty());

    lifecycle(&MysqlDriver::new()).await;

    // `SET SESSION max_statement_time` only exists on MariaDB; running the MariaDB driver against a
    // MySQL server is expected to fail, so only do it when the server really is MariaDB.
    if version.to_ascii_lowercase().contains("mariadb") {
        lifecycle(&MariaDbDriver::new()).await;
        println!("MariaDB driver path verified");
    } else {
        println!("server is not MariaDB; skipping the MariaDB-specific driver path");
    }
}

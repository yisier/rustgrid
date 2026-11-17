//! Live OceanBase smoke test. OceanBase's MySQL mode is served by the MySQL driver, so the test
//! exercises the paths the app uses on a plain connection: the login statements, the catalog,
//! paging and the write path.
//!
//! Docker:
//! ```text
//! docker run -p 2881:2881 --name ob314 -e MINI_MODE=1 --ulimit nofile=65536:65536 \
//!   oceanbase/oceanbase-ce:3.1.4
//! ```
//! then `cargo test -p rustgrid-mysql --test live_oceanbase -- --ignored`.

use rustgrid_core::{
    CellValue, ConnectionConfig, Driver, DriverId, PageRequest, RowInsert, RowUpdate, TlsMode,
};
use rustgrid_mysql::MysqlDriver;

fn config() -> ConnectionConfig {
    let host = std::env::var("RUSTGRID_OCEANBASE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("RUSTGRID_OCEANBASE_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2881);
    // The image's sys tenant account; a real deployment sets its own user (often `user@tenant`).
    let username =
        std::env::var("RUSTGRID_OCEANBASE_USER").unwrap_or_else(|_| "root@sys".to_string());
    let password = std::env::var("RUSTGRID_OCEANBASE_PASSWORD")
        .ok()
        .filter(|password| !password.is_empty());
    let mut config = ConnectionConfig {
        driver: DriverId::new("mysql"),
        host,
        port,
        username,
        password,
        database: None,
        options: Default::default(),
        settings: Default::default(),
    };
    config.settings.tls.mode = TlsMode::Disabled;
    config.settings.connect_timeout = Some(10);
    config
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live OceanBase server (set RUSTGRID_OCEANBASE_PASSWORD if needed)"]
async fn live_oceanbase_catalog_page_and_edit() {
    let driver = MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live OceanBase server");

    let version = connection.server_version().await.expect("server version");
    assert!(
        version.contains("OceanBase"),
        "expected an OceanBase server, got {version:?}"
    );

    let databases = connection.list_databases().await.expect("list databases");
    assert!(
        databases
            .iter()
            .any(|database| database.name == "information_schema"),
        "expected information_schema to be listed"
    );

    let database = "rustgrid_oceanbase_live";
    let table = "users";
    connection
        .execute_query(None, &format!("DROP DATABASE IF EXISTS `{database}`"))
        .await
        .expect("drop the scratch database");
    connection
        .execute_query(None, &format!("CREATE DATABASE `{database}`"))
        .await
        .expect("create the scratch database");
    connection
        .execute_query(
            Some(database),
            &format!(
                "CREATE TABLE `{table}` (\
                 `id` BIGINT NOT NULL, \
                 `name` VARCHAR(64) NULL, \
                 `score` INT NULL, \
                 PRIMARY KEY (`id`))"
            ),
        )
        .await
        .expect("create the scratch table");

    let tables = connection.list_tables(database).await.expect("list tables");
    assert!(
        tables.iter().any(|info| info.name == table),
        "expected {table} in {tables:?}"
    );
    let columns = connection
        .columns(database, table)
        .await
        .expect("read columns");
    assert_eq!(columns.len(), 3);
    assert!(columns[0].primary_key);

    // The table designer loads this; OceanBase's disabled prepares used to fail the options
    // query here with error 1235.
    let schema = connection
        .table_schema(database, table)
        .await
        .expect("read the table schema");
    assert_eq!(schema.columns.len(), 3);
    assert_eq!(schema.columns[0].name, "id");

    // These two read `information_schema` through prepared statements on other engines; on
    // OceanBase they must use the text protocol too.
    let routines = connection
        .list_routine_infos(database)
        .await
        .expect("list routines");
    assert!(routines.is_empty());

    connection
        .insert_rows(
            database,
            table,
            &[RowInsert {
                values: vec![
                    ("id".to_string(), Some("1".to_string())),
                    ("name".to_string(), Some("alice".to_string())),
                    ("score".to_string(), Some("10".to_string())),
                ],
            }],
        )
        .await
        .expect("insert");

    let page = connection
        .fetch_page(database, table, PageRequest::new(0, 10))
        .await
        .expect("fetch the first page");
    assert_eq!(page.columns.len(), 3);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.total_rows, Some(1));
    assert!(matches!(&page.rows[0][1], CellValue::Text(name) if name == "alice"));

    connection
        .update_rows(
            database,
            table,
            &[RowUpdate {
                set: vec![("name".to_string(), Some("bob".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .expect("update");
    let page = connection
        .fetch_page(database, table, PageRequest::new(0, 10))
        .await
        .expect("fetch the updated page");
    assert!(matches!(&page.rows[0][1], CellValue::Text(name) if name == "bob"));

    connection
        .delete_rows(
            database,
            table,
            &[vec![("id".to_string(), Some("1".to_string()))]],
        )
        .await
        .expect("delete");
    let page = connection
        .fetch_page(database, table, PageRequest::new(0, 10))
        .await
        .expect("fetch the empty page");
    assert_eq!(page.rows.len(), 0);
    assert_eq!(page.total_rows, Some(0));

    connection
        .execute_query(Some(database), &format!("DROP TABLE `{table}`"))
        .await
        .expect("drop the scratch table");
    connection
        .execute_query(None, &format!("DROP DATABASE IF EXISTS `{database}`"))
        .await
        .expect("drop the scratch database");

    connection.close().await.expect("close");
}

//! Live ODBC integration test (ignored by default).
//!
//! Set a connection string and run:
//!
//! ```powershell
//! $env:RUSTGRID_ODBC_CONNECTION_STRING="Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;UID=sa;PWD=...;TrustServerCertificate=yes;"
//! cargo test -p rustgrid-odbc -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;

use rustgrid_core::{
    ConnectionConfig, ConnectionOptions, Driver, DriverId, PageRequest, RowInsert, RowUpdate,
};
use rustgrid_odbc::OdbcDriver;

const PROBE: &str = "dbo.rustgrid_odbc_probe";
const PROBE_DB: &str = "tempdb";

fn config() -> Option<ConnectionConfig> {
    let connection_string = std::env::var("RUSTGRID_ODBC_CONNECTION_STRING").ok()?;
    Some(ConnectionConfig {
        driver: DriverId::new("odbc"),
        host: String::new(),
        port: 0,
        username: String::new(),
        password: None,
        database: None,
        options: BTreeMap::from([("odbc.connection_string".to_string(), connection_string)]),
        settings: ConnectionOptions::default(),
    })
}

#[tokio::test]
#[ignore]
async fn live_odbc_browse_query_and_edit() {
    let Some(config) = config() else {
        eprintln!("RUSTGRID_ODBC_CONNECTION_STRING not set; skipping");
        return;
    };

    let drivers = OdbcDriver::new().connection_drivers();
    eprintln!("drivers: {drivers:?}");
    assert!(!drivers.is_empty(), "expected at least one ODBC driver");

    let connection = OdbcDriver::new()
        .connect(&config)
        .await
        .expect("connect the ODBC connection");

    let databases = connection.list_databases().await.expect("list databases");
    let names = databases.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
    eprintln!("databases: {names:?}");
    assert!(!names.is_empty(), "expected at least one database");

    // A scratch table, cleaned up first in case a previous run left one behind.
    connection
        .execute_query(
            None,
            &format!(
                "IF OBJECT_ID('{PROBE_DB}.{PROBE}') IS NOT NULL DROP TABLE {PROBE_DB}.{PROBE}"
            ),
        )
        .await
        .expect("drop a stale probe table");
    connection
        .execute_query(
            None,
            &format!("CREATE TABLE {PROBE_DB}.{PROBE} (id int NOT NULL PRIMARY KEY, name varchar(50) NULL)"),
        )
        .await
        .expect("create the probe table");

    let columns = connection.columns(PROBE_DB, PROBE).await.expect("columns");
    eprintln!(
        "columns: {:?}",
        columns
            .iter()
            .map(|c| (&c.name, &c.data_type))
            .collect::<Vec<_>>()
    );
    assert_eq!(columns.len(), 2);

    connection
        .insert_rows(
            PROBE_DB,
            PROBE,
            &[
                RowInsert {
                    values: vec![
                        ("id".to_string(), Some("1".to_string())),
                        ("name".to_string(), Some("alpha".to_string())),
                    ],
                },
                RowInsert {
                    values: vec![
                        ("id".to_string(), Some("2".to_string())),
                        ("name".to_string(), None),
                    ],
                },
            ],
        )
        .await
        .expect("insert rows");

    let page = connection
        .fetch_page(PROBE_DB, PROBE, PageRequest::new(0, 10))
        .await
        .expect("fetch page");
    eprintln!("rows after insert: {:?}", page.rows);
    assert_eq!(page.rows.len(), 2);

    connection
        .update_rows(
            PROBE_DB,
            PROBE,
            &[RowUpdate {
                set: vec![("name".to_string(), Some("beta".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .expect("update a row");

    connection
        .delete_rows(
            PROBE_DB,
            PROBE,
            &[vec![("id".to_string(), Some("2".to_string()))]],
        )
        .await
        .expect("delete a row");

    let page = connection
        .fetch_page(PROBE_DB, PROBE, PageRequest::new(0, 10))
        .await
        .expect("fetch page again");
    eprintln!("rows after edit: {:?}", page.rows);
    assert_eq!(page.rows.len(), 1);

    connection
        .truncate_table(PROBE_DB, PROBE)
        .await
        .expect("truncate");
    connection
        .drop_table(PROBE_DB, PROBE)
        .await
        .expect("drop the probe table");

    let version = connection.server_version().await.unwrap_or_default();
    eprintln!("server: {version}");

    connection.close().await.expect("close");
}

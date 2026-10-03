//! Live SQL Server integration test. Ignored by default because it needs a real server.
//!
//! Enable it by setting `RUSTGRID_SQLSERVER_HOST` (and optionally
//! `RUSTGRID_SQLSERVER_PORT` / `USER` / `PASSWORD` / `DATABASE`) and running:
//!
//! ```text
//! cargo test -p rustgrid-sqlserver -- --ignored
//! ```

use rustgrid_core::{
    BackupObjectKind, ConnectionConfig, Driver, DriverId, ObjectKind, PageRequest, Result,
    RowInsert, RowUpdate, ViewEdit,
};
use rustgrid_sqlserver::SqlServerDriver;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

fn config() -> ConnectionConfig {
    ConnectionConfig {
        driver: DriverId::new("sqlserver"),
        host: env("RUSTGRID_SQLSERVER_HOST", "localhost"),
        port: env("RUSTGRID_SQLSERVER_PORT", "1433")
            .parse()
            .expect("RUSTGRID_SQLSERVER_PORT must be a number"),
        username: env("RUSTGRID_SQLSERVER_USER", "sa"),
        password: Some(env("RUSTGRID_SQLSERVER_PASSWORD", "")),
        database: Some(env("RUSTGRID_SQLSERVER_DATABASE", "master")),
        options: Default::default(),
        settings: Default::default(),
    }
}

#[tokio::test]
#[ignore = "requires a live SQL Server (RUSTGRID_SQLSERVER_*)"]
async fn round_trips_a_live_sql_server() {
    let config = config();
    let database = config
        .database
        .clone()
        .unwrap_or_else(|| "master".to_string());
    let connection = SqlServerDriver::new()
        .connect(&config)
        .await
        .expect("connect");

    let table = "rustgrid_live_users";
    let _ = connection
        .execute_query(
            Some(&database),
            &format!("DROP TABLE IF EXISTS [dbo].[{table}]"),
        )
        .await;
    connection
        .execute_query(
            Some(&database),
            &format!(
                "CREATE TABLE [dbo].[{table}] (\
                    [id] INT IDENTITY(1,1) PRIMARY KEY, \
                    [name] NVARCHAR(100) NOT NULL, \
                    [age] INT NULL, \
                    [created] DATETIME2 NULL)"
            ),
        )
        .await
        .expect("create table");
    connection
        .execute_query(
            Some(&database),
            &format!(
                "INSERT INTO [dbo].[{table}] ([name], [age]) VALUES (N'ann', 30), (N'bob', 25)"
            ),
        )
        .await
        .expect("insert rows");

    // Catalog.
    let databases = connection.list_databases().await.unwrap();
    assert!(databases.iter().any(|entry| entry.name == database));
    // SQL Server object names are schema-qualified in the listing (`dbo.users`).
    let tables = connection.list_tables(&database).await.unwrap();
    assert!(
        tables
            .iter()
            .any(|entry| entry.name == format!("dbo.{table}") && entry.kind == ObjectKind::Table)
    );
    let columns = connection.columns(&database, table).await.unwrap();
    assert_eq!(columns.len(), 4);
    assert!(columns[0].primary_key);
    assert!(!columns[0].nullable);

    let page = connection
        .fetch_page(&database, table, PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.total_rows, Some(2));

    // A schema-qualified name (as the tree shows it) works the same way.
    let page = connection
        .fetch_page(&database, &format!("dbo.{table}"), PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);

    // Editing.
    connection
        .update_rows(
            &database,
            table,
            &[RowUpdate {
                set: vec![("age".to_string(), Some("31".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .unwrap();
    connection
        .insert_rows(
            &database,
            table,
            &[RowInsert {
                values: vec![
                    ("name".to_string(), Some("cy".to_string())),
                    ("age".to_string(), Some("40".to_string())),
                ],
            }],
        )
        .await
        .unwrap();
    connection
        .delete_rows(
            &database,
            table,
            &[vec![("id".to_string(), Some("2".to_string()))]],
        )
        .await
        .unwrap();

    let page = connection
        .fetch_page(&database, table, PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(2));

    // Multi-statement script: one result per statement.
    let results = connection
        .execute_query_many(Some(&database), "SELECT 1 AS a; SELECT 2 AS b")
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.has_result_set));

    // Schema introspection.
    let schema = connection.table_schema(&database, table).await.unwrap();
    assert_eq!(schema.columns.len(), 4);
    assert!(schema.columns[0].auto_increment);

    // Backup metadata + row streaming + restore.
    let mut dump = connection
        .backup_object_metadata(&database, BackupObjectKind::Table, table)
        .await
        .unwrap();
    assert!(dump.ddl.to_ascii_uppercase().contains("CREATE TABLE"));
    assert_eq!(dump.fields.len(), 4);

    let mut collected: Vec<String> = Vec::new();
    let count = {
        let mut sink = |tuple: &str| -> Result<()> {
            collected.push(tuple.to_string());
            Ok(())
        };
        connection
            .stream_table_rows(&database, table, &mut sink)
            .await
            .unwrap()
    };
    assert_eq!(count, 2);
    dump.rows = collected;
    connection.restore_object(&database, &dump).await.unwrap();
    let page = connection
        .fetch_page(&database, table, PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(2));

    // Views.
    let view = "rustgrid_live_adults";
    let _ = connection
        .execute_query(
            Some(&database),
            &format!("DROP VIEW IF EXISTS [dbo].[{view}]"),
        )
        .await;
    connection
        .execute_query(
            Some(&database),
            &format!(
                "CREATE VIEW [dbo].[{view}] AS SELECT [name] FROM [dbo].[{table}] WHERE [age] >= 18"
            ),
        )
        .await
        .unwrap();
    let tables = connection.list_tables(&database).await.unwrap();
    assert!(
        tables
            .iter()
            .any(|entry| entry.name == format!("dbo.{view}") && entry.kind == ObjectKind::View)
    );
    let details = connection.view_details(&database, view).await.unwrap();
    assert!(
        details
            .definition
            .to_ascii_uppercase()
            .contains("CREATE VIEW")
    );
    connection
        .save_view(
            &database,
            Some(view),
            &ViewEdit {
                name: view.to_string(),
                definition: format!(
                    "CREATE VIEW [dbo].[{view}] AS SELECT [name] FROM [dbo].[{table}] WHERE [age] >= 21"
                ),
            },
        )
        .await
        .unwrap();
    let details = connection.view_details(&database, view).await.unwrap();
    assert!(details.definition.contains("21"));
    connection.drop_view(&database, view).await.unwrap();

    let _ = connection
        .execute_query(
            Some(&database),
            &format!("DROP TABLE IF EXISTS [dbo].[{table}]"),
        )
        .await;
    connection.close().await.unwrap();
}

//! End-to-end exercise of the SQLite driver against a real (temporary) database file. No server is
//! required, so it runs as a normal test rather than an `#[ignore]`d live test.

use rustgrid_core::{
    BackupObjectKind, ConnectionConfig, Driver, DriverId, ObjectKind, PageRequest, Result,
    RowInsert, RowUpdate, ViewEdit,
};
use rustgrid_sqlite::SqliteDriver;

fn config(path: &str) -> ConnectionConfig {
    ConnectionConfig {
        driver: DriverId::new("sqlite"),
        host: String::new(),
        port: 0,
        username: String::new(),
        password: None,
        database: Some(path.to_string()),
        options: Default::default(),
        settings: Default::default(),
    }
}

#[tokio::test]
async fn round_trips_a_sqlite_database() {
    let path = std::env::temp_dir().join(format!(
        "rustgrid-sqlite-{}-{:?}.db",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    // SQLite treats an empty file as a valid empty database; create it so the driver's
    // `create_if_missing(false)` opens it.
    std::fs::File::create(&path).expect("create the test database file");
    let path = path.to_string_lossy().into_owned();

    let connection = SqliteDriver::new()
        .connect(&config(&path))
        .await
        .expect("connect");

    connection
        .execute_query(
            None,
            "CREATE TABLE users (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                name TEXT NOT NULL, \
                age INTEGER)",
        )
        .await
        .expect("create table");
    connection
        .execute_query(
            None,
            "INSERT INTO users (name, age) VALUES ('ann', 30), ('bob', 25)",
        )
        .await
        .expect("insert rows");

    // Catalog.
    let databases = connection.list_databases().await.unwrap();
    assert_eq!(databases[0].name, "main");
    let tables = connection.list_tables("main").await.unwrap();
    assert!(
        tables
            .iter()
            .any(|table| table.name == "users" && table.kind == ObjectKind::Table)
    );
    let columns = connection.columns("main", "users").await.unwrap();
    assert_eq!(columns.len(), 3);
    assert!(columns[0].primary_key);
    assert!(!columns[0].nullable);

    let page = connection
        .fetch_page("main", "users", PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.total_rows, Some(2));

    // Editing.
    connection
        .update_rows(
            "main",
            "users",
            &[RowUpdate {
                set: vec![("age".to_string(), Some("31".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .unwrap();
    connection
        .insert_rows(
            "main",
            "users",
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
            "main",
            "users",
            &[vec![("id".to_string(), Some("2".to_string()))]],
        )
        .await
        .unwrap();

    let page = connection
        .fetch_page("main", "users", PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(2));

    // Script with several statements: one result per statement.
    let results = connection
        .execute_query_many(None, "SELECT 1; SELECT 2")
        .await
        .unwrap();
    assert_eq!(results.len(), 2);

    // Schema introspection.
    let schema = connection.table_schema("main", "users").await.unwrap();
    assert_eq!(schema.columns.len(), 3);
    assert!(schema.columns[0].auto_increment);
    assert!(!schema.columns[1].nullable);

    // Backup metadata + row streaming + restore.
    let mut dump = connection
        .backup_object_metadata("main", BackupObjectKind::Table, "users")
        .await
        .unwrap();
    assert!(dump.ddl.to_ascii_uppercase().contains("CREATE TABLE"));
    assert_eq!(dump.fields.len(), 3);

    let mut collected: Vec<String> = Vec::new();
    let count = {
        let mut sink = |tuple: &str| -> Result<()> {
            collected.push(tuple.to_string());
            Ok(())
        };
        connection
            .stream_table_rows("main", "users", &mut sink)
            .await
            .unwrap()
    };
    assert_eq!(count, 2);
    assert_eq!(collected.len(), 2);
    assert!(collected[0].starts_with('('));

    // The archive merges the streamed rows into the dump before restoring; replicate that here.
    dump.rows = collected;
    connection.restore_object("main", &dump).await.unwrap();
    let page = connection
        .fetch_page("main", "users", PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(2));

    // Views: create, list, introspect, replace and drop.
    connection
        .execute_query(
            None,
            "CREATE VIEW adult_users AS SELECT name FROM users WHERE age >= 18",
        )
        .await
        .unwrap();
    let tables = connection.list_tables("main").await.unwrap();
    assert!(
        tables
            .iter()
            .any(|table| table.name == "adult_users" && table.kind == ObjectKind::View)
    );
    let details = connection
        .view_details("main", "adult_users")
        .await
        .unwrap();
    assert!(details.definition.contains("CREATE VIEW"));
    assert_eq!(details.info.name, "adult_users");

    connection
        .save_view(
            "main",
            Some("adult_users"),
            &ViewEdit {
                name: "adult_users".to_string(),
                definition: "CREATE VIEW adult_users AS SELECT name FROM users WHERE age >= 21"
                    .to_string(),
            },
        )
        .await
        .unwrap();
    let details = connection
        .view_details("main", "adult_users")
        .await
        .unwrap();
    assert!(details.definition.contains("21"));

    connection.drop_view("main", "adult_users").await.unwrap();
    let tables = connection.list_tables("main").await.unwrap();
    assert!(!tables.iter().any(|table| table.name == "adult_users"));

    connection.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

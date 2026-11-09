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
    let ddl = connection
        .object_ddl("main", "users", false)
        .await
        .unwrap()
        .expect("the table's DDL");
    assert!(ddl.contains("CREATE TABLE"), "{ddl}");
    assert!(
        connection
            .object_ddl("main", "missing", false)
            .await
            .unwrap()
            .is_none()
    );

    let page = connection
        .fetch_page("main", "users", PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.total_rows, Some(2));

    // The Tables overview gets a real data length from `dbstat` instead of an empty cell.
    let status = connection.table_status("main", "users").await.unwrap();
    assert_eq!(status.engine.as_deref(), Some("SQLite"));
    assert_eq!(status.rows, Some(2));
    let data_length = status
        .data_length
        .expect("dbstat reports the table's pages");
    assert!(data_length > 0, "data length should be reported");
    // `id INTEGER PRIMARY KEY AUTOINCREMENT` with ids 1 and 2 inserted: the next id is 3.
    assert_eq!(status.auto_increment, Some(3));

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
    let ddl = connection
        .object_ddl("main", "adult_users", true)
        .await
        .unwrap()
        .expect("the view's DDL");
    assert!(ddl.contains("CREATE VIEW"), "{ddl}");

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

/// The Tables overview's 自动递增值: a declared `AUTOINCREMENT` continues from
/// `sqlite_sequence` (deleted ids are not reused), while a plain `INTEGER PRIMARY KEY` continues
/// from the current maximum and a composite key has no generated value at all.
#[tokio::test]
async fn table_status_reports_the_next_generated_key() {
    let path = std::env::temp_dir().join(format!(
        "rustgrid-sqlite-status-{}-{:?}.db",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::File::create(&path).expect("create the test database file");
    let path = path.to_string_lossy().into_owned();

    let connection = SqliteDriver::new()
        .connect(&config(&path))
        .await
        .expect("connect");
    for statement in [
        "CREATE TABLE auto (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
        "CREATE TABLE plain (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE composite (a INTEGER, b INTEGER, PRIMARY KEY (a, b))",
        "INSERT INTO auto (name) VALUES ('a'), ('b'), ('c')",
        "DELETE FROM auto WHERE id = 3",
        "INSERT INTO plain (name) VALUES ('x'), ('y')",
        "INSERT INTO composite (a, b) VALUES (1, 2)",
    ] {
        connection.execute_query(None, statement).await.unwrap();
    }

    let statuses = connection.table_statuses("main").await.unwrap();
    let status = |name: &str| {
        statuses
            .iter()
            .find(|(table, _)| table == name)
            .map(|(_, status)| status)
            .unwrap()
    };
    let auto = status("auto");
    assert_eq!(auto.rows, Some(2));
    // id 3 was inserted and deleted; AUTOINCREMENT still hands out 4 next.
    assert_eq!(auto.auto_increment, Some(4));
    // A rowid alias without AUTOINCREMENT follows the current maximum.
    assert_eq!(status("plain").auto_increment, Some(3));
    // A composite primary key is not generated by SQLite.
    assert_eq!(status("composite").auto_increment, None);

    connection.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

/// The table designer's write path against a real file: create a table from a `TableSchema`, read it
/// back, alter it and read it again — including a per-column `DESC` in the index.
#[tokio::test]
async fn designer_round_trips_a_table() {
    use rustgrid_core::{ColumnDef, IndexColumnDef, IndexDef, TableSchema};

    let path = std::env::temp_dir().join(format!(
        "rustgrid-sqlite-designer-{}-{:?}.db",
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

    let mut schema = TableSchema {
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
                primary_key: true,
                auto_increment: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "code".to_string(),
                data_type: "VARCHAR".to_string(),
                length: "64".to_string(),
                nullable: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "amount".to_string(),
                data_type: "REAL".to_string(),
                nullable: true,
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
        index_type: "BTREE".to_string(),
        comment: String::new(),
    });
    connection
        .save_table_schema("main", "items", None, &schema)
        .await
        .expect("create the table");

    let loaded = connection
        .table_schema("main", "items")
        .await
        .expect("read");
    assert!(
        loaded
            .columns
            .iter()
            .any(|column| column.name == "id" && column.primary_key && column.auto_increment),
        "the identity column did not round trip: {:?}",
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

    // Alter: a new column, and the index back to an ascending list.
    let mut modified = loaded.clone();
    modified.columns.push(ColumnDef {
        name: "note".to_string(),
        data_type: "TEXT".to_string(),
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
        .save_table_schema("main", "items", Some(&loaded), &modified)
        .await
        .expect("alter the table");

    let reloaded = connection
        .table_schema("main", "items")
        .await
        .expect("read the altered table");
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

    connection.drop_table("main", "items").await.expect("drop");
    connection.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

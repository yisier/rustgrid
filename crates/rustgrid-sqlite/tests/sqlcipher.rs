//! SQLCipher support: an encrypted database is created with sqlx's own SQLCipher pragmas (the
//! SQLCipher build is what the driver links), then read back through the driver with its key.
//! SQLCipher is compiled with libtomcrypt, so this runs without any OpenSSL installation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rustgrid_core::{ConnectionConfig, Driver, DriverId, Error, PageRequest};
use rustgrid_sqlite::SqliteDriver;
use sqlx::AssertSqlSafe;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

fn temp_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "rustgrid-sqlcipher-{}-{name}-{:?}.db",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn config(path: &Path, password: Option<&str>, encryption: &str) -> ConnectionConfig {
    let mut options = BTreeMap::new();
    options.insert("sqlite.encryption".to_string(), encryption.to_string());
    ConnectionConfig {
        driver: DriverId::new("sqlite"),
        host: String::new(),
        port: 0,
        username: String::new(),
        password: password.map(str::to_string),
        database: Some(path.to_string_lossy().into_owned()),
        options,
        settings: Default::default(),
    }
}

/// Create an encrypted database whose key is `key` (a plain passphrase, not a SQL literal).
async fn create_encrypted(path: &Path, key: &str, compatibility: Option<u8>) {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    let key = key.replace('\'', "''");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .after_connect(move |connection, _metadata| {
            let key = key.clone();
            Box::pin(async move {
                let mut sql = format!("PRAGMA key = '{key}';");
                if let Some(compatibility) = compatibility {
                    sql.push_str(&format!(" PRAGMA cipher_compatibility = {compatibility};"));
                }
                sqlx::raw_sql(AssertSqlSafe(sql))
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .expect("create the encrypted database");
    sqlx::query(
        "CREATE TABLE secrets (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, \
         note TEXT)",
    )
    .execute(&pool)
    .await
    .expect("create table");
    sqlx::query("INSERT INTO secrets (name, note) VALUES ('ann', 'first'), ('bob', NULL)")
        .execute(&pool)
        .await
        .expect("insert rows");
    pool.close().await;
}

#[tokio::test]
async fn opens_a_sqlcipher4_database() {
    let path = temp_path("v4");
    create_encrypted(&path, "correct horse", None).await;

    let connection = SqliteDriver::new()
        .connect(&config(&path, Some("correct horse"), "sqlcipher4"))
        .await
        .expect("connect with the right password");
    let tables = connection.list_tables("main").await.unwrap();
    assert!(tables.iter().any(|table| table.name == "secrets"));
    let page = connection
        .fetch_page("main", "secrets", PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.total_rows, Some(2));
    let _ = connection.close().await;

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_wrong_or_missing_sqlcipher_key_is_an_authentication_failure() {
    let path = temp_path("wrong");
    create_encrypted(&path, "right key", None).await;

    let wrong = SqliteDriver::new()
        .connect(&config(&path, Some("wrong key"), "sqlcipher4"))
        .await;
    let wrong = match wrong {
        Ok(connection) => {
            let _ = connection.close().await;
            panic!("a wrong key must not connect");
        }
        Err(error) => error,
    };
    assert!(
        matches!(wrong, Error::Authentication(_)),
        "a wrong key must ask for the password again, got: {wrong}"
    );

    let missing = SqliteDriver::new()
        .connect(&config(&path, None, "sqlcipher4"))
        .await;
    assert!(
        matches!(missing, Err(Error::Authentication(_))),
        "an encrypted database requires a key"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn opens_a_sqlcipher3_database_with_its_compatibility_level() {
    let path = temp_path("v3");
    create_encrypted(&path, "legacy key", Some(3)).await;

    let connection = SqliteDriver::new()
        .connect(&config(&path, Some("legacy key"), "sqlcipher3"))
        .await
        .expect("connect with SQLCipher 3 compatibility");
    let page = connection
        .fetch_page("main", "secrets", PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    let _ = connection.close().await;

    // The same file under the v4 defaults is not readable.
    let v4 = SqliteDriver::new()
        .connect(&config(&path, Some("legacy key"), "sqlcipher4"))
        .await;
    assert!(
        matches!(v4, Err(Error::Authentication(_))),
        "a v3 database needs the v3 compatibility level"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_key_with_quotes_and_semicolons_round_trips() {
    let path = temp_path("quoted");
    let key = "pa'ss;word -- not SQL";
    create_encrypted(&path, key, None).await;

    let connection = SqliteDriver::new()
        .connect(&config(&path, Some(key), "sqlcipher4"))
        .await
        .expect("connect with a key containing SQL punctuation");
    let tables = connection.list_tables("main").await.unwrap();
    assert!(tables.iter().any(|table| table.name == "secrets"));
    let _ = connection.close().await;

    let _ = std::fs::remove_file(&path);
}

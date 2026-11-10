//! Developer helper: writes the sample databases used to verify SQLCipher support by hand.
//! Run with `cargo run -p rustgrid-sqlite --example make_samples`.
//!
//! Output (under `target/sqlcipher-samples/`):
//! - `sqlcipher4.db` — SQLCipher 4 defaults, password `rustgrid4`
//! - `sqlcipher3.db` — SQLCipher 3 compatibility, password `rustgrid3`
//! - `plain.db`      — unencrypted SQLite

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rustgrid_core::{ConnectionConfig, Driver, DriverId, PageRequest};
use rustgrid_sqlite::SqliteDriver;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{AssertSqlSafe, SqlitePool};

fn sample_path(name: &str) -> PathBuf {
    Path::new("target/sqlcipher-samples").join(name)
}

async fn create(path: &Path, key: Option<&str>, compatibility: Option<u8>) {
    let _ = std::fs::remove_file(path);
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = match key {
        None => SqlitePool::connect_with(options)
            .await
            .expect("create the plain database"),
        Some(key) => {
            let key = key.replace('\'', "''");
            SqlitePoolOptions::new()
                .max_connections(1)
                .after_connect(move |connection, _metadata| {
                    let key = key.clone();
                    Box::pin(async move {
                        let mut sql = format!("PRAGMA key = '{key}';");
                        if let Some(compatibility) = compatibility {
                            sql.push_str(&format!(
                                " PRAGMA cipher_compatibility = {compatibility};"
                            ));
                        }
                        sqlx::raw_sql(AssertSqlSafe(sql))
                            .execute(&mut *connection)
                            .await?;
                        Ok(())
                    })
                })
                .connect_with(options)
                .await
                .expect("create the encrypted database")
        }
    };
    sqlx::query(
        "CREATE TABLE users (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL, \
            email TEXT, \
            joined TEXT)",
    )
    .execute(&pool)
    .await
    .expect("create table");
    sqlx::query(
        "INSERT INTO users (name, email, joined) VALUES \
         ('Alice', 'alice@example.com', '2025-01-15'), \
         ('Bob', 'bob@example.com', '2025-06-26'), \
         ('Carol', NULL, NULL)",
    )
    .execute(&pool)
    .await
    .expect("insert rows");
    pool.close().await;
    println!("created {}", path.display());
}

/// Read the sample back through the actual driver, which is the same path the application uses.
async fn read(path: &Path, password: Option<&str>, encryption: &str) {
    let mut options = BTreeMap::new();
    options.insert("sqlite.encryption".to_string(), encryption.to_string());
    let config = ConnectionConfig {
        driver: DriverId::new("sqlite"),
        host: String::new(),
        port: 0,
        username: String::new(),
        password: password.map(str::to_string),
        database: Some(path.to_string_lossy().into_owned()),
        options,
        settings: Default::default(),
    };
    let connection = SqliteDriver::new()
        .connect(&config)
        .await
        .expect("open the sample through the driver");
    let page = connection
        .fetch_page("main", "users", PageRequest::new(0, 10))
        .await
        .expect("read the sample table");
    println!(
        "  driver read {}: {} rows, {} columns",
        path.display(),
        page.rows.len(),
        page.columns.len()
    );
    let _ = connection.close().await;
}

#[tokio::main]
async fn main() {
    std::fs::create_dir_all("target/sqlcipher-samples").expect("create the output directory");
    create(&sample_path("plain.db"), None, None).await;
    read(&sample_path("plain.db"), None, "none").await;
    create(&sample_path("sqlcipher4.db"), Some("rustgrid4"), None).await;
    read(
        &sample_path("sqlcipher4.db"),
        Some("rustgrid4"),
        "sqlcipher4",
    )
    .await;
    create(&sample_path("sqlcipher3.db"), Some("rustgrid3"), Some(3)).await;
    read(
        &sample_path("sqlcipher3.db"),
        Some("rustgrid3"),
        "sqlcipher3",
    )
    .await;
}

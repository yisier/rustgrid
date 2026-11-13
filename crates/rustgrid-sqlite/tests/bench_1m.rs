//! Live 1M-row benchmark for the SQLite decode path.
//!
//! SQLite is file-based, so this test creates and seeds its own temporary database. Run:
//!
//! ```text
//! cargo test --release -p rustgrid-sqlite --test bench_1m -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::time::Instant;

use rustgrid_core::{CellValue, ConnectionConfig, Driver, DriverId};
use rustgrid_sqlite::SqliteDriver;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, TypeInfo, ValueRef};

/// The old every-storage-class probe, kept to measure the optimization.
fn old_decode(row: &sqlx::sqlite::SqliteRow, index: usize) -> CellValue {
    let is_null = row
        .try_get_raw(index)
        .map(|value| value.is_null())
        .unwrap_or(true);
    if is_null {
        return CellValue::Null;
    }
    if let Ok(value) = row.try_get::<i64, _>(index) {
        return CellValue::Int(value);
    }
    if let Ok(value) = row.try_get::<f64, _>(index) {
        return CellValue::Float(value);
    }
    if let Ok(value) = row.try_get::<String, _>(index) {
        return CellValue::Text(value);
    }
    if let Ok(bytes) = row.try_get::<Vec<u8>, _>(index) {
        return match String::from_utf8(bytes) {
            Ok(text) => CellValue::Text(text),
            Err(error) => CellValue::Bytes(error.into_bytes()),
        };
    }
    CellValue::Null
}

/// The current storage-class dispatch.
fn new_decode(row: &sqlx::sqlite::SqliteRow, index: usize) -> CellValue {
    let Ok(raw) = row.try_get_raw(index) else {
        return CellValue::Null;
    };
    if raw.is_null() {
        return CellValue::Null;
    }
    let type_info = raw.type_info();
    match type_info.name() {
        "INTEGER" => row
            .try_get::<i64, _>(index)
            .map_or(CellValue::Null, CellValue::Int),
        "REAL" => row
            .try_get::<f64, _>(index)
            .map_or(CellValue::Null, CellValue::Float),
        "TEXT" => row
            .try_get::<String, _>(index)
            .map_or(CellValue::Null, CellValue::Text),
        "BLOB" => match row.try_get::<Vec<u8>, _>(index) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => CellValue::Text(text),
                Err(error) => CellValue::Bytes(error.into_bytes()),
            },
            Err(_) => CellValue::Null,
        },
        _ => old_decode(row, index),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: creates a temporary 1M-row SQLite database"]
async fn bench_1m() {
    let path = std::env::temp_dir().join("rustgrid_bench_1m.sqlite");
    let _ = std::fs::remove_file(&path);
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let started = Instant::now();
    sqlx::query(
        "CREATE TABLE bench_1m (id INTEGER PRIMARY KEY, name TEXT, qty INTEGER, price REAL, created TEXT, note TEXT)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < 1000000) \
         INSERT INTO bench_1m SELECT x, 'user_' || (x % 1000), x % 1000, \
         (x % 10000) / 100.0 + 0.5, '2024-01-01 12:00:00', 'note ' || x FROM cnt",
    )
    .execute(&pool)
    .await
    .unwrap();
    eprintln!("sqlite seed: {:?}", started.elapsed());

    let started = Instant::now();
    let rows = sqlx::query("SELECT * FROM bench_1m")
        .fetch_all(&pool)
        .await
        .unwrap();
    eprintln!(
        "sqlite raw transport: {:?} rows {}",
        started.elapsed(),
        rows.len()
    );
    let started = Instant::now();
    let mut count = 0usize;
    for row in &rows {
        for index in 0..6 {
            count += usize::from(!matches!(old_decode(row, index), CellValue::Null));
        }
    }
    eprintln!(
        "sqlite old probe decode: {:?} cells {count}",
        started.elapsed()
    );
    let started = Instant::now();
    let mut count = 0usize;
    for row in &rows {
        for index in 0..6 {
            count += usize::from(!matches!(new_decode(row, index), CellValue::Null));
        }
    }
    eprintln!(
        "sqlite new dispatch decode: {:?} cells {count}",
        started.elapsed()
    );
    drop(rows);
    pool.close().await;

    let config = ConnectionConfig {
        driver: DriverId::new("sqlite"),
        host: String::new(),
        port: 0,
        username: String::new(),
        password: None,
        database: Some(path.to_string_lossy().to_string()),
        options: BTreeMap::new(),
        settings: Default::default(),
    };
    let connection = SqliteDriver::new().connect(&config).await.unwrap();
    let started = Instant::now();
    let results = connection
        .execute_query_many(Some("main"), "SELECT * FROM bench_1m")
        .await
        .expect("query");
    eprintln!(
        "sqlite rustgrid decode: {:?} rows {}",
        started.elapsed(),
        results.first().map(|result| result.rows.len()).unwrap_or(0)
    );
    assert!(matches!(results[0].rows[0][0], CellValue::Int(1)));
    assert!(matches!(results[0].rows[0][1], CellValue::Text(_)));
    connection.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

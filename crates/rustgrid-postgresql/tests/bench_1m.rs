//! Live 1M-row benchmark for the PostgreSQL decode path.
//!
//! The table must exist first:
//!
//! ```sql
//! CREATE TABLE bench_1m (
//!   id int PRIMARY KEY, name varchar(40), qty int, price numeric(10,2),
//!   created timestamp, note text
//! );
//! INSERT INTO bench_1m
//! SELECT i, 'user_' || (i % 1000), i % 1000,
//!        ((i % 10000) / 100.0 + 0.5)::numeric(10,2),
//!        '2024-01-01 12:00:00'::timestamp, 'note ' || i
//! FROM generate_series(1, 1000000) AS i;
//! ```
//!
//! Run:
//!
//! ```text
//! BENCH_PORT=5432 BENCH_DATABASE=appdb BENCH_PASSWORD=Root123! \
//!   cargo test --release -p rustgrid-postgresql --test bench_1m -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::time::Instant;

use futures_util::TryStreamExt;
use rustgrid_core::{CellValue, ConnectionConfig, Driver, DriverId};
use rustgrid_postgresql::PostgresDriver;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: needs a live PostgreSQL with bench_1m"]
async fn bench_1m() {
    let port: u16 = env("BENCH_PORT", "5432").parse().unwrap();
    let database = env("BENCH_DATABASE", "appdb");
    let password = env("BENCH_PASSWORD", "Root123!");
    let url = format!("postgres://postgres:{password}@127.0.0.1:{port}/{database}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let started = Instant::now();
    let mut count = 0usize;
    let mut stream = sqlx::raw_sql("SELECT * FROM bench_1m").fetch_many(&mut *connection);
    while let Some(item) = stream.try_next().await.unwrap() {
        if let sqlx::Either::Right(_) = item {
            count += 1;
        }
    }
    eprintln!(
        "postgres raw transport: {:?} rows {count}",
        started.elapsed()
    );
    drop(stream);
    drop(connection);
    pool.close().await;

    let config = ConnectionConfig {
        driver: DriverId::new("postgresql"),
        host: "127.0.0.1".to_string(),
        port,
        username: "postgres".to_string(),
        password: Some(password),
        database: Some(database.clone()),
        options: BTreeMap::new(),
        settings: Default::default(),
    };
    let connection = PostgresDriver::new().connect(&config).await.unwrap();
    let started = Instant::now();
    let results = connection
        .execute_query_many(Some(&database), "SELECT * FROM bench_1m")
        .await
        .expect("query");
    eprintln!(
        "postgres rustgrid decode: {:?} rows {}",
        started.elapsed(),
        results.first().map(|result| result.rows.len()).unwrap_or(0)
    );
    assert!(matches!(results[0].rows[0][0], CellValue::Int(1)));
    assert!(matches!(results[0].rows[0][1], CellValue::Text(_)));
    connection.close().await.unwrap();
}

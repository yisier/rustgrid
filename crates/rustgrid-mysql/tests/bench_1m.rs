//! Live 1M-row benchmark for the MySQL/MariaDB decode path.
//!
//! The table must exist first:
//!
//! ```sql
//! CREATE TABLE bench_1m (
//!   id INT PRIMARY KEY, name VARCHAR(40), qty INT, price DECIMAL(10,2),
//!   created DATETIME, note TEXT
//! );
//! -- fill 1,000,000 rows (see the benchmark notes in the PR/issue)
//! ```
//!
//! Run against MySQL (3306/appdb) or MariaDB (3307/UDAL_TEST):
//!
//! ```text
//! BENCH_ENGINE=mysql BENCH_PORT=3306 BENCH_DATABASE=appdb \
//!   cargo test --release -p rustgrid-mysql --test bench_1m -- --ignored --nocapture
//! ```
//!
//! `BENCH_PASSWORD` defaults to `Root123!`.

use std::collections::BTreeMap;
use std::time::Instant;

use futures_util::TryStreamExt;
use rustgrid_core::{CellValue, ConnectionConfig, Driver, DriverId};
use rustgrid_mysql::{MariaDbDriver, MysqlDriver};

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: needs a live MySQL/MariaDB with bench_1m"]
async fn bench_1m() {
    let engine = env("BENCH_ENGINE", "mysql");
    let port: u16 = env("BENCH_PORT", "3306").parse().unwrap();
    let database = env("BENCH_DATABASE", "appdb");
    let password = env("BENCH_PASSWORD", "Root123!");
    let url = format!("mysql://root:{password}@127.0.0.1:{port}/{database}");
    let pool = sqlx::mysql::MySqlPoolOptions::new()
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
        "{engine} raw transport: {:?} rows {count}",
        started.elapsed()
    );
    drop(stream);
    drop(connection);
    pool.close().await;

    let config = ConnectionConfig {
        driver: DriverId::new(&engine),
        host: "127.0.0.1".to_string(),
        port,
        username: "root".to_string(),
        password: Some(password),
        database: Some(database.clone()),
        options: BTreeMap::new(),
        settings: Default::default(),
    };
    let connection = if engine == "mariadb" {
        MariaDbDriver::new().connect(&config).await.unwrap()
    } else {
        MysqlDriver::new().connect(&config).await.unwrap()
    };
    let started = Instant::now();
    let results = connection
        .execute_query_many(Some(&database), "SELECT * FROM bench_1m")
        .await
        .expect("query");
    eprintln!(
        "{engine} rustgrid decode: {:?} rows {}",
        started.elapsed(),
        results.first().map(|result| result.rows.len()).unwrap_or(0)
    );
    assert!(matches!(results[0].rows[0][0], CellValue::Int(1)));
    assert!(matches!(results[0].rows[0][1], CellValue::Text(_)));
    connection.close().await.unwrap();
}

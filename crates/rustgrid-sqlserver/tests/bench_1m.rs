//! Live 1M-row benchmark for the SQL Server decode path.
//!
//! The table must exist first (the recursive CTE needs `MAXRECURSION 0`):
//!
//! ```sql
//! CREATE TABLE dbo.bench_1m (
//!   id int PRIMARY KEY, name varchar(40), qty int, price decimal(10,2),
//!   created datetime2, note nvarchar(max)
//! );
//! WITH n AS (SELECT 1 AS i UNION ALL SELECT i + 1 FROM n WHERE i < 1000000)
//! INSERT INTO dbo.bench_1m
//! SELECT i, 'user_' + CAST(i % 1000 AS varchar(10)), i % 1000,
//!        CAST((i % 10000) / 100.0 + 0.5 AS decimal(10,2)),
//!        CAST('2024-01-01T12:00:00' AS datetime2),
//!        N'note ' + CAST(i AS nvarchar(10))
//! FROM n OPTION (MAXRECURSION 0);
//! ```
//!
//! Run:
//!
//! ```text
//! cargo test --release -p rustgrid-sqlserver --test bench_1m -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::time::Instant;

use rustgrid_core::{CellValue, ConnectionConfig, Driver, DriverId};
use rustgrid_sqlserver::SqlServerDriver;
use tiberius::{AuthMethod, Client, Config};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: needs a live SQL Server with dbo.bench_1m"]
async fn bench_1m() {
    let mut config = Config::new();
    config.host("127.0.0.1");
    config.port(1433);
    config.authentication(AuthMethod::sql_server("sa", "Root123!"));
    config.trust_cert();
    config.database("master");
    let tcp = TcpStream::connect("127.0.0.1:1433").await.unwrap();
    tcp.set_nodelay(true).unwrap();
    let mut client = Client::connect(config, tcp.compat_write()).await.unwrap();
    let started = Instant::now();
    let rows = client
        .query("SELECT * FROM dbo.bench_1m", &[])
        .await
        .unwrap()
        .into_first_result()
        .await
        .unwrap();
    eprintln!(
        "sqlserver raw transport: {:?} rows {}",
        started.elapsed(),
        rows.len()
    );

    let config = ConnectionConfig {
        driver: DriverId::new("sqlserver"),
        host: "127.0.0.1".to_string(),
        port: 1433,
        username: "sa".to_string(),
        password: Some("Root123!".to_string()),
        database: Some("master".to_string()),
        options: BTreeMap::new(),
        settings: Default::default(),
    };
    let connection = SqlServerDriver::new().connect(&config).await.unwrap();
    let started = Instant::now();
    let results = connection
        .execute_query_many(Some("master"), "SELECT * FROM dbo.bench_1m")
        .await
        .expect("query");
    eprintln!(
        "sqlserver rustgrid decode: {:?} rows {}",
        started.elapsed(),
        results.first().map(|result| result.rows.len()).unwrap_or(0)
    );
    assert!(matches!(results[0].rows[0][0], CellValue::Int(1)));
    assert!(matches!(results[0].rows[0][1], CellValue::Text(_)));
    connection.close().await.unwrap();
}

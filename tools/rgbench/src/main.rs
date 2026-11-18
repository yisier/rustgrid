//! Benchmark harness for the RustGrid documentation site.
//!
//! Runs the operations the grid actually performs (first page, deep page, count, filtered page and
//! primary-key lookups) through RustGrid's own driver crates, against the 1M / 10M row data sets
//! created by `tools/rgbench/sql/*`.
//!
//! ```text
//! rgbench --config tools/rgbench/config.local.json --out docs/site/.data/bench.json
//! ```
//!
//! Every operation runs once as a warm-up and then three times; the median is reported. Point
//! lookups time `--point-lookups` random primary keys and report the average.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use rustgrid_core::{
    Connection, ConnectionConfig, ConnectionOptions, Driver, DriverId, FilterCondition,
    FilterConjunction, FilterNode, FilterOperator, PageRequest, SortColumn,
};
use serde_json::{Map, Value, json};

use rustgrid_clickhouse::ClickHouseDriver;
use rustgrid_mongodb::MongoDriver;
use rustgrid_mysql::MysqlDriver;
use rustgrid_oracle::OracleDriver;
use rustgrid_postgresql::PostgresDriver;
use rustgrid_sqlite::SqliteDriver;

const PAGE: u64 = 1000;
const LOOKUPS: usize = 30;
const RUNS: usize = 3;

struct Args {
    config: String,
    out: String,
    only: Vec<String>,
}

struct EngineConfig {
    host: String,
    port: u16,
    username: String,
    password: Option<String>,
    database: String,
    schema: String,
    path: String,
    options: BTreeMap<String, String>,
}

fn main() {
    let args = parse_args();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = runtime.block_on(run(args));
    std::process::exit(code);
}

fn parse_args() -> Args {
    let mut args = Args {
        config: "tools/rgbench/config.local.json".to_string(),
        out: "tools/rgbench/bench.json".to_string(),
        only: Vec::new(),
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => args.config = iter.next().expect("--config needs a path"),
            "--out" => args.out = iter.next().expect("--out needs a path"),
            "--only" => {
                args.only = iter
                    .next()
                    .expect("--only needs a list")
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            "--help" | "-h" => {
                println!("rgbench --config <file> --out <file> [--only mysql,postgresql]");
                std::process::exit(0);
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    args
}

async fn run(args: Args) -> i32 {
    let text = std::fs::read_to_string(&args.config).unwrap_or_else(|error| {
        panic!("cannot read {}: {error}", args.config);
    });
    let root: Value = serde_json::from_str(&text).expect("config is not valid JSON");
    let root = root.as_object().expect("config must be a JSON object");

    let mut report = Map::new();
    let mut engines_report = Map::new();
    let mut failures = Map::new();

    for (name, value) in root {
        if !args.only.is_empty() && !args.only.iter().any(|only| only == name) {
            continue;
        }
        let Some(engine) = value.as_object() else {
            continue;
        };
        let config = engine_config(engine);
        eprintln!("== {name} ==");
        let driver: Box<dyn Driver> = match name.as_str() {
            "mysql" => Box::new(MysqlDriver::new()),
            "postgresql" => Box::new(PostgresDriver::new()),
            "sqlite" => Box::new(SqliteDriver::new()),
            "clickhouse" => Box::new(ClickHouseDriver::new()),
            "oracle" => Box::new(OracleDriver::new()),
            "mongodb" => Box::new(MongoDriver::new()),
            other => {
                failures.insert(other.to_string(), json!("unsupported engine"));
                continue;
            }
        };

        match bench_engine(name, driver.as_ref(), &config).await {
            Ok(value) => {
                eprintln!("   collected {}", engine_summary_line(&value));
                engines_report.insert(name.clone(), value);
            }
            Err(error) => {
                eprintln!("   failed: {error}");
                failures.insert(name.clone(), json!(error.to_string()));
            }
        }
    }

    report.insert(
        "generated_at".to_string(),
        json!(chrono::Utc::now().to_rfc3339()),
    );
    report.insert("pagerows".to_string(), json!(PAGE));
    report.insert("lookups".to_string(), json!(LOOKUPS));
    report.insert("engines".to_string(), Value::Object(engines_report));
    report.insert("failures".to_string(), Value::Object(failures));

    let out = Value::Object(report);
    let text = serde_json::to_string_pretty(&out).expect("serialize report");
    if let Some(parent) = std::path::Path::new(&args.out).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&args.out, text).expect("write report");
    println!("written {}", args.out);
    0
}

fn engine_config(map: &Map<String, Value>) -> EngineConfig {
    let text = |key: &str, fallback: &str| {
        map.get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_string()
    };
    let database = text("database", "");
    let schema = match map.get("schema").and_then(Value::as_str) {
        Some(schema) if !schema.is_empty() => schema.to_string(),
        _ => database.clone(),
    };
    let options = map
        .get("options")
        .and_then(Value::as_object)
        .map(|options| {
            options
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    EngineConfig {
        host: text("host", "127.0.0.1"),
        port: map.get("port").and_then(Value::as_u64).unwrap_or(0) as u16,
        username: text("username", ""),
        password: map
            .get("password")
            .and_then(Value::as_str)
            .filter(|password| !password.is_empty())
            .map(str::to_string),
        database,
        schema,
        path: text("path", ""),
        options,
    }
}

impl EngineConfig {
    fn connection_config(&self, driver: &str) -> ConnectionConfig {
        let (host, port, database) = if driver == "sqlite" {
            (String::new(), 0, Some(self.path.clone()))
        } else {
            (
                self.host.clone(),
                self.port,
                Some(self.database.clone()).filter(|value| !value.is_empty()),
            )
        };
        ConnectionConfig {
            driver: DriverId::new(driver),
            host,
            port,
            username: self.username.clone(),
            password: self.password.clone(),
            database,
            options: self.options.clone(),
            settings: ConnectionOptions {
                connect_timeout: Some(120),
                query_timeout: Some(300),
                ..Default::default()
            },
        }
    }

    /// The schema argument the drivers take (`database` for every engine except Oracle, whose
    /// driver resolves objects in its current schema).
    fn object_schema(&self, driver: &str) -> String {
        if driver == "oracle" {
            String::new()
        } else {
            self.schema.clone()
        }
    }
}

async fn bench_engine(
    name: &str,
    driver: &dyn Driver,
    config: &EngineConfig,
) -> Result<Value, Box<dyn std::error::Error>> {
    let connect_started = Instant::now();
    let connection = driver.connect(&config.connection_config(name)).await?;
    let connect = connect_started.elapsed();
    let version = connection.server_version().await.unwrap_or_default();
    let schema = config.object_schema(name);

    let mut tables = Map::new();
    for (table, rows) in [("orders_1m", 1_000_000u64), ("orders_10m", 10_000_000u64)] {
        eprintln!("   {table}");
        // Oracle folds unquoted DDL names to upper case, and the driver quotes what it is given.
        let table_arg = if name == "oracle" {
            table.to_uppercase()
        } else {
            table.to_string()
        };
        let table_ref = table_arg.as_str();
        // Warm the caches the way an open grid would.
        let _ = connection
            .fetch_page(
                &schema,
                table_ref,
                PageRequest::new(0, PAGE).with_skip_total(true),
            )
            .await?;
        let _ = connection.count_rows(&schema, table_ref, &[]).await?;

        let (count, count_ms) = median(|| {
            let connection = &connection;
            let schema = schema.clone();
            async move {
                connection
                    .count_rows(&schema, table_ref, &[])
                    .await
                    .map(|value| value.unwrap_or(0))
                    .map_err(|error| error.to_string())
            }
        })
        .await?;

        let first_page = median_page(&connection, &schema, table_ref, || {
            PageRequest::new(0, PAGE).with_skip_total(true)
        })
        .await?;

        let deep_100k = median_page(&connection, &schema, table_ref, || {
            PageRequest::new(100, PAGE).with_skip_total(true)
        })
        .await?;

        let deep_1m = if rows >= 10_000_000 {
            Some(
                median_page(&connection, &schema, table_ref, || {
                    PageRequest::new(1000, PAGE).with_skip_total(true)
                })
                .await?,
            )
        } else {
            None
        };

        let status_column = if name == "oracle" { "STATUS" } else { "status" };
        let created_column = if name == "oracle" {
            "CREATED_AT"
        } else {
            "created_at"
        };

        let filtered = median_page(&connection, &schema, table_ref, || {
            PageRequest::new(0, 100)
                .with_skip_total(true)
                .with_filter(vec![equality(status_column, "paid")])
                .with_order_by(vec![SortColumn {
                    column: created_column.to_string(),
                    descending: true,
                }])
        })
        .await?;

        let id_column = match name {
            "mongodb" => "_id",
            "oracle" => "ID",
            _ => "id",
        };
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut lookup_total = Duration::ZERO;
        let mut lookup_rows = 0usize;
        for _ in 0..LOOKUPS {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let id = 1 + seed % rows;
            let started = Instant::now();
            let page = connection
                .fetch_page(
                    &schema,
                    table_ref,
                    PageRequest::new(0, 1)
                        .with_skip_total(true)
                        .with_filter(vec![equality(id_column, &id.to_string())]),
                )
                .await?;
            lookup_total += started.elapsed();
            lookup_rows += page.rows.len();
        }

        let mut ops = Map::new();
        ops.insert("count_ms".to_string(), json!(count_ms));
        ops.insert("first_page_ms".to_string(), json!(f64_ms(first_page)));
        ops.insert("deep_100k_ms".to_string(), json!(f64_ms(deep_100k)));
        if let Some(deep_1m) = deep_1m {
            ops.insert("deep_1m_ms".to_string(), json!(f64_ms(deep_1m)));
        }
        ops.insert("filtered_ms".to_string(), json!(f64_ms(filtered)));
        ops.insert(
            "point_ms".to_string(),
            json!(f64_ms(lookup_total.as_secs_f64() * 1000.0 / LOOKUPS as f64)),
        );
        ops.insert("point_found".to_string(), json!(lookup_rows));

        tables.insert(
            table.to_string(),
            json!({
                "rows": count,
                "ops": Value::Object(ops),
            }),
        );
    }
    connection.close().await?;

    Ok(json!({
        "version": version,
        "connect_ms": ms(connect),
        "tables": Value::Object(tables),
    }))
}

async fn median_page<F>(
    connection: &Box<dyn Connection>,
    schema: &str,
    table: &str,
    request: F,
) -> Result<f64, Box<dyn std::error::Error>>
where
    F: Fn() -> PageRequest,
{
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let page = connection.fetch_page(schema, table, request()).await?;
        let elapsed = started.elapsed();
        std::hint::black_box(page.rows.len());
        samples.push(elapsed);
    }
    samples.sort();
    let median = samples[samples.len() / 2];
    Ok(median.as_secs_f64() * 1000.0)
}

async fn median<F, Fut, T>(mut run: F) -> Result<(T, f64), Box<dyn std::error::Error>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut samples = Vec::with_capacity(RUNS);
    let mut value = None;
    for _ in 0..RUNS {
        let started = Instant::now();
        value = Some(run().await?);
        samples.push(started.elapsed());
    }
    samples.sort();
    let median = samples[samples.len() / 2];
    Ok((
        value.expect("at least one run"),
        median.as_secs_f64() * 1000.0,
    ))
}

fn equality(column: &str, value: &str) -> FilterNode {
    FilterNode::Condition(FilterCondition {
        column: column.to_string(),
        operator: FilterOperator::Equal,
        value: value.to_string(),
        value2: String::new(),
        conjunction: FilterConjunction::And,
        enabled: true,
    })
}

fn ms(duration: Duration) -> f64 {
    round(duration.as_secs_f64() * 1000.0)
}

fn f64_ms(value: f64) -> f64 {
    round(value)
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn engine_summary_line(value: &Value) -> String {
    let table = |name: &str, op: &str| {
        value["tables"][name]["ops"][op]
            .as_f64()
            .unwrap_or_default()
    };
    format!(
        "1M first={:.1}ms deep={:.1}ms | 10M first={:.1}ms deep1M={:.1}ms",
        table("orders_1m", "first_page_ms"),
        table("orders_1m", "deep_100k_ms"),
        table("orders_10m", "first_page_ms"),
        table("orders_10m", "deep_1m_ms"),
    )
}

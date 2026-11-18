-- RustGrid docs benchmark data set: ClickHouse.
CREATE DATABASE IF NOT EXISTS rgbench;

DROP TABLE IF EXISTS rgbench.orders_1m;
CREATE TABLE rgbench.orders_1m (
  id UInt64,
  user_id UInt32,
  status LowCardinality(String),
  amount Decimal(12,2),
  created_at DateTime,
  note String
) ENGINE = MergeTree ORDER BY id;

INSERT INTO rgbench.orders_1m
SELECT number + 1,
       toUInt32((number + 1) % 10000),
       ['paid','pending','shipped','refunded','cancelled'][1 + ((number + 1) % 5)],
       toDecimal64(((number + 1) % 100000) / 100, 2),
       toDateTime('2025-01-01 00:00:00') + ((number + 1) % 31536000),
       concat('note-', toString((number + 1) % 997))
FROM numbers(1000000);

DROP TABLE IF EXISTS rgbench.orders_10m;
CREATE TABLE rgbench.orders_10m AS rgbench.orders_1m;

INSERT INTO rgbench.orders_10m
SELECT number + 1,
       toUInt32((number + 1) % 10000),
       ['paid','pending','shipped','refunded','cancelled'][1 + ((number + 1) % 5)],
       toDecimal64(((number + 1) % 100000) / 100, 2),
       toDateTime('2025-01-01 00:00:00') + ((number + 1) % 31536000),
       concat('note-', toString((number + 1) % 997))
FROM numbers(10000000);

SELECT 'clickhouse' AS engine, (SELECT count() FROM rgbench.orders_1m) AS rows_1m, (SELECT count() FROM rgbench.orders_10m) AS rows_10m;

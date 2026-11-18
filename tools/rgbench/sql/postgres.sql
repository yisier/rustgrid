-- RustGrid docs benchmark data set: PostgreSQL.
\set ON_ERROR_STOP on
DROP DATABASE IF EXISTS rgbench;
CREATE DATABASE rgbench;
\connect rgbench

DROP TABLE IF EXISTS orders_1m;
CREATE TABLE orders_1m (
  id bigint PRIMARY KEY,
  user_id int NOT NULL,
  status varchar(16) NOT NULL,
  amount numeric(12,2) NOT NULL,
  created_at timestamp NOT NULL,
  note varchar(64) NOT NULL
);

INSERT INTO orders_1m
SELECT i,
       i % 10000,
       (ARRAY['paid','pending','shipped','refunded','cancelled'])[1 + (i % 5)],
       ((i % 100000) / 100.0)::numeric(12,2),
       timestamp '2025-01-01 00:00:00' + (i % 31536000) * interval '1 second',
       'note-' || (i % 997)
FROM generate_series(1, 1000000) AS i;

CREATE INDEX idx_orders_1m_status_created ON orders_1m (status, created_at);
CREATE INDEX idx_orders_1m_user ON orders_1m (user_id);
ANALYZE orders_1m;

DROP TABLE IF EXISTS orders_10m;
CREATE TABLE orders_10m (LIKE orders_1m INCLUDING DEFAULTS);

INSERT INTO orders_10m
SELECT o.id + k * 1000000,
       (o.id + k * 1000000) % 10000,
       (ARRAY['paid','pending','shipped','refunded','cancelled'])[1 + ((o.id + k * 1000000) % 5)],
       (((o.id + k * 1000000) % 100000) / 100.0)::numeric(12,2),
       timestamp '2025-01-01 00:00:00' + ((o.id + k * 1000000) % 31536000) * interval '1 second',
       'note-' || ((o.id + k * 1000000) % 997)
FROM orders_1m o CROSS JOIN generate_series(0, 9) AS k;

ALTER TABLE orders_10m ADD PRIMARY KEY (id);
CREATE INDEX idx_orders_10m_status_created ON orders_10m (status, created_at);
CREATE INDEX idx_orders_10m_user ON orders_10m (user_id);
ANALYZE orders_10m;

SELECT 'postgresql' AS engine, (SELECT count(*) FROM orders_1m) AS rows_1m, (SELECT count(*) FROM orders_10m) AS rows_10m;

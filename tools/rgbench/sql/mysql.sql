-- RustGrid docs benchmark data set: MySQL / MariaDB.
-- Table shape is identical on every engine:
--   id (PK), user_id, status, amount(12,2), created_at, note
-- 1,000,000 and 10,000,000 rows, one index on (status, created_at).

CREATE DATABASE IF NOT EXISTS rgbench CHARACTER SET utf8mb4;
USE rgbench;

DROP TABLE IF EXISTS orders_1m;
CREATE TABLE orders_1m (
  id BIGINT NOT NULL,
  user_id INT NOT NULL,
  status VARCHAR(16) NOT NULL,
  amount DECIMAL(12,2) NOT NULL,
  created_at DATETIME NOT NULL,
  note VARCHAR(64) NOT NULL,
  PRIMARY KEY (id)
) ENGINE=InnoDB;

SET SESSION cte_max_recursion_depth = 1000000;
INSERT INTO orders_1m
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000000)
SELECT
  i,
  i % 10000,
  ELT(1 + (i % 5), 'paid', 'pending', 'shipped', 'refunded', 'cancelled'),
  (i % 100000) / 100,
  DATE_ADD('2025-01-01 00:00:00', INTERVAL (i % 31536000) SECOND),
  CONCAT('note-', i % 997)
FROM n;

ALTER TABLE orders_1m ADD INDEX idx_status_created (status, created_at), ADD INDEX idx_user (user_id);
ANALYZE TABLE orders_1m;

DROP TABLE IF EXISTS orders_10m;
CREATE TABLE orders_10m LIKE orders_1m;
ALTER TABLE orders_10m DROP INDEX idx_status_created, DROP INDEX idx_user;

INSERT INTO orders_10m
SELECT
  o.id + k.n * 1000000,
  (o.id + k.n * 1000000) % 10000,
  ELT(1 + ((o.id + k.n * 1000000) % 5), 'paid', 'pending', 'shipped', 'refunded', 'cancelled'),
  ((o.id + k.n * 1000000) % 100000) / 100,
  DATE_ADD('2025-01-01 00:00:00', INTERVAL ((o.id + k.n * 1000000) % 31536000) SECOND),
  CONCAT('note-', (o.id + k.n * 1000000) % 997)
FROM orders_1m o
CROSS JOIN (SELECT 0 AS n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4
            UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) k;

ALTER TABLE orders_10m ADD INDEX idx_status_created (status, created_at), ADD INDEX idx_user (user_id);
ANALYZE TABLE orders_10m;

SELECT 'mysql' AS engine, (SELECT COUNT(*) FROM orders_1m) AS rows_1m, (SELECT COUNT(*) FROM orders_10m) AS rows_10m;

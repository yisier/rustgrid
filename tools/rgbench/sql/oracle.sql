-- RustGrid docs benchmark data set: Oracle (run as TEST/Root123! in FREEPDB1).
SET SERVEROUTPUT ON
SET FEEDBACK OFF
SET TIMING OFF
WHENEVER SQLERROR CONTINUE
BEGIN EXECUTE IMMEDIATE 'DROP TABLE orders_1m'; EXCEPTION WHEN OTHERS THEN NULL; END;
/
BEGIN EXECUTE IMMEDIATE 'DROP TABLE orders_10m'; EXCEPTION WHEN OTHERS THEN NULL; END;
/
WHENEVER SQLERROR EXIT SQL.SQLCODE

CREATE TABLE orders_1m (
  id NUMBER(19) NOT NULL,
  user_id NUMBER(10) NOT NULL,
  status VARCHAR2(16) NOT NULL,
  amount NUMBER(12,2) NOT NULL,
  created_at DATE NOT NULL,
  note VARCHAR2(64) NOT NULL
);

INSERT /*+ APPEND */ INTO orders_1m
SELECT n, MOD(n, 10000),
       CASE MOD(n, 5) WHEN 0 THEN 'paid' WHEN 1 THEN 'pending' WHEN 2 THEN 'shipped'
                      WHEN 3 THEN 'refunded' ELSE 'cancelled' END,
       MOD(n, 100000) / 100,
       DATE '2025-01-01' + MOD(n, 31536000) / 86400,
       'note-' || MOD(n, 997)
FROM (
  SELECT ROWNUM AS n FROM
    (SELECT 1 FROM dual CONNECT BY LEVEL <= 1000) a,
    (SELECT 1 FROM dual CONNECT BY LEVEL <= 1000) b
);
COMMIT;

ALTER TABLE orders_1m ADD CONSTRAINT pk_orders_1m PRIMARY KEY (id);
CREATE INDEX idx_orders_1m_status_created ON orders_1m (status, created_at);
CREATE INDEX idx_orders_1m_user ON orders_1m (user_id);
EXEC DBMS_STATS.GATHER_TABLE_STATS(USER, 'ORDERS_1M');

CREATE TABLE orders_10m AS SELECT * FROM orders_1m WHERE 1 = 0;

INSERT /*+ APPEND */ INTO orders_10m
SELECT o.id + k.n * 1000000,
       MOD(o.id + k.n * 1000000, 10000),
       CASE MOD(o.id + k.n * 1000000, 5) WHEN 0 THEN 'paid' WHEN 1 THEN 'pending' WHEN 2 THEN 'shipped'
                      WHEN 3 THEN 'refunded' ELSE 'cancelled' END,
       MOD(o.id + k.n * 1000000, 100000) / 100,
       DATE '2025-01-01' + MOD(o.id + k.n * 1000000, 31536000) / 86400,
       'note-' || MOD(o.id + k.n * 1000000, 997)
FROM orders_1m o
CROSS JOIN (
  SELECT 0 AS n FROM dual UNION ALL SELECT 1 FROM dual UNION ALL SELECT 2 FROM dual UNION ALL
  SELECT 3 FROM dual UNION ALL SELECT 4 FROM dual UNION ALL SELECT 5 FROM dual UNION ALL
  SELECT 6 FROM dual UNION ALL SELECT 7 FROM dual UNION ALL SELECT 8 FROM dual UNION ALL
  SELECT 9 FROM dual
) k;
COMMIT;

ALTER TABLE orders_10m ADD CONSTRAINT pk_orders_10m PRIMARY KEY (id);
CREATE INDEX idx_orders_10m_status_created ON orders_10m (status, created_at);
CREATE INDEX idx_orders_10m_user ON orders_10m (user_id);
EXEC DBMS_STATS.GATHER_TABLE_STATS(USER, 'ORDERS_10M');

SELECT 'oracle' AS engine, (SELECT COUNT(*) FROM orders_1m) AS rows_1m, (SELECT COUNT(*) FROM orders_10m) AS rows_10m FROM dual;
EXIT

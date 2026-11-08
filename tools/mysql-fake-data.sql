-- ============================================================================
-- RustGrid issue #41 reproduction data.
--
-- One wide table (`orders_1m`) with 1,000,000 rows, built server-side in 10
-- batches of 100,000 so no giant single transaction is needed.
--
--   * MySQL 5.7+ / 8.x, or MariaDB 10.2+ (InnoDB, utf8mb4).
--   * Creates a dedicated database `rustgrid_perf`; nothing outside it is
--     dropped. Re-running the script rebuilds the table from scratch.
--   * The `tag` column deliberately has NO index: filtering on it is the
--     worst case a big table hits (full-scan COUNT(*) + page fetch).
--   * ~34 columns, including VARCHAR / CHAR / TEXT / INT / BIGINT / DECIMAL /
--     FLOAT / DOUBLE / TINYINT / ENUM / JSON / DATE / TIME / DATETIME.
--
-- Usage (mysql CLI, cmd.exe):
--   mysql -u root -p --default-character-set=utf8mb4 < tools\mysql-fake-data.sql
--
-- PowerShell does not accept `<` redirection; use cmd.exe, or pipe:
--   Get-Content -Raw tools\mysql-fake-data.sql | mysql -u root -p
--
-- Or open the file in Navicat / MySQL Workbench / DBeaver and run it.
-- Those clients understand the DELIMITER block around the procedure.
--
-- To load a different row count, change the CALL near the bottom, e.g.
--   CALL fill_orders_1m(500000, 100000);
--
-- Expected disk use: about 0.6-0.9 GB (data + indexes), depending on page
-- size and how much of `remark` is long text.
-- ============================================================================

SET NAMES utf8mb4;
SET SESSION unique_checks = 0;
SET SESSION foreign_key_checks = 0;

-- Optional on a throwaway instance with binary logging: skipping the binlog
-- makes the load much faster. It needs SUPER / SYSTEM_VARIABLES_ADMIN.
-- SET SESSION sql_log_bin = 0;

CREATE DATABASE IF NOT EXISTS rustgrid_perf
  DEFAULT CHARACTER SET utf8mb4
  DEFAULT COLLATE utf8mb4_general_ci;

USE rustgrid_perf;

DROP PROCEDURE IF EXISTS fill_orders_1m;
DROP TABLE IF EXISTS orders_1m;
DROP TABLE IF EXISTS digits;

-- Ten rows (0..9). Cross-joined together five times they produce the 100,000
-- numbers of one batch without a recursive CTE or a permanently huge helper
-- table.
CREATE TABLE digits (n TINYINT UNSIGNED NOT NULL PRIMARY KEY);
INSERT INTO digits (n) VALUES (0),(1),(2),(3),(4),(5),(6),(7),(8),(9);

CREATE TABLE orders_1m (
  id             BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  order_no       VARCHAR(32)     NOT NULL,
  user_id        BIGINT UNSIGNED NOT NULL,
  user_name      VARCHAR(64)     NOT NULL,
  email          VARCHAR(128)    NOT NULL,
  phone          VARCHAR(32)     NOT NULL,
  country        CHAR(2)         NOT NULL,
  province       VARCHAR(32)     NOT NULL,
  city           VARCHAR(32)     NOT NULL,
  address        VARCHAR(255)    NOT NULL,
  store_id       INT             NOT NULL,
  channel        VARCHAR(16)     NOT NULL,
  product_id     INT             NOT NULL,
  product_name   VARCHAR(128)    NOT NULL,
  category       VARCHAR(32)     NOT NULL,
  quantity       INT             NOT NULL,
  unit_price     DECIMAL(12,2)   NOT NULL,
  discount       DECIMAL(5,2)    NOT NULL,
  total_amount   DECIMAL(14,2)   NOT NULL,
  currency       CHAR(3)         NOT NULL,
  status         TINYINT         NOT NULL,
  payment_method ENUM('alipay','wechat','card','paypal','bank') NOT NULL,
  is_vip         TINYINT(1)      NOT NULL,
  tag            VARCHAR(32)     NOT NULL,
  remark         TEXT            NULL,
  detail_json    JSON            NULL,
  avatar_url     VARCHAR(255)    NOT NULL,
  birth_date     DATE            NOT NULL,
  login_time     TIME            NOT NULL,
  score          FLOAT           NOT NULL,
  weight         DOUBLE          NOT NULL,
  created_at     DATETIME        NOT NULL,
  updated_at     DATETIME        NOT NULL,
  paid_at        DATETIME        NULL,
  PRIMARY KEY (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci ROW_FORMAT=DYNAMIC;

DELIMITER $$

CREATE PROCEDURE fill_orders_1m(IN p_total BIGINT, IN p_batch_size BIGINT)
BEGIN
  DECLARE v_batch BIGINT DEFAULT 0;
  DECLARE v_batches BIGINT DEFAULT 0;

  IF p_batch_size IS NULL OR p_batch_size < 1 THEN
    SET p_batch_size = 100000;
  END IF;
  SET v_batches = CEIL(p_total / p_batch_size);

  WHILE v_batch < v_batches DO
    INSERT INTO orders_1m (
      id, order_no, user_id, user_name, email, phone, country, province, city,
      address, store_id, channel, product_id, product_name, category, quantity,
      unit_price, discount, total_amount, currency, status, payment_method,
      is_vip, tag, remark, detail_json, avatar_url, birth_date, login_time,
      score, weight, created_at, updated_at, paid_at
    )
    SELECT
      s.idx,
      s.order_no,
      s.user_id,
      s.user_name,
      s.email,
      s.phone,
      s.country,
      s.province,
      s.city,
      s.address,
      s.store_id,
      s.channel,
      s.product_id,
      s.product_name,
      s.category,
      s.quantity,
      s.unit_price,
      s.discount,
      ROUND(s.quantity * s.unit_price * (100 - s.discount) / 100, 2),
      'CNY',
      s.status,
      s.payment_method,
      s.is_vip,
      s.tag,
      s.remark,
      s.detail_json,
      s.avatar_url,
      s.birth_date,
      s.login_time,
      s.score,
      s.weight,
      s.created_at,
      DATE_ADD(s.created_at, INTERVAL (s.idx % 72) HOUR),
      CASE WHEN s.status >= 2
           THEN DATE_ADD(s.created_at, INTERVAL (s.idx % 48) HOUR)
           ELSE NULL
      END
    FROM (
      SELECT
        x.idx,
        CONCAT('ORD', LPAD(x.idx, 10, '0')) AS order_no,
        (x.idx * 7919) % 500000 AS user_id,
        CONCAT(
          ELT(1 + x.idx % 10, '张','王','李','赵','刘','陈','杨','黄','周','吴'),
          ELT(1 + (x.idx DIV 10) % 10, '伟','芳','娜','敏','静','强','磊','洋','艳','勇'),
          x.idx % 1000
        ) AS user_name,
        CONCAT(
          ELT(1 + x.idx % 5, 'user','test','demo','mail','buyer'),
          x.idx,
          '@',
          ELT(1 + x.idx % 4, 'example.com','gmail.com','qq.com','163.com')
        ) AS email,
        CONCAT(
          '1',
          ELT(1 + x.idx % 5, '38','39','58','86','88'),
          LPAD((x.idx * 7919) % 100000000, 8, '0')
        ) AS phone,
        'CN' AS country,
        ELT(1 + (x.idx DIV 1000) % 10,
            '广东','江苏','浙江','山东','河南','四川','湖北','湖南','福建','安徽') AS province,
        ELT(1 + (x.idx DIV 100) % 10,
            '广州','深圳','杭州','南京','成都','武汉','长沙','合肥','福州','济南') AS city,
        CONCAT(
          '科技园',
          ELT(1 + x.idx % 20,
              '一','二','三','四','五','六','七','八','九','十',
              '十一','十二','十三','十四','十五','十六','十七','十八','十九','二十'),
          '路 ', 1 + x.idx % 200, ' 号 ',
          1 + x.idx % 30, ' 栋 ',
          1 + x.idx % 20, ' 层'
        ) AS address,
        x.idx % 500 AS store_id,
        ELT(1 + x.idx % 4, 'web','app','mini','offline') AS channel,
        1000 + x.idx % 2000 AS product_id,
        ELT(1 + x.idx % 12,
            '机械键盘','27寸显示器','人体工学椅','胶囊咖啡机','降噪耳机','无线鼠标',
            '轻薄笔记本','高清摄像头','千兆路由器','移动硬盘','电容麦克风','护眼台灯') AS product_name,
        ELT(1 + x.idx % 6, '外设','数码','家居','家电','音频','存储') AS category,
        1 + x.idx % 9 AS quantity,
        ROUND(5 + (x.idx * 37 % 50000) / 100, 2) AS unit_price,
        ELT(1 + x.idx % 6, 0, 0, 5, 10, 15, 20) AS discount,
        x.idx % 5 AS status,
        ELT(1 + x.idx % 5, 'alipay','wechat','card','paypal','bank') AS payment_method,
        IF(x.idx % 7 = 0, 1, 0) AS is_vip,
        CONCAT('tag_', LPAD(x.idx % 50, 2, '0')) AS tag,
        CASE WHEN x.idx % 13 = 0
             THEN CONCAT(
               REPEAT('长文本备注：用于测试表格里的长文本显示与编辑。', 10),
               ' #', x.idx
             )
             ELSE CONCAT('普通备注 ', x.idx)
        END AS remark,
        CONCAT(
          '{"source":"', ELT(1 + x.idx % 3, 'web','app','api'),
          '","level":', x.idx % 5,
          ',"step":', x.idx,
          ',"tags":["a","b","c"]}'
        ) AS detail_json,
        CONCAT('https://cdn.example.com/avatar/', LPAD(x.idx % 1000, 4, '0'), '.png') AS avatar_url,
        DATE_ADD('1970-01-01', INTERVAL (x.idx * 37 % 15000) DAY) AS birth_date,
        SEC_TO_TIME((x.idx * 13) % 86400) AS login_time,
        ROUND((x.idx % 1000) / 10, 1) AS score,
        ROUND(40 + (x.idx * 17 % 5000) / 100, 2) AS weight,
        DATE_ADD(
          DATE_ADD('2023-01-01 00:00:00', INTERVAL (x.idx * 7919 % 1400) DAY),
          INTERVAL (x.idx * 31 % 86400) SECOND
        ) AS created_at
      FROM (
        SELECT
          v_batch * p_batch_size + b.n + 1 AS idx
        FROM (
          SELECT
            d0.n + d1.n * 10 + d2.n * 100 + d3.n * 1000 + d4.n * 10000 AS n
          FROM digits d0
          CROSS JOIN digits d1
          CROSS JOIN digits d2
          CROSS JOIN digits d3
          CROSS JOIN digits d4
        ) AS b
      ) AS x
    ) AS s
    WHERE s.idx > v_batch * p_batch_size
      AND s.idx <= LEAST((v_batch + 1) * p_batch_size, p_total);

    SET v_batch = v_batch + 1;
    COMMIT;
  END WHILE;
END$$

DELIMITER ;

CALL fill_orders_1m(1000000, 100000);

DROP PROCEDURE IF EXISTS fill_orders_1m;

-- Secondary indexes are built after the load: an empty table with only the
-- clustered index inserts far faster. `tag` is left unindexed on purpose.
ALTER TABLE orders_1m
  ADD UNIQUE KEY uk_order_no (order_no),
  ADD KEY idx_email (email),
  ADD KEY idx_user_id (user_id),
  ADD KEY idx_status_created (status, created_at),
  ADD KEY idx_created_at (created_at),
  ADD KEY idx_category (category),
  ADD KEY idx_store_id (store_id);

ANALYZE TABLE orders_1m;

-- ---------------------------------------------------------------------------
-- Verification.
-- ---------------------------------------------------------------------------
SELECT VERSION() AS server_version;

SELECT COUNT(*) AS rows_loaded FROM orders_1m;

SELECT table_rows AS approx_rows,
       ROUND(data_length / 1024 / 1024, 1) AS data_mb,
       ROUND(index_length / 1024 / 1024, 1) AS index_mb
FROM information_schema.tables
WHERE table_schema = 'rustgrid_perf' AND table_name = 'orders_1m';

SELECT id, user_name, email, city, product_name, quantity, unit_price,
       total_amount, status, tag, created_at
FROM orders_1m
ORDER BY id
LIMIT 20;

-- ---------------------------------------------------------------------------
-- The queries RustGrid sends when a filter is applied. `tag` has no index, so
-- the COUNT is a 1,000,000-row scan and the page fetch is what a big-table
-- filter costs. Each tag value matches about 20,000 rows.
-- ---------------------------------------------------------------------------
SELECT COUNT(*) AS tag_07_matches FROM orders_1m WHERE tag = 'tag_07';

SELECT * FROM orders_1m
WHERE tag = 'tag_07'
ORDER BY id
LIMIT 1000 OFFSET 0;

-- An indexed filter for comparison.
SELECT COUNT(*) AS user_matches FROM orders_1m WHERE user_id = 12345;

SELECT * FROM orders_1m
WHERE user_id = 12345
ORDER BY id
LIMIT 1000 OFFSET 0;

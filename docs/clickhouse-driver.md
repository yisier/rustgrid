# ClickHouse driver (`rustgrid-clickhouse`)

ClickHouse is RustGrid's **OLAP** engine. It is a normal network driver: the connection tree shows
databases and their tables/views, the data grid pages a table with the WHERE filter and sorting, the
export wizard streams it, and the query editor runs SQL. Two engine properties shape the driver:

- There is **no unique row identity**. MergeTree tables have a sorting key, not a primary key, and
  `ALTER TABLE … UPDATE/DELETE` is an asynchronous mutation that can touch several rows.
- **`OFFSET` is linear**: a page far into a large table reads everything before it.

## Stack

- The official Rust client **`clickhouse` 0.15** (`default-features = false`, `rustls-tls`). It speaks
  ClickHouse's **HTTP interface** (port 8123, or 8443 with TLS) over hyper 1. There is no native
  (9000) client in the Rust ecosystem, and ClickHouse Cloud only exposes HTTPS, so HTTP covers both.
- TLS reuses the workspace's rustls (no OpenSSL, no C toolchain). The driver builds its own hyper
  client so the standard TLS page works: a CA verifies the server, a client certificate/key performs
  mutual TLS, `需要` encrypts without verifying, and `首选` tries HTTPS then falls back to HTTP.
  (`VerifyCa` also verifies the host name, because rustls has no separate `VerifyCa` verification.)
- SSH / SOCKS5 / HTTP tunnels reuse `rustgrid-tunnel`: the client connects to the local forwarder.

## Connection form

| Field | Option key | Page |
| --- | --- | --- |
| Default database | standard database field | 常规 |
| Connect timeout / query timeout / keep-alive / read-only | standard advanced fields | 高级 |

The database is optional: a blank field means ClickHouse's `default` database. There are no
engine-specific options. `read_only` is enforced by the server (`readonly = 2` on every request), so
the query editor cannot write through a read-only connection either. ClickHouse's HTTP interface has
no session for a one-off init statement, so 连接初始化 SQL is not offered.

A wrong password (or an empty one where the server requires a password) maps to
`Error::Authentication`, so the app opens its password prompt like it does for the SQL engines.

## Data model

- `list_databases` hides `system`, `INFORMATION_SCHEMA` and `_temporary_and_external_tables`; the
  system catalogue is still queryable from the SQL editor.
- `list_tables` reads `system.tables` (`is_temporary = 0`, internal `.inner*` tables hidden). Engine
  `View` / `MaterializedView` / `LiveView` / `WindowView` become the app's **views**.
- `columns` reads `system.columns`: the full ClickHouse type (`Nullable(String)`,
  `Decimal(20, 6)`, `Array(UInt8)`), nullable, comment, and the MATERIALIZED/ALIAS/EPHEMERAL flag as
  *generated*. There is no auto-increment. Column default expressions are shown by the table's DDL
  (`SHOW CREATE TABLE`/`system.tables.create_table_query`) in the info pane; `ColumnInfo` has no
  default field yet.
- The table overview columns come from `system.tables`: `total_rows` (approximate), `total_bytes`,
  `metadata_modification_time`, `engine`, `comment`, and the `PARTITION BY`/`ORDER BY` keys as the
  create options.

### Result decoding

Every result-producing statement is asked for
`FORMAT JSONCompactEachRowWithNamesAndTypes`: one JSON array per row, preceded by the column names
and their ClickHouse types. A single request therefore reports both the schema and the rows.

| ClickHouse type | Grid cell |
| --- | --- |
| `Int8` … `Int64` / `UInt8` … `UInt64` | number |
| `Int128`/`Int256`/`UInt128`/`UInt256` | exact decimal text (wider than the cell model) |
| `Float32`/`Float64` | number |
| `Decimal*` | exact text (`output_format_json_quote_decimals=1`) |
| `Bool` | boolean |
| `Date`/`Date32`/`DateTime`/`DateTime64` | text |
| `String`/`FixedString`/`UUID`/`IPv4`/`IPv6`/`Enum*` | text |
| `Array`/`Tuple`/`Map`/`Nested`/`JSON`/`Variant`/`Dynamic` | compact JSON text |
| `Nullable(T)` / `LowCardinality(T)` | `T` (a `NULL` value is the grid's `NULL`) |

Wide integers and decimals stay quoted through the whole trip, so no value is rounded through
`f64`.

## The grid is read-only

The driver advertises `DriverCapability::ReadOnlyGrid`, so a ClickHouse table page disables
编辑/新增/复制/删除, and a query result stays read-only even when sqlparser infers a single table.
`update_rows` / `delete_rows` refuse with a clear message if reached anyway.

Reasoning: a row update/delete needs a `WHERE` that identifies exactly one row. ClickHouse has no
unique constraint; `system.tables.primary_key` is the `ORDER BY` sorting-key *expression*, which may
match many rows, and the mutation that implements it is asynchronous (`SYSTEM MUTATIONS` shows its
progress). The SQL editor is the right place for `ALTER TABLE … UPDATE/DELETE` (or lightweight
`DELETE FROM`, 23.3+).

`insert_rows` **is** implemented (batched `INSERT … VALUES`, `DEFAULT` for omitted columns), so the
Import Wizard can load an existing table and create a new one with `ENGINE = MergeTree ORDER BY
tuple()`. Import type inference produces MySQL-shaped types (`bigint`, `varchar`, `decimal(20,6)`…);
the driver maps them to `Int64`, `String`, `Decimal(20, 6)`… in `create_table_sql`. A column whose
value is a complex ClickHouse type (`Array`, `Map`, …) must be written as SQL: the wizard's text
values are quoted literals, which ClickHouse does not parse into those types.

The export wizard works normally (it reads pages through `fetch_page`), as does a grid's 导出当前
记录.

## Query tab

- A run is split into statements by the shared splitter, and each statement goes through
  `execute_query` **without** the client's `?` binding (`query_raw`), so a statement's own `?` is
  left alone.
- Result-producing statements (`SELECT`, `WITH`, `SHOW`, `DESCRIBE`, `EXPLAIN`, `EXISTS`) get the
  row format appended unless they already end with their own `FORMAT`.
- The query editor's script session pins a ClickHouse `session_id`, so `SET` survives between the
  statements of one run. `USE db` is handled by the session itself (the following requests name that
  database), because every HTTP request names its database and would otherwise override the
  server-side session's current database.
- A statement that returns no rows reports "no result" like the other drivers; ClickHouse does not
  report a row count for DDL/INSERT, so there is no affected-row number.

## Capabilities and scope

`DriverCapability::DatabaseManagement` + `DriverCapability::ReadOnlyGrid`:

- 新建数据库 / 删除数据库 work (`CREATE/DROP DATABASE`); 编辑数据库 has nothing to edit (ClickHouse
  databases have no charset, collation or owner), so the dialog shows just the name.
- Drop / rename / truncate go through `Connection` (`DROP TABLE`, `RENAME TABLE`, `TRUNCATE TABLE`);
  清空表 and 截断表 both truncate (ClickHouse has no `DELETE FROM` without a mutation).
- No table designer (`supports_table_designer = false`): MergeTree needs its engine, `PARTITION BY`,
  `ORDER BY`, `TTL`, sampling and settings modelled, which is a later phase.
- No `.rgbak` backup (`supports_backup = false`).
- No users/roles, routines or events (`system.users`/`system.grants` are queryable from the SQL
  editor; ClickHouse has no stored procedures).
- Views are listed and open read-only; the view designer is not offered (view management is a later
  phase).

## Limitations

- Object names are plain `database.table`. ClickHouse has no schemas (`system` is a database), so the
  tree keeps the flat database → category shape.
- A page's row order is only as stable as the requested `ORDER BY` (or the engine's block order):
  without a unique key, duplicated sort values may move between pages of a changing table.
- `total_rows` in the object overview is ClickHouse's own estimate; the grid's record count uses
  `count()`, which is exact but scans when a filter is applied.

## Live test

```bash
RUSTGRID_CLICKHOUSE_PASSWORD=clickhouse cargo test -p rustgrid-clickhouse -- --ignored
```

Optional: `RUSTGRID_CLICKHOUSE_HOST`/`PORT`/`USER`/`DATABASE`. The test creates and drops its own
`rustgrid_live` database and exercises connect, catalog, columns, paging with a filter and a sort,
`count()`, arbitrary SQL, multi-statement runs, DDL, rename/truncate/drop and the read-only refusals.

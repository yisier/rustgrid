# rgbench

The benchmark harness behind the performance report on the documentation site
(<https://yisier.github.io/rustgrid/#benchmark>).

It runs the operations a RustGrid grid performs, through RustGrid's own driver crates:
first page (`fetch_page`, 1,000 rows + `COUNT(*)`), deep pages, a filtered + sorted page,
`count_rows`, and primary-key lookups. Each operation runs once as a warm-up and then three
times; the median is written to JSON.

## Data set

One shape on every engine:

```sql
orders (id bigint primary key, user_id int, status varchar(16),
        amount decimal(12,2), created_at timestamp, note varchar(64))
-- index (status, created_at), index (user_id)
```

`orders_1m` holds 1,000,000 rows, `orders_10m` holds 10,000,000. Values are derived from the
row id (`status` cycles five values, `amount` cycles 0-999.99, `created_at` spreads over 2025),
so every engine gets identical data.

Generators:

| Engine | File |
| --- | --- |
| MySQL / MariaDB | `sql/mysql.sql` |
| PostgreSQL | `sql/postgres.sql` |
| ClickHouse | `sql/clickhouse.sql` |
| Oracle | `sql/oracle.sql` |
| MongoDB | `sql/mongo.js` |
| SQLite | `sql/sqlite.mjs` |

Run one against the local Docker lab:

```bash
# WSL / Linux shell
tools/rgbench/generate.sh mysql      # postgres | clickhouse | oracle | mongo
node tools/rgbench/sql/sqlite.mjs    # writes target/rgbench/rgbench.db
```

The MySQL/PostgreSQL scripts recreate the `rgbench` database. Oracle writes `orders_1m` /
`orders_10m` into the `TEST` schema; MongoDB recreates the `rgbench` collections.

## Running

```bash
cp tools/rgbench/config.example.json tools/rgbench/config.local.json
# fill in the passwords (config.local.json is gitignored)

cargo build --profile harness -p rgbench
target/harness/rgbench --config tools/rgbench/config.local.json --out site/.vuepress/data/bench.json
```

`--only mysql,postgresql` narrows the run. The `harness` profile is an optimized release build
without LTO, so timings reflect optimized code without a long rebuild.

The report is only indicative: every database runs in Docker on one machine, one at a time,
so absolute numbers depend on the host, the Docker volume and the engine's warm-up state.

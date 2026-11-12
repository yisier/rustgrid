# Real UDAL / TeleDB DBProxy lab

A reproducible **real** UDAL front end for the MySQL driver tests. It runs the open-source
[`TeleDB-DBProxy`](https://gitee.com/teledb/teledb-dbproxy) — the DBProxy at the heart of
天翼云 DRDS / TeleDB (UDAL) — in front of a MariaDB backend. It is not an emulator: the
`1046`, plain-OK metadata and binary-row quirks below are produced by the actual Java proxy.

`tools/udal-proxy/proxy.js` remains the fast, scriptable Node fallback for the reactive
`1047`/`3013`/`1046` paths. This lab is for the **proactive** behaviour that a hand-written
emulator kept missing.

## Run

```sh
cd tools/udal-lab
docker compose up -d --build
```

The first build clones the pinned Gitee commit and runs Maven inside a JDK 8 container
(roughly one to three minutes with the Aliyun mirror). Once `udal-dbproxy` is up:

```sh
docker compose exec udal-mariadb mariadb -uudal -pudal123 -e "SELECT * FROM UDAL_TEST.users;"
mysql -h127.0.0.1 -P8066 -uroot -p123456 -DTEST -e "SHOW FULL TABLES; SELECT * FROM users;"
```

The logical schema is **`TEST`**; UDAL maps it to the physical database **`UDAL_TEST`**.
`SHOW FULL TABLES` therefore reports `Tables_in_UDAL_TEST`, and an unqualified `users`
addresses the logical table.

Run the live tests:

```sh
RUSTGRID_UDAL_PASSWORD=123456 RUSTGRID_UDAL_PORT=8066 \
  cargo test -p rustgrid-mysql --test live_udal -- --ignored
```

Optional environment: `RUSTGRID_UDAL_HOST` (default `127.0.0.1`) and
`RUSTGRID_UDAL_USER` (default `root`).

## What the real proxy actually does

These are the behaviours the driver is written against (see `AGENTS.md`, "A MySQL-protocol
proxy may reject server-side prepares"):

- **No database, no statement.** A session without a current database answers
  `1046 - UDAL - No database selected` to almost everything. A narrow whitelist still works:
  `SELECT VERSION()`, `SELECT DATABASE()`, `SELECT 1`, `SELECT NOW()`, `SHOW DATABASES`,
  `SHOW VARIABLES`, `SHOW WARNINGS`, `SET NAMES`. An alias (`SELECT 1 AS one`), an
  `@@session.*` variable, `SHOW FULL TABLES` and even a fully qualified table are refused.
- **`COM_STMT_PREPARE` is sticky.** The first prepare sets a session-wide `prepared` flag
  (`ServerPrepareHandler` → `NonBlockingSession.setPrepared(true)`) that is never cleared.
  Every later **text** query is then encoded as a **binary row** (`0x00` header + NULL bitmap
  before the text values). A text result-set reader fails with
  `buffer exhausted ... decoded length is 144`. Navicat avoids this by never preparing.
  The driver recognises UDAL from the greeting (`5.7.21-UDAL-DBPROXY-...`) in
  `after_connect` and switches the whole pool to the text protocol before the first statement.
- **A qualified name bypasses the logical mapping.** `SELECT * FROM TEST.users` reads the
  backend database `TEST` (if it exists), while `SELECT * FROM users` with the session on
  `TEST` reads the mapped `UDAL_TEST`. Generated SQL therefore uses unqualified names once
  `needs_database` is set (`MysqlConnection::table_ref`), exactly like Navicat.
- **`INFORMATION_SCHEMA` filtered by the logical name reads the physical database** (or
  nothing when the shards are named differently). The table designer reads columns/indexes/
  options with `SHOW FULL COLUMNS` / `SHOW INDEX` / `SHOW TABLE STATUS`
  (`table_schema_via_show`).
- **DDL is disabled in the open-source build.** `CREATE` / `DROP` answer
  `ERROR 1141 ... UDAL - CREATE command denied` (`PrivilegePoints.forbid()`), so the live tests
  use the pre-created `users` table. INSERT/UPDATE/DELETE work through the text protocol.

## Testing DDL (lab-only)

The open-source DBProxy hard-codes `CREATE` / `DROP` / `ALTER` as forbidden
(`PrivilegePoints.forbid()`), so the default lab answers
`1141 - UDAL - CREATE command denied` no matter what the account was granted. To exercise the
app's table designer against the real proxy, build the lab with the opt-in patch:

```sh
UDAL_ALLOW_DDL=1 docker compose up -d --build
```

This only changes the local image (it makes `forbid()` return false); it is **not** what the
commercial UDAL necessarily does. The MariaDB 11.4 backend does not know MySQL 8's
`utf8mb4_0900_ai_ci` collation, so use `utf8mb4_uca1400_ai_ci` (or omit `COLLATE`) in DDL
generated for this lab.

## Files

| File | Purpose |
|---|---|
| `Dockerfile` | Builds the proxy from the pinned Gitee commit and runs it with JDK 8. |
| `config.json` | File-mode configuration (`-Dddl.config.type=file`): one logical `TEST` schema, one MariaDB data node, one `root` account. |
| `myid.properties` | ZooKeeper address used by the proxy's lock/registry services. |
| `cacheservice.properties` | The cache pool the proxy always initialises. |
| `init.sql` | Creates `UDAL_TEST` and the `users` fixture table. |
| `docker-compose.yml` | MariaDB 11.4 + ZooKeeper 3.9 + the proxy. |

## Limitations

- The lab has no TLS; it exercises `mysql_native_password`. The auth-failure test runs
  directly against the MariaDB/MySQL port when needed.
- The open-source DBProxy is `2.7.5`; a commercial UDAL may enable DDL and differ in its
  management commands. The protocol behaviour above is the part the driver depends on.
- The proxy source is fetched at build time, not vendored. If the Gitee repository moves,
  update the commit in the `Dockerfile`.

# AGENTS.md

A Navicat-like database management tool built with **Rust + gpui-kit**, targeting
**cross-platform** (Windows, macOS, Linux). **Extensibility is a first-class requirement**,
not a later refactor.

## Repository layout (Cargo workspace)

- Root files: `AGENTS.md`, `README.md` / `README_zh.md`, and the changelog pair `CHANGELOG.md` /
  `CHANGELOG_zh.md` — **every version change is recorded in both changelogs** (see Commands).
- `crates/rustgrid-core` — engine-agnostic domain: `Driver`/`Connection` traits, models
  (`CellValue`, `ConnectionProfile`, `TablePage`, ...), `Error`, `DriverRegistry`.
  **No sqlx / GPUI / OS dependencies here.**
- `crates/rustgrid-backup` — RustGrid's own `.rgbak` backup container (zstd chunks + JSON
  manifest), used by the Backup main tab.
- `crates/rustgrid-export` — table-data export writers: `.xlsx` (rust_xlsxwriter), `.csv`, `.sql`
  and `.txt`. Engine-agnostic formatting fed by decoded `CellValue` rows.
- `crates/rustgrid-import` — source-file readers for the Import Wizard: Excel workbooks
  (`.xlsx`/`.xls`/`.xlsm`/`.xlsb`/`.ods`) via `calamine`, and `.csv`/`.txt` with an RFC 4180
  parser (`encoding_rs` GB18030 fallback; TXT auto-detects its field delimiter among
  tab/comma/semicolon/pipe), plus the source-column type inference used when the wizard creates a
  table.
- `crates/rustgrid-mysql` — the MySQL-protocol drivers; implements the core traits with sqlx.
  It serves both **MySQL** and **MariaDB** (`MysqlDriver` / `MariaDbDriver`) because they share the
  wire protocol and almost all catalog/DDL syntax; `engine.rs`'s `Engine` carries the few dialect
  differences (driver id, session-variable names).
- `crates/rustgrid-sqlite` — the file-based **SQLite** driver. One database is one file: the
  profile's `database` holds the path and there is no server/user/password (see `Driver::is_file_based`).
  Browsing, paging, editing, arbitrary SQL, table DDL and backup/restore are implemented; SQLite has
  no stored routines or events, so those trait methods return empty.
- `crates/rustgrid-sqlserver` — the **Microsoft SQL Server** driver, built on **tiberius** (pure-Rust
  async TDS) with a **bb8** connection pool (`pool.rs`). It is not sqlx-based (sqlx 0.9 has no MSSQL
  driver). `driver.rs` builds the tiberius `Config` (host/port, SQL authentication, TLS, timeouts) and
  maps login failures to `Error::Authentication`; `connection.rs` implements `Connection` with
  `[bracketed]` identifiers, `@P1` parameters and `OFFSET … FETCH NEXT` paging (always with an
  `ORDER BY`); `helpers.rs` holds the dialect helpers (quoting, script splitting aware of
  `BEGIN`/`CASE … END`, filter translation, `CellValue` decoding); `user.rs` maps SQL Server logins
  onto the account model — server privileges, server roles and per-database 用户映射 (each login's
  database user, default schema and database-role memberships) — while database/object privileges
  are managed through the 对象权限管理器. Statements use the
  text/batch path for DDL and `CREATE PROCEDURE`, and the parameterized RPC path for `SELECT`/DML so
  row counts are available. Its connection form declares SQL Server's own fields (验证 →
  `sqlserver.authentication` = SQL Server / Windows, `sqlserver.application_name`) through the shared
  engine-field mechanism; Windows authentication is
  compiled in on Windows only (`winauth` under `[target.'cfg(windows)'.dependencies]`; tiberius' Unix
  `sspi-rs` backend is **not** enabled because it conflicts with the workspace's `russh`/`crypto-bigint`,
  so Unix offers SQL Server authentication only), while Navicat's Active Directory modes need an
  MSAL/OAuth flow tiberius does not provide.
- `crates/rustgrid-postgresql` — the **PostgreSQL** driver, built on **sqlx 0.9** with its
  `postgres` feature (sharing the workspace's `tls-rustls-ring`). PostgreSQL cannot query across
  databases, so `PostgresConnection` keeps a **lazily-created `PgPool` per database** (`pools`,
  default database kept warm, others capped at 2 connections with a short idle timeout and an LRU
  cap) and routes every operation through `pool(database)`. Object names are **schema-qualified**
  (`public.users`), matching SQL Server, so the tree nests databases → schema → tables/views/…;
  system schemas (`pg_*`, `information_schema`) are hidden. Read paths project every column to text
  (`"c"::text`) because the binary protocol path (taken whenever a filter is bound) cannot decode
  `numeric`/`uuid`/`json`/arrays from raw bytes; writes cast every bound parameter to the column's
  `format_type` (`CAST($1 AS integer)`) because PostgreSQL is strongly typed. `helpers.rs` holds the
  dialect helpers (double-quote identifiers, `$n` placeholders, dollar-quote-aware script splitting,
  filter translation with casts, text decoding, literal rendering); `schema.rs` assembles
  `CREATE/ALTER TABLE` from `pg_catalog` (there is no `SHOW CREATE`); `routine.rs` encodes a routine's
  schema and identity arguments into its name (`public.get_user(integer)`); `view.rs`, `user.rs` and
  `backup.rs` cover views, roles/ACLs and `.rgbak`. Multi-statement scripts run on **one pinned
  connection** through `raw_sql(...).fetch_many`, letting the server split dollar-quoted function
  bodies. See `docs/postgresql-driver.md`.
- `crates/rustgrid-oracle` — the **Oracle** driver, built on Oracle's official pure-Rust thin driver
  **`oracledb`** (26.0.0-beta.4, pinned exactly; **no OCI / Instant Client**). Oracle is a
  **schema-first** engine like SQL Server/PostgreSQL (one connection sees every schema of its
  service/PDB), so a single `Arc<oracledb::Pool>` serves every operation. `oracledb` is a
  **synchronous** API, so every trait method runs inside `tokio::task::spawn_blocking` and checks a
  connection out of the pool (`connection.rs::with_conn`); `Pool` is not `Clone`, hence the `Arc`.
  `helpers.rs` holds the dialect helpers (double-quote identifiers, `:n` placeholders **without
  lower-casing**, `TO_DATE`/`TO_TIMESTAMP` binds, `OFFSET … FETCH` paging, filter translation, the
  **client-side script splitter** that keeps PL/SQL blocks and `q'[...]'` quotes whole and strips
  trailing semicolons, plus `CellValue` decoding); `columns`/`fetch_page` read the `ALL_*` catalogs,
  `schema.rs` assembles `CREATE/ALTER TABLE` (Oracle has no `SHOW CREATE`; the designer reads
  `ALL_TAB_COLUMNS`/`ALL_CONSTRAINTS`), `routine.rs` reads functions/procedures/packages/triggers
  from `ALL_OBJECTS` and `DBMS_METADATA.GET_DDL`, and `view.rs`/`user.rs`/`backup.rs` cover views,
  users/roles and `.rgbak`. See `docs/oracle-driver.md`.
- `crates/rustgrid-redis` — the **Redis** driver, built on **`redis` 0.32** (redis-rs: async,
  cluster, sentinel and rustls TLS). Redis has no tables, so the driver maps a numbered database to
  the object container and a **key** to a "table": a hash pages as field/value, a list as index/value,
  a set as member, a zset as member/score, a stream as entry id/field/value, a string/ReJSON key as a
  single value. `TYPE` drives the column set; every edit goes back through the key's own commands
  (`SET`, `HSET`, `LSET`, `SADD`, `ZADD`, `HDEL`, `SREM`, `ZREM`, `UNLINK`, `RENAME`). Key listing is
  `SCAN`-based and capped by the connection's 键列表上限 (`redis.key_limit`, default 1000); in cluster
  mode every master from `CLUSTER SLOTS` is scanned, because `SCAN` only sees one shard. Each
  database gets its own multiplexed connection (`SELECT` is connection state), and the driver gates
  commands on the server version read from `INFO server`: `MEMORY USAGE`/`UNLINK`/`PTTL` need 4.0,
  streams 5.0, `SET … KEEPTTL` and ACL 6.0 (on older servers a string edit saves and restores the
  PTTL around the write). The query editor runs one command per line through the shared editor, and
  refuses what would break a shared connection (`SELECT`/`AUTH`, `MULTI`, `CLIENT REPLY`, blocking
  commands, `SHUTDOWN`). See `docs/redis-driver.md`.
- `crates/rustgrid-mongodb` — the **MongoDB** driver, built on the official **`mongodb` 3.10** Rust
  driver (`bson-3`, rustls TLS; DNS/SRV, compression and the KMS integrations are off). A
  connection is a **document store** (`DriverCapability::Documents`): databases hold collections, a
  collection is shown as a table whose columns are the union of the top-level fields of a 50-document
  sample, and one document is one grid row. `_id` is the primary key, so grid edits become `$set`,
  insert/delete are keyed by it and a `_id` edit is ignored (immutable). Scalars keep their
  `CellValue` shape where possible; ObjectId/decimal/date/binary and embedded documents/arrays are
  text/Extended JSON and are parsed back through the column's BSON type (`value.rs`), so the
  existing row form shows embedded documents in its JSON editor. The filter tree is translated to a
  query document (`filter.rs`), sorting/paging use `skip`/`limit` with an `_id` tie-breaker. The
  query tab runs **one JSON command** (`find`, `aggregate`, `count`, `distinct`, `listCollections`,
  `listDatabases`, otherwise `runCommand`) instead of JavaScript, and its grids are read-only
  because no SQL table can be inferred. TLS, SSH/SOCKS5/HTTP tunnels, SCRAM/None authentication and
  the auth database come from the standard connection form; there is no table designer, `.rgbak`
  backup, users/roles or Views category. See `docs/mongodb-driver.md`.
- `crates/rustgrid-tunnel` — the shared **SSH / SOCKS5 / HTTP** tunnel (local port forwarding) used
  by the network drivers (`rustgrid-mysql`, `rustgrid-postgresql`, `rustgrid-oracle`,
  `rustgrid-sqlserver`, `rustgrid-redis` in standalone mode, `rustgrid-mongodb`). Migrated out of
  `rustgrid-mysql` so PostgreSQL reuses it; `rustgrid-mysql` re-exports `Tunnel`.
- `crates/rustgrid-odbc` — the **generic ODBC driver** (app feature `driver-odbc`, on by default).
  It connects through a user-installed ODBC driver or a raw connection string and speaks the engine's
  SQL, as the fallback for engines without a native Rust driver (DB2, 达梦, ...). The ODBC
  driver manager (`odbc32.dll` / `libodbc.so`) is loaded **at runtime** via `libloading` (`api.rs`),
  so the build carries no link-time dependency and a machine without ODBC still runs normally;
  `driver.rs`/`connection.rs` implement the trait on top of that. It is synchronous and implements
  catalog listing, `execute_query` and paging; other `Connection` methods report "not supported yet".
  See `docs/odbc-driver.md`.
- `crates/rustgrid-config` — versioned settings/profiles plus encrypted secret storage in the
  OS config dir (`connections.json`, `settings.json`, `secrets.json`). Its `transfer` module owns
  connection import/export: the RustGrid `.rgc` bundle, the Navicat `.ncx` reader (XML plus the
  `libcckeylibcckey` AES-128-CBC password scheme DBX also reads) and the DBX plain-JSON reader,
  all mapped onto a common `TransferConnection`.
- `crates/rustgrid-app` — GPUI binary `RustGrid`: `src/app/` is the view/render layer, split
  by feature (`mod.rs` holds `AppView`, its state, the `Render` entry, free helpers and
  tests; the rest are `impl AppView` submodules: `tree`, `database`, `db_dialog`, `objects`,
  `sidebar`, `tabs`, `toolbar`, `query`, `query_view`, `query_editor`, `grid`, `grid_input`,
  `grid_commit`, `grid_view`, `grid_cell`, `grid_scroll`, `grid_toolbar`, `dialogs`,
  `export`, `import`, `routine`, `routine_view`, `view`, `view_view`, `widgets`, `form`). New view code goes in the matching submodule, **not** `mod.rs`.
  `ui/` is the internal design system (buttons, scrollbars, text fields, dropdowns, ...) and the
  wrapper layer over gpui-kit: put shared chrome there, never one-off `div`s in feature code. Several subtrees are child
  `Entity` views wired through `WeakEntity<AppView>` + `notify_*` invalidation: the Tables/Views
  object list (`ObjectPane`), the tab strip (`TabBar`), the connection tree (`TreePane`), and
  each open grid (`GridView`, one entity per grid, owning its `GridState` and all grid
  interaction state). Follow that pattern when a subtree gets large. Dialogs are **not** drawn by
  `AppView`: it holds the dialog state (`form`, `db_dialog`, `password_prompt`, `error_dialog`,
  `delete_confirm`, `options_open`) and `sync_dialog` hands it to `Root` via
  `window.open_dialog` — see "UI conventions".
  Other files: `src/session.rs` (UI state), `src/form.rs` (connection form),
  `src/theme.rs` (light/dark palettes), `src/assets.rs` (embedded asset loader for
  `assets/`), `src/runtime.rs` (tokio bridge), locales in `locales/`.
- The root `Cargo.toml` owns all versions under `[workspace.dependencies]`; member crates use
  `<dep>.workspace = true`. Add new dependencies there, not inline in a member.

## Tech stack (decided)

- Rust **stable**, **edition 2024** (`rust-toolchain.toml` pins `stable`; workspace
  `rust-version = "1.94"`).
- UI: **`gpui-kit = "0.8"`** (crates.io). It is the facade over a published GPUI snapshot plus
  `gpui-base` and the shadcn-styled `gpui-component`, and it re-exports the engine itself. The
  workspace enables its **`gpui-fast`** feature, so the engine is **`gpui-fast = "0.1.12"`** —
  Longbridge's rendering/layout-optimised fork of `gpui-pre` — and the alias the ~40 view files
  import is `gpui = { package = "gpui-fast", version = "0.1.12" }`. **The two lines must move in
  lockstep**: the kit re-exports whichever engine the feature selects, so a mismatch is two GPUI
  cores with incompatible element types. `gpui-kit` still depends on `gpui-pre` unconditionally, so
  that crate (and `gpui-pre-windows`' build script) is compiled too — see Gotchas. Do **not**
  switch either line to a git dependency on the Zed monorepo.
- MySQL: **sqlx 0.9**, `default-features = false`, only features
  `runtime-tokio`, `mysql`, `tls-rustls-ring`, `chrono`.
- SQLite: the same **sqlx 0.9** with its `sqlite` feature added by `rustgrid-sqlite` (this
  statically bundles SQLite through `libsqlite3-sys`, so a C compiler is required — `gcc` on
  Windows, see Gotchas).
- PostgreSQL: the same **sqlx 0.9** with its `postgres` feature added by `rustgrid-postgresql`
  (TLS via the workspace's `tls-rustls-ring`, so no new native dependency).
- Oracle: **`oracledb = "=26.0.0-beta.4"`** (Oracle's official pure-Rust thin driver, `default-features
  = false`; **pinned exactly** because it is a pre-release whose API is still changing). It needs no
  OCI / Instant Client; TLS reuses the workspace's `rustls 0.23` (already in the lock file), so it
  adds no native compilation. Its API is **synchronous**, so the driver bridges through
  `tokio::task::spawn_blocking` (see the `rustgrid-oracle` bullet).
- Redis: **`redis` 0.32** (redis-rs) with only `tokio-comp`, `cluster-async`, `sentinel` and
  `tokio-rustls-comp`, so a single client covers standalone servers, Sentinel deployments and
  Cluster (including `MOVED`/`ASK` routing) with rustls TLS and no OpenSSL. RESP2 is used on purpose:
  it works from Redis 2.8 through 7.x, while RESP3 needs 6.0.
- SQL Server: **tiberius 0.13** (`tds80` + `rustls` + `chrono`, `default-features = false`;
  `winauth` added on Windows by `rustgrid-sqlserver` through a per-target dependency entry, for
  Windows/SSPI authentication — the Unix `sspi-rs` backend is not enabled, as it pins
  `crypto-bigint 0.7.0-rc.8` against `russh`'s `0.7.5`) with
  **bb8 0.9** and **tokio-util** (`compat`). tiberius is not sqlx-based, uses rustls (so no OpenSSL),
  and its `Client` is `Send` but not `Sync`, so every operation checks out a pooled client. tokio's
  `net` feature is enabled for `TcpStream`.
- i18n: **rust-i18n 4**. Config dir: **directories 6**.
- Stored secrets: **chacha20poly1305 0.11** + **base64 0.22** (XChaCha20-Poly1305).
- Export: **rust_xlsxwriter 0.99** writes `.xlsx`; **rfd 0.17** (XDG-portal backend on Linux) drives
  the native folder/file dialogs. CSV/SQL/TXT need no dependencies.
- Import: **calamine 0.36** (`dates`) reads `.xlsx`/`.xls`/`.xlsm`/`.xlsb`/`.ods`; `.csv` is
  comma-separated and `.txt` auto-detects tab/comma/semicolon/pipe, both via a built-in RFC 4180
  parser with `encoding_rs` 0.8 for a GB18030 fallback. The same **rfd 0.17** drives the
  source-file dialog. Imported date/time values are normalised with **dtparse 2** (a Python
  `dateutil` port, day-first) to MySQL's canonical form before insert.

## Commands

- **After every code change, ALWAYS build automatically before finishing** (do not wait to be
  asked): run `cargo fmt --all` then `cargo build` (use the GNU toolchain env vars in Gotchas on
  Windows). A faster `cargo check -p rustgrid-app` (or `cargo check --workspace`) may be used while
  iterating; finish with `cargo build`. Debug builds do not need the shader toolchain below.
- `cargo check --workspace` / `cargo build`
- `cargo run -p rustgrid-app` (produced binary is `RustGrid`)
- The generic ODBC driver is a **default feature** (see `docs/odbc-driver.md`); disable it with
  `--no-default-features`. Either way there is no build dependency: the ODBC driver manager is loaded
  at runtime and is only needed if a user actually connects through ODBC.
- Release build (`cargo build --release -p rustgrid-app`) needs an HLSL compiler on Windows: on
  this machine the Windows SDK `fxc.exe` is absent, so build the fallback shim once
  (`gcc -O2 -o fxc.exe tools/fxc-shim/fxc.c -lkernel32`). The gitignored `.cargo/config.toml`
  then points `GPUI_FXC_PATH` at it via `[env]`, so a plain `cargo build --release` just works;
  `GPUI_FXC_PATH=<abs path to fxc.exe>` on the command line is only needed on a machine whose
  `.cargo/config.toml` lacks that entry. See Gotchas.
  The shim binary is gitignored; only `tools/fxc-shim/fxc.c` is tracked.
- `cargo test --workspace`
- Live MySQL integration test (ignored by default): set `RUSTGRID_MYSQL_PASSWORD` (and
  optionally `RUSTGRID_MYSQL_HOST`/`PORT`/`USER`/`DATABASE`), then
  `cargo test -p rustgrid-mysql -- --ignored`. It exercises connect, catalog listing, paging,
  and the auth-failure mapping against a real server.
- Live SQL Server integration test (ignored by default): set `RUSTGRID_SQLSERVER_PASSWORD` (and
  optionally `RUSTGRID_SQLSERVER_HOST`/`PORT`/`USER`/`DATABASE`), then
  `cargo test -p rustgrid-sqlserver -- --ignored`. It exercises connect, catalog listing, paging,
  editing, arbitrary SQL, schema introspection, backup/restore and views against a real server
  (e.g. Docker `mcr.microsoft.com/mssql/server`).
- Live PostgreSQL integration test (ignored by default): set `RUSTGRID_PG_PASSWORD` (and optionally
  `RUSTGRID_PG_HOST`/`PORT`/`USER`/`DATABASE`), then `cargo test -p rustgrid-postgresql -- --ignored`.
  It exercises connect, catalog/schema listing, paging with a filter, strongly-typed
  insert/update/delete, arbitrary SQL (multi-statement, dollar-quoted functions, transactions),
  schema management, table designer, views, routines, roles and backup/restore against a real server
  (e.g. Docker `postgres:17`).
- Live Oracle integration test (ignored by default): set `RUSTGRID_ORACLE_PASSWORD` (and optionally
  `RUSTGRID_ORACLE_HOST`/`PORT`/`USER`/`SERVICE`), then `cargo test -p rustgrid-oracle -- --ignored`.
  It exercises connect, catalog/schema listing, paging, editing and arbitrary SQL (including PL/SQL
  blocks and statements with a trailing `;`) against a real server
  (e.g. Docker `gvenzl/oracle-free:23-slim`, service `FREEPDB1`).
- Live ODBC integration test (ignored by default): set `RUSTGRID_ODBC_CONNECTION_STRING` (e.g.
  `Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;UID=sa;PWD=...;TrustServerCertificate=yes;`),
  then `cargo test -p rustgrid-odbc -- --ignored`. It exercises driver enumeration, catalog listing,
  query, paging, insert/update/delete and table management against the real driver manager.
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all`
- No `DATABASE_URL` or sqlx offline cache: queries use the **runtime** API (`sqlx::query`,
  not `sqlx::query!`). Do not add the `macros` feature.
- `Cargo.lock` is committed (this is an application, not a library). No CI yet.
- **Never move the version on your own.** The workspace version in the root `Cargo.toml`, the git
  tags and the GitHub releases change **only when the maintainer asks for that version** — do not
  bump one because changes landed, do not pick the number yourself, and do not tag, re-tag or delete
  a release unbidden. Write a version's section when (and only when) that version is bumped on
  request.
- **Every version change is recorded in both changelogs — no exceptions.** The moment the workspace
  version in the root `Cargo.toml` changes, the same commit writes that version's section into
  **both** `CHANGELOG.md` and `CHANGELOG_zh.md` (newest first, Keep-a-Changelog headings — Added /
  Changed / Fixed, translated in the Chinese file) describing what the version brings. **Keep entries
  short: one line per change, user-visible outcome only** — no implementation detail, no file names,
  no restating the issue; the section becomes the GitHub release notes verbatim, so a long entry
  makes an overlong release. **Each file stays in its own language**: `CHANGELOG.md` is English
  throughout — a UI label is quoted in its English form as `locales/en.yml` has it (选项 → 关于 is
  written `Options → About`) — and
  `CHANGELOG_zh.md` is Chinese throughout. The only exceptions are the language-switcher line, the
  name of the other language, engine/product names, identifiers, file paths and code spans. Never
  paste a Chinese label into the English file (or an English label into the Chinese one) just because
  that is what the UI, an issue or a commit message called it; a test
  (`crates/rustgrid-app/src/app/changelog.rs`) fails on CJK anywhere else in `CHANGELOG.md`. Those
  files — and any translated `CHANGELOG_<lang>.md` such as `CHANGELOG_ru.md` — are the single source
  of the version's own notes: the release job publishes a link-only body pointing at them
  (`CHANGELOG.md`, `CHANGELOG_zh.md`, plus any other `CHANGELOG_<lang>.md`) and fails when either
  must-have file lacks the section, alongside its tag-vs-`Cargo.toml` check. 选项 → 关于
  fetches the newer release's section — never this build's own — from the changelog for the UI
  language (`changelog_file`), falls back to English when that language has no file or its translation
  lags, and leaves the 更新日志 section off the page entirely while this build is the newest release
  (`crates/rustgrid-app/src/app/changelog.rs`). The release body is never the notes; the app only
  reads it as a fallback for a hand-written release, and the workflow's link-only body is ignored
  (`update.rs::release_notes` → `changelog::version_notes`). A version bump with no changelog entry
  therefore fails the app's own test (`changelog.rs`) as well as the release job — write the entry
  with the bump, never after it.

## Extensibility requirements (design for these from the start)

Keep engine-specific code out of the UI/app layers. Adding a second engine must not require
touching UI code.

- **Engine abstraction.** UI depends only on `rustgrid_core::{Driver, Connection}` and core
  models — never on `sqlx::MySql*` types or raw SQL strings.
- **Driver loading.** `DriverRegistry` plus the `DriverSource` trait are the loader boundary.
  Future runtime driver installation adds a `DriverSource`; keep each driver in its own crate
  so it can be built/distributed independently. Do not assume a single bundled driver.
- **Per-engine connection pages.** `DriverDescriptor::connection_form` (`ConnectionFormSpec`) is
  the whole page spec: `home` (`ConnectionHomePage::{Network,File,Odbc}`) shapes the 常规 page,
  `tabs` lists the pages shown after it (`ConnectionPage::Tls`/`Tunnel`/`Advanced`), `labels`
  renames the standard fields per engine (Oracle's database field is a 服务名/SID, SQL Server's the
  初始数据库) and may hide one while an option is outside its `visible_when` list (Oracle hides
  服务名/SID for the `tns`/`connect_string` types), and `options` declares the engine's own fields —
  each a `ConnectionFieldSpec` with a `profile.options` key, i18n `label_key`/`hint_key`, a
  `ConnectionFieldKind` (`Text`/`Secret`/`Folder`/`Select`), the `page` it sits on and an optional
  `visible_when` condition. `advanced` (`AdvancedFields`) gates the standard 高级 fields
  (connect/query timeout, keep-alive, read-only, init SQL) to the ones an engine honors, so no
  driver shows a dead knob. `optional_fields` lists standard fields the engine treats as optional
  — Redis's user name is an ACL account, so a blank one means password-only `AUTH` / the `default`
  user and the label carries no `*`. 常规 also carries **使用 TLS/SSL** and **使用 SSH/HTTP/SOCKS
  隧道** switches; the TLS and tunnel tabs are only listed while those settings are on, so a plain
  local connection shows just 常规 + 高级. The app renders each driver's pages generically (never matching an
  engine id) and round-trips the options through `ConnectionProfile::options`, so an engine gets its
  own page set without touching UI code and without migrating `connections.json`. Engine option
  fields on 常规 render **before** 记住密码, so a controlling field (Oracle's 连接类型) sits directly
  under the field it controls. The 常规 page does **not** repeat the engine in a read-only box — the
  window title (`新建/编辑连接 · <engine>`) already names it. The generic ODBC driver is the one that
  keeps the plain form (`home: Odbc`) and declares no TLS/tunnel/advanced pages (it reads none of
  them). The username a fresh form starts with is the engine's own
  (`Driver::default_username`: `root`, `postgres`, `sa`; empty where there is no conventional
  account), never one engine's account shown for another.
- **Per-engine table design.** `DriverDescriptor::table_designer` (`TableDesignerSpec`) is the whole
  designer spec, read from the driver through the registry (`AppView::table_designer`) — never by
  matching on an engine id. `tabs` lists the sub-tabs (`DesignerTab`) in order, `column_options` the
  per-column controls of the detail panel under the field list (`DesignerColumnOption` — `Default`,
  `Collation`, `IdentityGeneration` (a drop-down) and `IdentityStart`/`IdentityIncrement` (text) are
  editors, the rest are check boxes), `field_columns` the field-list cells after 列名/类型
  (`DesignerFieldColumn`, where `Length`/`Size`, `Decimals`/`Scale` and `Generated`/`Computed` are
  the same cells under different names), `option_fields` the rows of the 选项 page
  (`DesignerOptionField`) and `default_suggestions` the common expressions offered by the chevron
  beside 默认, each narrowed to the column types it suits (`DefaultSuggestion::applies` filters them
  against the selected column, so 长度/类型 changes change the list). The default is MySQL's shape,
  so an engine opts in by declaring its own, and only a control the engine's designer SQL actually
  honors is listed, so no driver shows a dead knob.
  MySQL/MariaDB keep 自动递增/无符号/填充零 and every tab; SQL Server declares 排序规则/稀疏,
  大小/比例/计算列, and the extra 唯一键/检查 tabs; SQLite has no 选项/注释 (it has neither) and
  Oracle no 选项, while PostgreSQL's 选项 keeps only 注释 and its column detail adds the identity
  sequence's 生成方式/起始值/增量 (`ColumnDef::identity`/`identity_start`/`identity_increment`).
  The character-set /
  collation catalogues are queried only when the spec shows a control fed by them
  (`TableDesignView::needs_option_catalogs`). `ColumnDef` carries the per-column attributes
  (`sparse`) and `TableSchema` the SQL Server-only constraint lists (`unique_keys`, `checks`), so a
  driver fills in only what its engine has and the two extra grids stay empty elsewhere. Every
  designer grid renders through `TableDesignView::render_grid_scaffold`, which also owns the grids'
  wheel handling: a `uniform_list` registers **no** wheel handler of its own (unlike gpui's `list`),
  so — exactly as the table grid does — a capture-phase `canvas` hitbox picks the gesture up and the
  wheel moves the selected row one step and brings it into view through `scroll_to_item` (the only
  scrolling a uniform list performs), a horizontal gesture pans the columns, and a gesture aimed at
  an overlay above the grid is left to it (`should_handle_scroll`). Never write the list's scroll
  offset directly: the row extent is computed from the row count (`design_scroll_extent`) because
  `ScrollHandle::max_offset` is not the list's own extent. Where an auto-increment may sit comes from
  the same spec (`TableDesignerSpec::auto_increment`, an `AutoIncrementRule`): the field details
  panel leaves the control out on a type the engine does not accept and moves the flag off whichever
  column had it, and `change_existing: false` (SQL Server, whose `ALTER COLUMN` cannot add an
  `IDENTITY`) restricts editing to a newly added column or a new table. A grid cell that holds a
  *list* of columns is a drop-down built on `design_picker_cell`: the whole cell is the trigger (no
  "..." button), it opens flush under the row you clicked (each row records its own anchor in
  `DesignPickerAnchors`, a map keyed by row — a single shared anchor would open the list under
  whichever row painted last). The 索引 cell opens `IndexFieldsDraft`, an editor panel whose rows
  carry the engine's per-column options (`TableDesignerSpec::index_columns`: `Sort` everywhere,
  `PrefixLength` for MySQL/MariaDB) and which owns its per-row controls
  (`index_field_combos` / `index_field_prefix_inputs`, rebuilt on every structural change); every
  edit stays on the draft until 确定 and 取消 / a click outside discards it. The 索引 grid lists the
  table's secondary indexes only: the primary key's name and kind are the key's own (not editable)
  and its columns are the 字段 tab's 主键 flags, so — like Navicat — it gets no row. The model keeps
  the entry, pushed last (`push_primary_index_last`), so a grid row is always its own model row:
  `TableDesignView::index_row_count` is what the grid's row count, the scroll extent
  (`design_row_count`) and 添加索引's insert position all use. The 外键 / 唯一键 cells keep the
  check-box list.
  The designer owns a key context of its own (`TableDesigner`, on the root div, which
  `request_focus` focuses when the tab opens or is brought to the front — a key binding only fires
  while its context is in the focused element's path): 保存 from `ctrl-s` / `cmd-s` (`SaveDesign`),
  and `tab` / `shift-tab` (`NextDesignerCell` / `PrevDesignerCell` → `tab_designer_cell`) walking the
  fields grid's cells — 列名 → 类型 → the engine's text cells (`field_cell_chain`), rolling onto the
  neighbouring row at the row's end (`field_cell_neighbor`) and handing the keystroke back to the
  window's focus chain (the detail panel below the grid) only past the grid's first/last cell. The
  engine's toggles (不是 null / 主键 / 虚拟 / 稀疏) are not Tab stops — they have no editor to focus.
  Arrow keys work like the table grid too: while the designer root holds focus, Left/Right/Up/Down
  move `selected_cell` (clamped at the grid edges, keeping the cell kind vertically) and Enter/F2
  open that cell's editor. Inside a cell editor Up/Down (`DesignerRowUp`/`DesignerRowDown`, bound at
  `TableDesignerCell > Input`) commit and move one row and Left/Right stay with the caret; Enter
  commits and moves down. Focus returns to the designer root after the move, so the next arrow keeps
  navigating. The fields grid uses the table grid's own selection model (`CellSelection` /
  `CellRange` in `field_selection`): a click selects one cell, drag or Shift-click extends a
  rectangular range, Ctrl/Cmd-click toggles a range, Ctrl+A (`select_all_fields`) selects every
  cell, and a press on the left gutter selects/extend whole rows. A click only selects; a second
  click / double-click / Enter / F2 opens the cell's editor (`field_mouse_up`), exactly like the
  table grid, and a drag never opens one. Painting is per cell (`CellSelection::contains`), not
  from `selected_cell`, so a range shows every cell it covers. 删除字段, 上移 and 下移 act on every
  row the selection touches (`selected_field_rows`); `move_field` moves those rows one slot as a
  group (`moved_field_rows`), keeping their internal order. Ctrl+C (`designer.copy`) copies the
  selected cells as TSV, using the table grid's convention: the selection's bounding rectangle,
  with unselected cells in it empty. Tab must be claimed by a key binding
  rather than an element's `on_key_down` (`ui::TextInput::on_tab`
  never fires for it: gpui dispatches a keystroke a binding matches as an action, and `Root` binds
  `tab`). The 类型 drop-down is driven from the keyboard: `type_highlight` marks a row, Up/Down
  (`ui::TextInput::on_arrow`) move it and `keep_type_highlight_visible` scrolls it in, Enter takes
  the highlighted type and a search that narrows the list restarts the highlight at its top.
- **Internationalization.** Route every user-facing string through `t!`. Keys live in
  `crates/rustgrid-app/locales/{en,zh-CN}.yml` and must be added to **all** locale files.
  `t!` returns `Cow<'_, str>`; call `.to_string()` before handing it to a gpui element.
- **Shortcuts are commands.** A user-facing keystroke is a `keymap::Command` (`src/app/keymap.rs`):
  an id stored in `settings.json`, a label, a group for Options → Shortcuts, a key context and its
  defaults. Add a command there (and its action and label) instead of a hard-coded `KeyBinding`, so
  it is listed, searchable and rebindable. See `docs/keyboard-and-query.md`. Every list page offers
  刷新 in its toolbar and in the row/blank-area context menu; F5 is the `general.refresh_page`
  command bound in the content pane's `PAGE_CONTEXT`, while a focused grid's deeper
  `grid.refresh` binding still wins.
- **SQL text is lexed once.** `src/app/sql_lex.rs` colours the query editor, decides what is a
  comment (`--`, `//`, `#` on MySQL, `/*` to the end while unclosed), rewrites what the engine
  would not accept before a run, and splits MySQL scripts (`DELIMITER`, routine bodies). Keep
  highlighting and execution on that one scanner so they cannot disagree.
- **Config storage.** Use `rustgrid_config::ConfigStore`. It owns these files under the OS
  config dir: `connections.json` (`CURRENT_VERSION`), `settings.json` (`SETTINGS_VERSION`),
  and encrypted `secrets.json` + `secret.key`; `session.json` holds the workspace session (open
  tabs, connections and databases, restored on start — `src/app/workspace.rs`). Bump the matching
  version and extend `migrate()` when a schema changes. **Passwords never live in a profile** —
  they are keyed by profile `id` and stored encrypted (see Gotchas).

## UI conventions

- **Look: shadcn/ui on Navicat's layout.** The structural layout is Navicat's — a custom 34px
  titlebar, menu bar, command toolbar, then the connection tree + content pane — but the chrome is
  the shadcn look delivered by gpui-kit: rounded corners, muted borders, solid `primary` fills,
  ghost/outline button variants. Do not re-introduce bespoke square Win32 chrome. Every window
  draws its titlebar through `src/app/ui/titlebar.rs`, a thin app wrapper over
  `gpui_kit::component::TitleBar`: `ui::window_options(title)` sets the transparent titlebar and
  `app_owns_titlebar_drag`, `ui::title_bar(theme)` applies the app palette, `ui::bar_title` adds
  the logo + title, and `ui::window_title_bar` is the child-window bar. The kit draws the window
  controls itself (native caption buttons on Windows, window-manager controls on Linux, native
  traffic lights on macOS), so do not add app-drawn min/max/close buttons back. The main window's
  theme/GitHub/update/settings cluster must stay inside `ui::action_cluster`: it occludes the kit
  bar's full-width `WindowControlArea::Drag` hitbox, without which Windows hit-tests those buttons
  as `HTCAPTION` and they never receive a click. Closing goes through the platform close path and
  `Window::on_window_should_close`; do not put the unsaved-work confirmation on a custom button.
- **Use gpui-kit components first.** Reach for `gpui_kit::component::*` before hand-drawing a
  control: `button::Button` (via `AppView::win_button` / `dialog_button` / `toolbar_item`),
  `checkbox`, `Dialog`, `Input`, `Combobox`, `Scrollbar`, `Tab`, `Table`, `Theme`. Anything
  shared by more than one view belongs in `src/app/ui/`, never inline in feature code.
- **The `ui/` wrappers own the app-facing API.** `ui::TextInput` wraps gpui-kit's
  `InputState`/`Input` and `ui::ComboBox` wraps `ComboboxState`/`Combobox`, exposing the app's
  original signatures (`TextInputOptions`, `on_change`/`on_submit`/`on_cancel`/`on_tab`,
  `set_text`, `set_options`, `set_selected`, ...). Because those gpui-kit states require a
  `&mut Window` to construct, the inner state is created on the **first render** and pending
  mutations/callbacks are flushed there. Do not use `InputState`/`ComboboxState` directly in
  feature code — extend the wrapper instead.
- **Dialogs are `Root`-managed, not app-drawn.** `AppView` holds the dialog state (`form`,
  `db_dialog`, `password_prompt`, `error_dialog`, `delete_confirm`, `options_open`);
  `dialogs.rs::sync_dialog` maps it to a `DialogKind` and opens/closes
  `window.open_dialog(...)` accordingly. Dialog bodies are rebuilt every render through
  `Entity<AppView>::update`, so status (e.g. "testing connection") stays live. Dialogs are
  centered and **not** draggable. There is no `overlay`/`dialog_frame`/`confirm_dialog`
  machinery any more — do not add one back.
- **Colors come from `Theme`** (`src/theme.rs`), resolved from `ThemeSetting` +
  `window.appearance()` on every `render`. The palettes follow shadcn's "zinc" tokens (keep
  `Theme::light()`/`Theme::dark()` in sync) and the same `render` calls
  `gpui_kit::component::Theme::change(...)` when the resolved mode flips, so gpui-kit
  components match. Reference colors as `rgb(theme.field)`; don't hardcode palette values.
- **Icons are embedded assets, with a gpui-kit fallback.** Register every new SVG in
  `Assets::load` (`src/assets.rs`) and load it with `svg().path("icons/foo.svg")` (or
  `Icon::default().path(...)` for gpui-kit components); `Assets::load` delegates unknown paths
  to `gpui_kit::assets::Assets`, which serves the bundled Lucide set. Bitmaps use
  `img(ImageSource::Resource(Resource::Embedded("logo.png".into())))`. An unregistered path
  that is not Lucide fails to load silently.
- **Toolbar/tab items are flat, not push buttons.** The main toolbar
  (`AppView::render_main_tab`) and the object toolbar (`AppView::toolbar_item`) are borderless
  icon+label items (ghost buttons) with thin `toolbar_separator`s between them — do **not**
  style them with borders or filled buttons. `win_button`/`dialog_button` are for push buttons
  inside dialogs. `ui::toolbar_item` supplies the content as explicit children (16px icon, 12px
  label) because gpui-kit's default button typography (16px) is too large for the app's chrome.
- **The grid header can carry two extra lines: the field's type (数据) and comment.** With the 数据
  options on (`AppSettings::grid_show_types` / `grid_show_comments`),
  `grid_view.rs::header_cell_text` paints each column's data type, then its comment, under the name
  at 10px muted (the type sits between the name and the comment), and the header grows by
  `GRID_HEADER_LINE_HEIGHT` (14px) per line (`GridView::header_height`). Every line is clipped to the
  column with `truncate()` (an ellipsis) and shown in full in a hover tooltip when it does not fit.
  Both options are off by default, and each line only appears while the grid actually has such a
  column. The date/time picker positions itself from the same header height
  (`grid_cell.rs::render_date_picker`) instead of assuming one row.
- **The 字段筛选 popup is an anchored, click-away-dismissed panel, and it configures the grid's field
  *layout*.** `app/grid_fields.rs` draws it as a `deferred` pair under `root`: a transparent
  `.occlude()` backdrop (`FIELD_FILTER_BACKDROP_PRIORITY`, whose `on_mouse_down` dismisses) and the
  `ui::popup_panel` on top (`FIELD_FILTER_POPUP_PRIORITY`), positioned from the toolbar item's
  window rect (recorded by the `on_children_prepainted` wrapper in `render_grid_toolbar`) minus the
  pane's (`GridView::grid_pane_bounds`, the **union** of the root's children rects — taking only the
  first child gave the toolbar's width but its 28px height, which pinned the list to its minimum
  height and made the popup look short). Because the backdrop covers the pane *including* the
  trigger, a second press on the trigger dismisses rather than re-opens — the trigger keeps a plain
  `on_click`. Rows reorder by dragging only their `drag_handle.svg` handle (never the row, which
  would fight the visibility toggle) using the pointer's travel over fixed-height rows. The toolbar
  item carries the **label 筛选 (the same `grid.filter` string as the WHERE panel's button) with one
  icon**, and says everything about its state through colour: the brand accent (`theme.brand`) once a
  field is hidden or re-ordered (`GridState::field_layout_configured`), the plain chrome text colour
  (`theme.text`) while the grid shows the table's natural field set — so the icon is colourless until
  a layout is set — and no background tint of its own (the open popup is the feedback). Every edit
  (visibility, 重置, 全选, a drag) lands on a **draft** (`GridView::field_filter_draft`), not on the
  grid: 确定 (`field_filter_apply`) installs it, remembers it per table and closes, while 取消, Escape
  or a click outside just discards it, so the grid only ever shows an applied layout. The footer is
  重置 (`field_filter_reset`: table order + every field shown), 全选 (`field_filter_show_all`: every
  field shown) and 反选 (`field_filter_invert` → `GridFieldLayout::invert_visibility`, which never
  hides the last visible field) on the left, 取消 / 确定 on the right. The popup title
  stays 字段筛选, which is what tells it apart from the WHERE button.
- **A field layout never touches the row data.** `GridState` keeps the engine's columns in
  `all_columns`, the display order in `column_order` (display position -> natural index, visible
  fields first), and `visible_columns`; `columns`/`column_widths` are the *projection* of that order
  while `rows` stay in engine order and are read through `GridState::source_col` (see
  `grid_view.rs::source_cell`). Hiding, re-ordering or resetting a field therefore re-projects
  metadata only — no refetch, no row copy — and every index-based path (cell reads, commit keys,
  selection, hit testing, sums) must map through `source_col`/`visible_columns` rather than assuming
  `columns`/`rows` line up. `session.rs::permute_in_place` applies the re-order in place, and
  `reproject_fields` composes the new order with the one already on screen.
- **The object list (Tables/Views) follows Navicat's layout.** It is column-major: items are
  chunked into columns of `object_rows_per_column()` (derived from the scroll viewport height,
  `OBJECT_ROW_HEIGHT` per row) so a column fills top-to-bottom and then wraps to the next column
  to the right, and it scrolls **horizontally** (`overflow_x_scroll`), not vertically. The
  number of rows per column therefore adapts to the window height. Do not switch it back to
  `flex_row()`/`overflow_scroll()`. A plain mouse wheel pans it horizontally: `ColumnGrid::scroller`
  wraps the scroll area in gpui-kit's `ScrollableMask::vertical_wheel`, because gpui gives an
  `overflow_x_scroll` container only the horizontal delta, so the wheel would otherwise do nothing
  (trackpad and horizontal wheels are left to the scroller, and the wheel chains to the parent at
  the edge). gpui-pre does **not** paint scrollbars for `overflow_*`
  (it only scrolls and reserves space), so the list ships its own horizontal scrollbar
  (`render_object_hscrollbar`, driven by `object_scroll`); keep it in sync when touching the
  object list.
- **Floating popups must be built with `ui::popup_panel`.** gpui dispatches `ScrollEvent`
  to *every* scrollable hitbox under the cursor — each `overflow_*` container handles it and none
  stops propagation — so a popup whose list overlays a scrollable pane scrolls **both** it and the
  pane behind it. The only fix is hitbox behavior: a `BlockMouse` hitbox (`.occlude()`) makes all
  hitboxes behind it report `should_handle_scroll() == false`. `ui::popup_panel(theme)` is the
  single shared builder for app-drawn floating surfaces (dropdown lists, menus, pickers): it sets
  `absolute` + `.occlude()` + the dialog face/border/shadow, so scrolling inside a popup never
  scrolls the view underneath. Never hand-roll a `div().absolute()` popup; build it on
  `popup_panel`. gpui-kit's own `Combobox`/`Select`/`Popover` already occlude, so they are fine.
- **A menu that opens at the pointer is positioned by gpui's `anchored`, never by a guessed
  height.** `render_context_menu` / `render_tab_menu` wrap their `popup_panel` in
  `anchored().position(pointer).anchor(Anchor::TopLeft)` and leave `left`/`top` to it (the wrapped
  panel is `.relative()` inside it). `anchored` measures the menu after layout and, when it would
  overflow, switches its anchor corner — growing up/left from the pointer — falling back to snapping
  it to the window edge. A fixed height estimate used to clamp a menu opened on the last row far
  above the click, so the menu floated in the middle of the pane instead of under the row. The
  flyouts' `left(relative(1.0))` still resolves against the panel (it is the nearest positioned
  ancestor), so their placement is unaffected.
- **A hover-opened submenu flyout closes through a two-rectangle zone, not through its trigger
  row's hover alone.** The menu's flyouts (颜色, 管理分组, 转储 SQL 文件, 导入连接) open while the
  pointer is on their row, but a flyout built on `popup_panel` occludes the row behind it, so the
  row immediately reports `hovered = false` once the pointer crosses onto the flyout — a row-only
  `on_hover(false)` therefore cannot tell that from "the pointer left the menu" (the flyout used to
  stay up until the whole menu was dismissed). Each flyout's `SubmenuZone` keeps the flyout's rect
  (recorded by the row's `on_children_prepainted`) *and* the row's, recorded by wrapping the row in
  `AppView::submenu_row`, and the row **and** the flyout both close/open from
  `SubmenuZone::stays_open`, which consults the pointer position: the flyout stays up while the
  pointer is inside either rectangle and closes as soon as it leaves both. Keep the two flyout
  positions (`left(relative(1.0))`, i.e. 100% of the row's padding box) abutting the row: a gap
  between them would be inside neither rectangle, so crossing it would close the submenu.
- **Dropdowns share one style: search on top, a check mark on the selected row.** gpui-kit's
  `Combobox` (via `ui::ComboBox`) already renders a searchable list and a right-aligned check that
  is invisible when unselected (so rows never shift). `ui::ComboBox` defaults to the kit's
  `Size::XSmall` (20px, 12px text) so every dropdown in the project is the same compact size; the
  filter builder is the one exception and opts into the design's taller trigger via
  `ComboBox::height` / `text_size` / `radius`. The table designer's type dropdown
  (`design_view.rs::render_type_combo`) is hand-built and must mirror the compact default: a 20px
  `ui::TextInput` search (`TextInputOptions { size: Some(Size::XSmall), .. }`) above 20px rows, and
  an `icons/check.svg` on the right of the selected row, `opacity(0)` on the others.
- Keep dialog controls compact: 12px text, ~24px-high buttons, compact text fields. gpui-kit's
  sizing defaults are larger than the app's metrics, so `ui::TextInput` renders the kit input at
  `Size::Small`, disables the kit's focus ring (the "shadow" that otherwise floats over the field)
  and pins the text to `DEFAULT_TEXT_SIZE` (12.5px); in-place cell editors (grid/table designer)
  additionally pass `TextInputOptions { bare: true, text_size: Some(..) }`, which drops the kit's
  background, border and inner padding so the field blends into its cell. The grid renders its
  editor **inside** the editing cell (`editing_cell` in `grid_view.rs`) instead of as an absolute
  overlay, and skips that cell's own text — so a transparent `bare` editor never ghosts, and
  scrolling the list moves the editor with its row instead of leaving it floating at a stale
  offset. A selected grid cell paints `Theme::grid_selection_bg` (blue) with
  `grid_selection_text`.
- Grid selection is **Excel-like and multi-range**: `CellSelection` holds `Vec<CellRange>` plus an
  `active` index, and a cell is painted selected when any range contains it. Plain click starts a
  new single-cell range (and opens the editor on release), **Shift+click/drag** moves the active
  range's `cursor`, and **Ctrl/Cmd+click** toggles the clicked cell's range in/out
  (`add_or_remove_range`). Anything that reads a "selection" — status bar, delete, `set_selection_null`,
  multi-cell edit, header highlight — must go through `contains`/`cells`/`row_indices`/`col_indices`
  rather than assuming a single rectangle.
- Embed bitmap images with `img(ImageSource::Resource(Resource::Embedded("name".into())))`.
  Calling `img("name")` treats the bare filename as a **URI** and fails with
  `Failed to load asset ... loading image asset from "name"`.

## Upgrading gpui-kit

`gpui-kit` and the workspace's `gpui` alias are the same engine and **must be bumped together** in
the root `Cargo.toml`; a mismatch gives two GPUI copies and incompatible element types. The alias is
`gpui = { package = "gpui-fast", … }` while the kit's own engine line is `gpui-kit`, and the kit's
`gpui-fast` feature is what makes the two agree — bump both, and re-check that the feature is still
called that. Two kinds of upgrade:

- **A patch bump (no code):** `cargo update -p gpui-fast -p gpui-kit -p gpui-pre`. The `"0.1.12"` /
  `"0.8"` ranges in the root `Cargo.toml` keep this in-family (the `-p gpui-pre` is only there
  because the kit pins it too).
- **A breaking bump (e.g. `gpui-kit 0.9`, a new `gpui-fast` snapshot):** edit both lines in the root
  `Cargo.toml`, then fix the fallout. It is localized by design:
  - `ui/text_input.rs`, `ui/combo.rs` (the wrappers) and the dialog builders
    (`dialogs.rs`, `db_dialog.rs`, `options.rs`) absorb gpui-kit component API changes; the
    40+ call sites behind them do not move.
  - The rest is plain GPUI API churn. The `0.6` → `0.7` bump only needed the main window to
    move from `cx.open_window` + `Root::new(AppShell)` to `gpui_kit::open_window` (0.7's `Root`
    hosts dialogs/sheets/notifications automatically, so `AppShell` was deleted). The
    `gpui 0.2` → `gpui-pre 0.3` jump was ~41 mechanical errors across ~17 files
    (`focus(&h, cx)`, `ScrollHandle::max_offset()` returning `Point`, `ShapedLine::paint`
    taking `TextAlign` + `Option<Pixels>`, `BoxShadow { inset }`, `track_scroll(&h)`,
    `Entity::update` returning `R`). Budget one pass like that per breaking engine bump.
  - `0.7` → `0.8.1` (gpui-fast 0.1.7 → 0.1.12, gpui-pre 0.3.9 → 0.3.10) was exactly one file
    plus one helper: keystrokes are no longer parsed from runtime strings.
    `Keystroke::parse` takes a `'static` string and `KeyBinding::new` inherits that, so the app's
    rebindable commands (`settings.json` overrides) go through `keymap.rs::load_binding`, which
    calls `KeyBinding::load(..., cx)` (host-OS aware, so `secondary` resolves correctly) with a
    `KeyBindingContextPredicate`; display strings use `Keystroke::parse_with_os(.., cx.platform_info().os)`
    (`shortcut_label` now takes `OperatingSystem`, and `keymap::conflicts` takes it too), and
    `Modifiers::secondary()`/`secondary_key()` likewise take `&App`. The only other delta was
    `ScrollWheelEvent`, renamed to `ScrollEvent` (with a new `source: ScrollSource` field).
  - `Cargo.lock` is committed, so pin exactly what built.

## Dependency policy

- **Do not reinvent the wheel**: use mature, stable third-party crates for non-trivial
  functionality (connection helpers, UI widgets, config storage) instead of hand-rolling.
- Verify a crate's maturity before adopting it — prefer widely used, actively maintained crates.
- SQL language work uses `sqlparser` (lexer/parser/keyword list) and `sqlformat` (beautifier);
  do not add a bespoke SQL lexer/formatter. Syntax highlighting maps `sqlparser` tokens to colors
  (`sql.rs`), completion combines `sqlparser` keywords with loaded table names, and `Beautify SQL`
  calls `sqlformat`.
- Keep the dependency tree small (see Gotchas).
- **UI widget reference — `longbridge/gpui-kit`** (https://github.com/longbridge/gpui-kit,
  formerly `gpui-component`, **Apache-2.0**). It **is** a dependency of this workspace (the
  `gpui-kit` facade, see Tech stack): use its components rather than hand-drawing controls, and
  when a widget is still missing (toast, tooltip, virtualized table, ...) prefer
  wiring the kit's component over porting an implementation. Only fall back to porting upstream
  code when the kit genuinely lacks the widget; keep ports under `src/app/ui/` and retain the
  upstream Apache-2.0 attribution. Upgrade procedure and blast radius: "Upgrading gpui-kit".
- **The grid's in-place date/time picker reuses the kit's `Calendar` + `TimeField`.** `DatePicker`
  holds an `Entity<CalendarState>` (`gpui_kit::component::calendar`) as the source of truth for
  the date and an `Option<Entity<TimeFieldState>>` (`gpui_kit::component::time_field`, precision
  `Second`) for `datetime`/`timestamp` columns; `grid_cell.rs::render_date_picker` renders both
  and only app-draws the today/OK/Cancel footer. Do not hand-roll the calendar or the time
  spinners again. Picking a date/time does **not** mirror into the cell editor; the picker's 确定
  (`date_picker_ok`) syncs once, closes the popup and returns focus to the in-place editor, so a
  temporal cell commits exactly like every other cell editor — on blur, Enter, or the bottom 提交
  button, never by confirming the picker. 取消/Esc discard. While a temporal cell is in edit mode
  it shows a `…` button (`grid_view.rs`) that toggles the popup: it opens it from the editor's
  value, or applies-and-closes it when already open. The popup opens **only** from that `…` button
  — `begin_edit` no longer auto-opens it for temporal cells. The footer's 取消/确定 use
  `ui::popup_button` at `Size::XSmall`.
  The kit's styled `Calendar` facade is sized for a full popup (28px cells), so
  `ui/calendar.rs::compact_calendar` wraps the **unstyled** `gpui_base::Calendar` and overrides
  the cell metrics through its `Calendar::item` hook (the styled facade exposes no item hook);
  the localized labels come from the app's own `calendar.week.*` / `calendar.month.*` entries in
  `locales/{en,zh-CN}.yml`, because the kit's `Calendar.*` translations live in the kit's
  `rust-i18n` backend and are not reachable from the app's `t!`. Keep the popup from stealing
  focus: gpui focuses any `track_focus` element on mouse-down, so the calendar is wrapped in
  `capture_any_mouse_down(|_, window, _| window.prevent_default())` (the `TimeField` is left
  focusable so it can be keyboard-edited), and the cell editor's blur handler skips its
  auto-commit while `date_picker` is `Some` (clicking another cell still commits through
  `grid_mouse_down`).

## Scope (do not exceed)

Keep Navicat's layout (connection tree on the left, content/details pane on the right) with the
shadcn chrome described above. Implemented today:

1. Connection management: create/edit/delete, connect/disconnect, password prompt, and the
   connection right-click menu's 颜色 submenu (红/黄/绿, clearing by re-picking the active colour),
   which stores a per-connection environment colour (by profile id, in `settings.json`). The colour
   paints the connection node (and its whole subtree) in the tree — visible whether the connection
   is open or not, and kept on the selected/hovered row — and tints every tab of that connection in
   the content pane's tab bar (the bar itself stays neutral so tabs never blend into it; the
   active tab is markedly stronger and bold so it stands out, and the pinned 对象 tab keeps the
   colour while it is the active tab, so the environment stays visible on 用户/查询/备份 too), so
   production stands out. Tab titles ellipsize at 170px and show their full text on hover.
   Dragging a file onto the window (or onto `RustGrid.exe`, which forwards it to the running
   instance) opens it through the generic drop frame in `app/file_drop.rs`; SQLite is the only
   recognised format today, and `classify_dropped_file` is the extension point for the next one.
2. Enumerate databases; create/edit/delete a database (charset + collation) and edit defaults
3. Per database, list tables and views
4. Show a selected table's data in a grid with **pagination** — display only, **no editing** yet
5. SQL query editor tabs (`New Query` button / `Queries` main tab): multi-line editor with SQL
   syntax highlighting and keyword/table completion, `Beautify SQL`, run arbitrary SQL against a
   chosen connection + database, `Explain`, and a result grid that reuses the table grid
   (controls, scrollbars, status, and in-place editing when a single table can be inferred).
   **导出结果** exports the result sets through the Export Wizard: one result set directly, or a
   multi-select dropdown (结果1 / 结果2 / … plus 全选) when the last run produced several. Several
   selected results are written into **one `.xlsx` workbook**, one worksheet per result; other
   formats keep one file per result.
6. Right-clicking a table (in the object list or the connection tree) offers Open/Design plus Drop
   Table, Empty Table (`DELETE FROM`), Truncate Table and Rename; Drop/Empty/Truncate go through
   the shared confirm dialog, while Rename (or `F2` on the selection) edits the name **in place**
   in the row that started it. These run through `Connection`
   (`drop_table`/`empty_table`/`truncate_table`/`rename_table`), never as raw SQL written in the UI.
7. Saving named queries: `Save` (or `Ctrl+S`) in the query editor overwrites the `.sql` file an
   already-saved tab is bound to (`QueryTab::saved_path`), without prompting; only an unsaved tab —
   or one whose bound file was renamed/deleted from the list — opens a dialog for the query name and
   save location (connection + database). `Ctrl+S`/`Cmd+S` are bound in the editor's
   `QUERY_EDITOR_CONTEXT`, which `query_editor.rs::render_query_editor` sets on the focus container
   (key bindings only fire while that context is in the focused element's path). Saved queries are
   listed under the `Queries`
   main tab in a **sortable three-column list** (query name / connection / database; click a header
   to sort). The list is scoped by the connection tree's selection — a selected database shows only
   that database's queries, a selected connection shows all of its databases, and any other
   selection shows every saved query. Rows open with a double-click and are deleted through the
   shared confirm dialog. They persist in the config dir's `queries.json` (`SavedQuery`, keyed by
   connection profile id + database name).
8. Table export wizard: with a table selected, **Export Wizard** (object toolbar / table context
   menu) opens a separate OS window walking four pages — P1 format (`.xlsx`/`.csv`/`.sql`/`.txt`),
   P2 tables + output paths (native folder/file dialogs via `rfd`, defaulting to the desktop),
   P3 exported columns per selected table (source-table dropdown when several are chosen, all
   fields by default), and P4 options + run (include column titles, continue on error) with a
   live log/progress view. Writes go through `rustgrid-export`; the wizard streams table pages via
   `Connection::fetch_page` (ordered by the primary key when the table has one).
   A grid's own 导出 first asks **全部记录 / 当前 N 条记录 / 取消** (Navicat's choice): 当前记录
   exports exactly the rows on screen — the applied filter and sort and the current page — through
   the same wizard, as an in-memory snapshot instead of `fetch_page`.
9. Import wizard: **Import Wizard** (object toolbar / table context menu, when a database is
   open) opens a separate OS window walking four pages — P1 import type (`Excel File`, `CSV File`,
   `Text File`), P2 the merged source page (a native file dialog picks the workbook or delimited
   file; each worksheet — or the single file-named table for CSV/TXT — becomes a row with an
   editable destination table and a **Create Table** check box), P3 field mapping (a destination
   dropdown per source column, auto-matched by normalised header name), and P4 run/log. A source
   table whose name matches an existing table targets it; otherwise the name becomes the table name
   and **Create Table** is ticked automatically (`database_table_names` + `find_existing`).
   Field matching is case/separator-insensitive (`normalize_key`). A created table's columns and
   types are inferred from the data by `rustgrid-import::infer_columns` (integer → `bigint`,
   decimal → `decimal(20,6)`, dates → `date`/`datetime`, booleans → `tinyint(1)`, text → `varchar`/
   `longtext`), and the `id` column becomes the primary key (forced `NOT NULL`). Rows are inserted
   in `IMPORT_BATCH_ROWS` batches through `Connection::insert_rows`; unmatched source columns are
   simply not imported. Date/time values are parsed by `dtparse` (day-first, so `26/6/2025` is
   26 June) and rewritten to MySQL's canonical `YYYY-MM-DD[ HH:MM:SS]`, so text sources with local
   date conventions import into `date`/`datetime` columns.
10. User management: the `Users` main tab lists the server's accounts (toolbar: 编辑 / 新建 /
   删除 / 权限管理器, plus a search field). **New User** and **Edit User** are the *same* separate
    OS window (`user_create.rs`, `UserCreateWindow` observing `AppView`), and the *same* window
    creates or edits the account. There is no 快速视图/完整视图 split: a left navigation
    (`UserSection`) shows five sections over one shared `UserEditorState` — 常规 (identity,
    authentication, account lock, password expiry and the online resource limits), 服务器权限
    (the global `*.*` grid with one-click 快捷模板), 权限 (a database list beside the selected
    database's detail: 全部表/指定具体表 scope, quick presets and grouped fine-grained privileges),
    角色 (the 成员属于 / 成员 lists, whose 集 column grants
    `WITH ADMIN OPTION`, granting the role first if needed) and SQL 预览 (the script Save would
    run). Save is one path for both flows; changing the username/host emits `RENAME USER` before
    the rest of the script so the account keeps its grants and role edges, and a successful save
    reloads the account in place. Passwords are prompted on connect, never stored in a profile.
    Disconnecting a connection closes the account window and the 对象权限 window with it (and drops
    the Users list), so no window is left editing an account over a dead connection.
11. Stored routines (functions and procedures): the `Functions` main tab lists a database's
    routines from the connection tree's selection, with the toolbar 设计函数 / 新建函数 /
    删除函数 / 运行函数 (and a matching right-click menu in the object list and the connection
    tree's Functions category). 设计函数 opens a routine editor as a main-content tab that reuses
    the SQL editor: its toolbar is 保存 / 运行 / 停止 / 查找 / 自动换行 and its sub-tabs are
    定义 (the full editable `CREATE ...` statement, loaded from `SHOW CREATE`), 信息 (metadata
    and creation settings) and SQL 预览 (the script Save runs). Save derives the routine's kind
    and name from the statement, drops the original and replays the `CREATE` through
    `Connection::save_routine`; 运行 calls the routine (`SELECT` for a function, `CALL` for a
    procedure) in a new query tab, running it immediately when it takes no arguments.
12. Views: the `Views` main tab lists a database's views from the connection tree's selection, with
    the toolbar 打开视图 / 设计视图 / 新建视图 / 删除视图 / 导出向导 (and a matching right-click
    menu in the object list and the connection tree's Views category). 设计视图 opens a view
    designer as a main-content tab that reuses the SQL editor: its toolbar is 保存 / 预览 / 解释 /
    视图创建工具 (a deliberate stub) / 美化SQL and its sub-tabs are 定义 (the full editable
    `CREATE ... VIEW` statement, loaded from `SHOW CREATE VIEW`), 高级 (definer, security,
    algorithm, check option, updatability, timestamps and charset/collation) and SQL 预览 (the
    script Save runs). Save derives the view name from the statement, drops the original and
    replays the `CREATE` through `Connection::save_view`; 预览 opens the saved view's data in a
    grid and 解释 runs `EXPLAIN` over the definition's `SELECT`, showing the result in a bottom
    信息/解释 panel inside the designer (信息 is the executed statement plus status, 解释 is the
    plan grid, reusing `GridView`). The designer is a `QueryTab` whose `view` (a `ViewTabState`) is
    set, exactly like the routine editor.
13. List interaction: the Users, Backup, Tables and Views/Functions object lists all share one
    Explorer-style list behaviour (`src/list_select.rs` + `app/list_ops.rs`): a plain click replaces
    the selection, Ctrl/Cmd+click toggles a row, Shift+click extends from the anchor, and a
    rubber-band drag over the list background selects every intersected row. Every page has the
    icon-only 详细列表 / 平铺网格 toggle (tooltip on hover) placed to the left of, and adjacent
    to, its search box; each page remembers its own layout by a stable `VIEW_PAGE_*` key and
    **defaults to 平铺网格**. Both layouts come from the shared `ui` chrome, so all pages look the
    same: one `ui::grid_item`/`grid_column` tile template (20px rows, 16px `leading_icon_badge`) and
    one `ui::detail_header_row`/`detail_row` table template (shared `gap_3` so header and data
    columns line up, one compact row metric — see item 17 — and `ui::DETAIL_BADGE_SIZE` badges).
    The detail lists have Navicat's compact columns, but each is
    **fitted to its content** (the widest of its header label and its visible cells, via
    `ui::detail_column_width`) and **drag-resizable** from the handle on its header's right edge
    (per-page `ui::DetailColumns` state, seeded through `resolve` and updated by `begin_resize` /
    `drag_resize` / `end_resize`; a page routes its list's mouse-move/up to those):
    Backup 名称/修改日期/文件大小/备注, Queries 名称/修改日期/文件大小, Users 名称 + the resource
    limits + 超级用户, Functions 名/修改日期/函数类型/决定性/注释, Views 名/可以更新, Tables
    名/自动递增值/修改日期/数据长度/引擎/行/注释 (the table overview columns come from
    `Connection::table_statuses`, one bulk query loaded with `list_tables`). F2 renames the
    selected item in place (accounts, backups and tables/views). Keep `ListSelection`
    layout-independent so both layouts share the selection, and publish row rectangles through
    `AppView::note_row_rect(MarqueeTarget::…, …)` from each row's `on_children_prepainted`. The
    Tables object list additionally supports Ctrl/Cmd+A (select every visible row) and dropping
    every selected table through one confirm dialog (`DeleteConfirm::Tables` → `run_drop_tables`).

14. Connection-tree groups (`app/groups.rs`): the tree is a nested list of groups and connections
    (`AppView::tree_layout`, persisted in `settings.json` as `ConnectionGroup`/`GroupEntry`, which
    bumped `SETTINGS_VERSION`). A connection is referenced by its profile id, so the layout survives
    reordering/deleting profiles; `reconcile_tree` prunes removed connections and appends new ones
    at startup. Right-clicking the blank tree area offers 新建分组 / 刷新; a group's menu offers
    新建连接 / 重命名 / 删除分组 / 刷新 (F2 renames in place, a new group is named 新分组 and enters
    edit mode); deleting a group keeps its children by splicing them into the parent. A
    connection's 管理分组 submenu lists 移动到 (every group, indented by depth) and 从分组内排除
    (enabled only while the connection is in a group). Connections and groups are drag-reorderable
    (drop before/after a row, or into a group's middle) with a `deferred` insertion indicator;
    groups nest arbitrarily and a group cannot be dropped into its own descendant.

15. Connection import/export (`config::transfer` + `app/transfer.rs`): the New Connection dropdown
    and the connection tree's blank-area menu end with a separator, then 导入连接 (a submenu with
    导入RustGrid 连接 / 导入Navicat 连接 / 导入DBX 连接) and 导出连接. Each import entry opens a
    native picker pre-filtered by its extension (`.rgc` / `.ncx` / `.json`|`.dbx`), parses on
    tokio's blocking pool, and adds the connections straight to the tree (deduplicated names,
    passwords written to the encrypted store, unsupported engines and undecryptable passwords
    counted and reported) — there is no preview step. A `.rgc` also carries the connection tree's
    `groups` (plus each connection's profile id) so an import restores the groups and the order;
    other formats import flat. Navicat passwords are decrypted with the right scheme for the file:
    v2 (AES-128-CBC, `libcckeylibcckey` / `libcciv libcciv `, Navicat 12+) when the hex decodes to
    whole 16-byte blocks, else v1 (Blowfish, Navicat 11). 导出连接 opens a separate OS window (like
    the export/import wizards, since the save picker must not run on gpui's event thread): a
    checkable connection list with 全选/取消全选 and 导出密码. 确定 opens the native save dialog
    (defaulting to `rustgrid-connections.rgc` on the desktop), and the window closes once the file
    is written. When 导出密码 is ticked the passwords are written **encrypted** (XChaCha20-Poly1305
    under a fixed built-in key, `password_protection: "builtin"`) rather than as plaintext; unchecked
    the file carries no password but still imports. All six engines are read (MySQL/MariaDB,
    PostgreSQL, SQLite, SQL Server, Oracle, ODBC); Navicat/DBX Oracle service-vs-SID and SSH tunnels
    are mapped, and an absent port falls back to the engine default.

16. Grid field presentation (`app/grid_fields.rs` + `app/options.rs` + `session.rs::GridFieldLayout`):
    the table grid's toolbar ends with the 字段筛选 item (labelled 字段, after 导出, so it is not
    confused with the WHERE 筛选 button). It opens a popup panel under the button (not a child
    window) listing every field with a visibility check box, a search box, the field's comment
    beside its name, and 重置 / 全选 / 反选; the last visible field
    cannot be hidden (the same rule 反选 honors) and a drag on a row's handle re-orders the fields.
    The edits stay on a draft and only 确定 applies them (取消 / click-away discards); the applied
    layout (order + hidden set) is then remembered per connection/database/table in `settings.json`
    (`grid_field_layouts`), so the table reopens with the same field set (a field added to the table
    afterwards is appended rather than lost). One icon serves both states, told apart by colour —
    the plain `theme.text` for the table's own field set, the accent `theme.brand` once a field is
    hidden or re-ordered — and the popup's counter reads `visible/total`. The 表格 page of the Options window
    (`OptionsSection::Data`, between 编辑器 and 关于) holds the two grid display switches, 展示备注
    (`AppSettings::grid_show_comments`) and 展示类型 (`grid_show_types`), both off by default, which
    paint each column's comment / data type under its name in the grid header. Right-clicking a
    grid column header opens its own menu — 冻结列 / 取消冻结 / 取消冻结全部列 / 隐藏列 / 刷新.
    Freezing moves the clicked field to the front of the display order and pins it there; freezing
    more fields appends them in that order (first frozen field = first column, second = second,
    ...), so the 字段 popup lists the frozen fields first too. The frozen set lives in
    `GridState::frozen_columns` (by field name, in freeze order) while `GridState::field_layout`
    keeps the unfrozen order, so 取消冻结 / 取消冻结全部列 puts every field back where it was
    without a snapshot. Each frozen header and 字段 row carries a lock glyph; a frozen row cannot
    be dragged in the popup, and 隐藏列 unfreezes a hidden field. The columns after the frozen block
    pan inside their own clipped strip. 隐藏列 writes the same `GridFieldLayout.hidden` set the 字段
    popup edits (and the menu listener persists it), so the field is listed unchecked there.

17. 详细列表 density and sorting (`ui/views.rs` + every list page): all detail lists share one
    compact row metric — `DETAIL_ROW_HEIGHT` (28px), `DETAIL_BADGE_SIZE` (18px) and a 32px header —
    so Tables/Views/Functions, Users, Backup, Queries and the backup configurations line up and read
    as the same table. Every column is **sortable**: a click on a header sorts by that column
    (ascending), a second click reverses it, and another column starts ascending there. The state is
    one `DetailSort` per page (`DetailSort::toggle` / `indicator` / `sort_by`), the arrow comes from
    `ui::detail_header_cell`, and the cell values are mapped to `ui::SortKey` (text compared
    case-insensitively, counts/sizes/dates where the engine reports them, missing values first). The
    sort lives beside the page's `DetailColumns`/`DetailScroll`, so it survives a re-render (and a
    pane's Tables ↔ Views switch falls back to the name order when the column no longer exists). A
    table's overview columns come from the same `table_statuses` map the cells render from, so the
    sort and the visible values always agree; the Backup list sorts files and saved configurations
    as one list (a configuration has no date/size, so it sorts into the empty group there).

18. Query-page comfort settings (`app/options.rs`, `AppView::zoom_editor_font`,
    `ui/sql_editor.rs`): the 编辑器 page adds **Ctrl + 鼠标滚轮调整字体大小**
    (`AppSettings::editor_ctrl_wheel_zoom`, on by default). `SqlEditor` takes an `on_zoom` callback
    and claims the wheel gesture in the **capture** phase over a hitbox covering the editor (a plain
    wheel still scrolls it; a scrollable container handles the wheel in the bubble phase), so the
    app can step the font size along the same `EDITOR_FONT_SIZES` ladder the 字号 dropdown offers
    and remember it. The 编辑器 settings now reach the editors that are already open: 确定 pushes the
    font, 显示行号 and 自动换行 into every query editor, 信息 log and SQL 预览 pane
    (`SqlEditor::set_display_options`), instead of only the editors opened afterwards. The 常规 page
    adds **启动时恢复** (`AppSettings::restore_session`, on by default, tooltip: restore query tabs,
    connections, open databases and table tabs at the next start) — the workspace session is then
    neither restored nor saved, so a start with it off is a clean one, and the saved connection,
    database and group expansion is not restored either (the tree starts collapsed) — and the
    object-search switch is now simply **保留对象搜索** (`preserve_object_search`, on by default).
    The one search box is shared by 表 / 视图 / 函数 / 查询, and `AppView::object_searches` files
    its text **per list** (keyed by the `VIEW_PAGE_*` page of the shown list,
    `AppView::object_search_page`), so 表's
    search never filters 视图 or 查询; `sync_object_search` (called once per render) swaps the
    field's content when the shown list changes, `set_object_search` writes the typed text to that
    list's entry, `clear_object_search` (Esc) clears just that one and
    `clear_object_search_unless_kept` clears every list on the page/database/schema/connection
    switches it guards. With the setting on each list's search survives all of those, so one search
    covers the same table name in every environment; 用户 and 备份 already keep their own box.
    **Clicking a main tab always clears that tab's own search** (`clear_main_tab_search`; 表 / 视图
    / 函数 / 查询 through `object_searches`, 用户 / 备份 through their own field), so re-entering a
    list always shows every object — the setting only covers the database/schema/connection
    switches. A
    `settings.json` written before this version had the old `false` default, so loading turns the
    setting on once; files this version writes keep the user's choice.

    The query result panel's tab strip (信息 + one 结果 tag per result set) scrolls horizontally and
    shows ◀ / ▶ paging buttons **only while the tags do not all fit** (`QueryTab::result_tabs`,
    mirroring the main tab bar). The strip's overflow is measured by a canvas after layout (like
    `DetailList`), so the buttons appear on the next frame instead of waiting for another render,
    and selecting a result scrolls its tag into view.

19. Redis (`crates/rustgrid-redis` + `app/grid_key.rs`): a Redis connection is browsed through
    the shared object list. For `DriverCapability::KeyValue` the database node expands to its
    **namespace folders** (key prefixes with counts, from `list_namespaces`) instead of SQL
    categories; the database row itself is the all-keys view (the prototype's key count sits on
    it), so there is no 全部 row and no 查询 category under it. The Tables list is a key table
    (键名 / 类型 / TTL / 内存 / 值) that **always** uses the detailed layout (its 详细列表 / 平铺网格
    switch is not shown). The 值 column is a one-line preview from `TableStatus::preview` — a string
    key's leading bytes (`GETRANGE`, so a multi-megabyte value never travels) shortened in the cell
    with the rest on hover, and a collection's entry count (`TableStatus::rows`), since a collection
    has no single value. **Every key opens on its own key page**, never the SQL grid:
    string/ReJSON keys get the prototype's RAW / JSON switch and a large editor (JSON is a
    read-only pretty print; invalid JSON falls back to RAW, with a warning; Java long literals such
    as `103L` are accepted), while hash, set, list, zset and stream use a Field/Value, Member,
    Index/Value, Member/Score or ID/Field/Value table (no `#` row-number column) with
    添加字段/添加成员/添加元素, 删除选中, 保存修改 and 刷新. Streams and ReJSON keys are read-only.
    The key header bar shows the type badge, key name, TTL, memory and collection actions; the
    table pages carry a compact pager instead of the SQL record toolbar, WHERE/sort panels or
    field layout. The key-list toolbar follows the prototype: a filled 新建键, an outlined 删除,
    plain 设置 TTL / 刷新 actions and the active namespace chip; 新建键 and 设置 TTL are
    `Root`-managed modals (`app/dialogs.rs`), not a separate tools window. 新建键 shows the chosen
    type's own first-entry fields as plain single-line inputs — one 初始值 for a string/list, 成员
    for a set, 字段 + 值 for a hash/stream and 成员 + 分数 for a zset — never a multi-line editor for
    all of them. `list_objects_page`
    loads the configured `redis.key_limit` in one go, so there is no 加载更多 button. SQL-only
    designer/import/export items are hidden. The main ribbon keeps only 表 / 查询 for a Redis
    connection. See `docs/redis-driver.md`.

20. Row form in the right-hand pane (`app/info_pane.rs`): the grid footer's 编辑表单 button (right
    of the page-size gear, `icons/form.svg`) opens the selected row as an editable form in the
    right-hand pane — the first selected row when several are selected. The button only lights up
    (and is clickable) while a data row is selected; query result grids, Redis key pages and the
    designer's preview grids have their own layouts and no such button. The form and the
    object-details pane share the right side, so opening the form closes the details pane (and
    opening the details pane from the status bar closes the form). The form turns itself off when
    the selection disappears (page turn, refresh, tab close), so it never sits over an empty
    selection. Because the pane is narrow, each field is one `字段 · 类型 · 注释` line with its
    editor below instead of Navicat's three columns; the type and comment follow the 表格 options
    (`grid_show_types` / `grid_show_comments`) and every segment is ellipsized with the full text
    on hover when it does not fit. Typing stages straight into the grid's `edits` (the grid cell
    shows the pending value after the field is left; undo/redo is the focused input's own,
    Ctrl+Z / Ctrl+Y — Cmd+Z / Cmd+Shift+Z on macOS — exactly like the grid's in-place cell editor),
    Enter saves through the grid's normal commit path, Ctrl+S (`keymap.form.save`, bound in the
    form's `RowForm` context) saves too, and Escape restores the page's value for that field.
    A long text value (over 80 characters, containing a newline, or a `text`/`blob`-like column) and
    every multi-line JSON value use the multi-line `ui::TextArea` (`EditorState`; JSON points it at
    the `json` grammar) with a three-row default height and a bottom drag handle that resizes it.
    A JSON value renders as a code block: a header strip carries the `json` label on the left and a
    `格式化` switch on the right (on = pretty-print, off = compact; both stage the rewritten text),
    and the editor underneath shows line numbers and syntax colours. JSON/JSONB columns holding a
    scalar value (`123`, `"text"`, `true`, `null`) keep the ordinary single-line editor. The header (table name + ✓ / ✕) stays fixed above the field list, which is the only part that
    scrolls; the buttons are muted grey while the grid has nothing staged and take their save /
    rollback colours once it does. A `NULL` cell shows an empty field
    with a `NULL` placeholder. A temporal field (`date` / `datetime` / `timestamp`, the same types
    the grid's cell picker accepts) carries the grid picker's "…" button; it opens an inline
    `compact_calendar` + `TimeField` panel under the
    editor with the app-drawn 今天 / 取消 / 确定 footer, and 确定 writes the formatted value into the
    editor and stages it exactly like typing it. The per-keystroke staging path must not notify the
    grid: the grid clones its whole page when it repaints, and the pane re-renders every field, so
    notifying per character made typing (and IME composition) lag. Only the first/last staged
    change of a run and the field's blur repaint, and the pane must never `set_text` a field that
    holds focus — its change callback is dispatched on the wrapper's next render, so the model can
    lag one keystroke and rewriting it would drop that keystroke and clear the input's undo
    history.

21. Connection-tree search (`app/sidebar.rs` + `app/tree.rs`): a search box at the top of the
    connection tree filters what the tree already holds — connections, databases, schemas and
    tables. A table matches its bare name, its schema-qualified `schema.table` and its
    fully-qualified `database.schema.table` (`database.table` on engines whose tree has no schema
    level), so `ORDER.ORDER_INFO` (or `sales.ORDER.ORDER_INFO`) finds the table without expanding
    the ORDER schema first. A schema matches its name or `database.schema`. The filter only sees
    objects that are already loaded: opening a database loads all its schemas' tables, which is
    what makes the qualified search work, while a database that was never opened is matched by its
    name only and the search never issues a catalog query of its own. A matching branch keeps its
    immediate children (a matched connection shows its databases, a matched database its schemas),
    the branches on the way to a match open, only 表/视图 are drawn while filtering (with just the
    matching objects), and a group without a surviving connection is skipped. Down / Up in the
    search box hands the keyboard to the filtered rows — Down selects the first table/view result,
    Up the last (falling back to the first/last row when only a branch matched) — and the tree keeps
    the focus, so its arrows, Page Up/Down and Enter work from there. Esc or the field's ✕ clears
    the filter and restores the saved tree; 无匹配项 is shown when nothing matches.

Still out of scope: engines other than MySQL, MariaDB, SQLite, SQL Server, PostgreSQL, Oracle and
Redis, and the disabled placeholder UI
(the `Design/New/Delete Table` toolbar buttons and the query editor's `Query Builder`/`Snippets`
items are deliberate stubs — leave them disabled unless asked). The abstractions above are what
make more engines cheap later — do not build those features early.

## Gotchas

- The grid filter builder is a **tree**, not a flat list: `GridState::filters` / `filter_draft` are
  `Vec<FilterNode>` where a node is either a `FilterCondition` or a `FilterGroup` (nested children,
  parenthesized). Each node carries its own `FilterConjunction` (how it joins its previous sibling),
  so a group may mix `AND`/`OR`; the root is an implicit list. UI rows are addressed by a node
  **path** (`Vec<usize>`, indices from the root) — keep that in sync when adding node operations.
  `rustgrid-mysql::filter_clause` and `session.rs::filter_display_clause` recurse over the tree,
  skip disabled/incomplete nodes, drop groups left empty, and must keep bind order identical to the
  rendered `?` placeholders. The filter builder is a compact take on the reference design: 24px
  controls, a 66px left rail, and 22px `−`/`+` row actions. Every in-group condition row leads with
  its **enable checkbox** in a fixed left column (so the boxes line up down the panel, at the card's
  left edge), followed by the row's `并且/或者` pill when it has one; the pill is a **single-label**
  toggle that shows the node's current conjunction and flips it on click (no two-segment switch).
  The checkbox turns the node — and, for a group, its whole subtree — off. Each group is a
  bordered card (10px radius, 8px padding/gaps); between cards runs a 24px connector — a full-width
  hairline with the floating conjunction control (checkbox + pill) centred on it, bound to the
  **next** group's conjunction, and 18px `−`/`+` actions revealed on hover to its right (in a
  balanced slot so the pill never shifts). The connector after the last group has no next group to
  join, so its control stays hidden until hovered; condition-row actions are always visible. The
  condition block fills the pane (responsive) up to 700px; the hover-only
  divider on its right edge (so it does not read as a scrollbar) drags a manual width (min 520,
  max 700) and double-clicking it restores auto-fit (`filter_resize_drag`).

- **A key-down listener must never claim a printable keystroke to "use" its character.** On
  Windows `WM_GPUI_KEYDOWN` runs `TranslateMessage` only while the event propagates, so calling
  `cx.stop_propagation()` for a letter/digit/symbol suppresses the `WM_CHAR` that would type it.
  Type-ahead handlers that only move a list's highlight (the connection tree's
  `sidebar::nav_typeahead`, the object list's `ObjectPane::object_key`) therefore leave the event
  propagating, and skip working at all while an in-place rename editor is open inside them —
  `stop_propagation` there silently made the group rename editor untypable in every non-IME input
  mode. A claimed character is fine only where the handler itself inserts text.

- **A connection-tree row's action must not re-enter the tree pane.** gpui runs an entity's event
  listener inside that entity's own update, so a tree row that calls an `AppView` method which
  updates `TreePane` borrows the tree a second time and aborts the app. `AppView::activate_grid`
  (reached by `select_table` when the table's grid is already open, e.g. after a session restore)
  therefore defers its database-row selection one effect cycle with `cx.defer`; the tree's
  namespace/query/backup rows defer the same way. Keep new row actions deferred, or fix it in the
  `AppView` method like `activate_grid` does.

- **An empty grid shows gpui-kit's empty state, not a painted empty table.** With no rows to paint,
  `GridView::render` swaps the `uniform_list` for a compact, centered
  `gpui_kit::component::empty::Empty` card (inbox icon, `grid.no_rows` title and a muted hint) rather
  than stretching the kit surface across the whole body. It carries no hitboxes, so nothing in it is
  selectable; a click just focuses the grid. Do not bring back a decorative per-column filler: it
  repeated the table header as blank cells and needed a measured `empty_grid_height` that fed back
  into layout.

- **A pending insert row is not always at the bottom, so never index a displayed row into
  `state.rows`.** `GridView::inserts` holds `PendingInsert { values, after }`: the `+` button appends
  (`after: None`, the old behaviour), while 复制行 (`duplicate_row`) copies the selected record into a
  row **anchored under it** (`after: Some(page row)`) and opens the in-place editor on its first
  column. Map a displayed row through `GridView::row_at` / `grid::row_origin` (and
  `grid::display_of_insert` for the reverse) everywhere a row index is turned into data, and keep
  `state.edits` keyed by the **page** row index, not the displayed one: the clone shifts every row
  below it. The clone leaves `ColumnInfo::auto_increment` / `generated` columns unset so the engine
  assigns them, skips `CellValue::Bytes` (the insert path carries text, so a BLOB's hex preview must
  not be written back), and copies hidden columns too.

- **A running fetch is a spinner inside a bar the grid already draws, never a bar of its own.** The
  old `加载中...` strip was a `flex_none` child of the grid root, so every page load pushed the rows
  down and back up. `GridView::render_loading_spinner` is now appended to the grid toolbar's far end
  (`render_grid_toolbar`, right-aligned by a `flex_1` spacer) and, for grids without a toolbar (SQL
  results), to the status bar's right group next to the page info; a grid with neither (the view
  designer's preview) floats it over the header's right end with `absolute`. Do not put it back in
  the flex column — the pane must not reflow for a fetch.

- **Read-only text the user must copy is selectable, but `SelectableText` alone does not copy.**
  `gpui_kit::base::SelectableText` participates in the window's selection — a mouse drag selects —
  but Ctrl+C only reaches it while something inside the run holds focus, because `gpui_base::Root`
  dispatches its `Copy` action along the focused element's path. Nothing focuses a plain text run, so
  a drag over one selects and Ctrl+C does nothing (verified in
  `ui/sql_editor.rs::tests::readonly_preview_selects_and_copies`). Text the user must actually be able
  to copy therefore renders through a **read-only `ui::SqlEditor`** (the SQL 预览 panes: the table
  designer's 预览 tab, the user window's, the 对象权限管理器's, and the routine/view designers') — the
  kit's editor handles Copy itself, and gives SQL syntax highlighting and scrolling with it. The main
  window is opened with `gpui_kit::open_window`, whose `gpui_base::Root` renders the one
  `TextSelectionLayer` selection needs — a plain `div` opened with `cx.open_window` does not, so wrap
  such a window in `gpui_kit::component::Root` (as the wizards do) before relying on it. The grid
  status bar's SQL line (`grid_toolbar.rs::render_status_sql`) instead owns its selection, because it
  must treat **double-click as select-all** (base multi-click would pick a word): it keeps the
  laid-out `StyledText`, paints the selected range with a `HighlightStyle`, and exposes copy /
  select-all through its own `"GridStatus"` key context (`CopyStatusSql` / `SelectAllStatusSql`) — the
  `Root` context's `ctrl-c`/`ctrl-a` would otherwise swallow the keystroke before it reached the line.

- **The filter builder's value fields are ordinary `ui::TextInput`s** (caret, mouse selection,
  clipboard and IME from gpui-kit), one entity per `(node path, slot)` in
  `GridView::filter_value_inputs` and dropped on any structural change (like the field/operator
  combos). The pager's 页码 / 每页记录数 boxes are `ui::TextInput`s too (`grid.rs::make_page_input` /
  `make_page_size_input`): the shared field owns caret, selection, clipboard and IME, `accepts`
  keeps them digits-only, `on_submit` commits (Enter), `on_cancel` restores the current value
  (Escape) and the page-size box's `on_arrow` steps by 100. `GridView` no longer implements
  `EntityInputHandler`; a new single-line field should be a `ui::TextInput` rather than a bespoke
  append-only editor.

- **SQL Server (tiberius) is not sqlx.** `tiberius::Client` drives one request at a time and is
  `Send` but **not `Sync`**, so `SqlServerConnection` holds a `bb8::Pool` and checks a client out per
  operation. `OFFSET n ROWS FETCH NEXT m ROWS ONLY` requires an `ORDER BY`, so `fetch_page` orders by
  the primary key when no sort is requested and `ORDER BY (SELECT NULL)` otherwise. Result-set
  statements use the parameterized RPC path, DML uses the RPC path for its row count, and everything
  else (DDL, `CREATE PROCEDURE`/`CREATE VIEW`) is sent as one text batch so the server receives it
  intact; `helpers::split_statements` splits scripts on top-level semicolons while tracking
  `BEGIN`/`CASE … END` and falls back to the whole script when blocks are unbalanced. Decimal/numeric
  values decode to `CellValue::Text` (their exact `Numeric` string) rather than a lossy `f64`; money
  decodes to `Float`. The driver's `columns`/`fetch_page` fully qualify objects because a pooled
  session's database cannot be assumed. Three T-SQL quirks matter: `DROP VIEW`/`DROP PROCEDURE`/
  `DROP FUNCTION` reject a database-qualified name (`DROP TABLE` accepts one), so those drops `USE`
  the database and use a two-part `[schema].[name]`; `SET IDENTITY_INSERT` does **not** carry over
  across `sp_executesql` requests, so restore wraps `SET IDENTITY_INSERT … ON`, the `INSERT`, and
  `… OFF` in one plain text batch; and a script that starts with `DECLARE` is sent whole (batch-scoped
  variables cannot span the split statements). `list_tables`/`list_routine_infos` return
  **schema-qualified names** (`dbo.users`), and every table/view/routine operation parses that prefix
  (`resolve_object`). The connection tree therefore inserts a schema level — 数据库 → dbo/… →
  表/视图/函数/查询/备份 — for any engine whose object names are schema-qualified (SQL Server,
  PostgreSQL);
  schema-less engines (MySQL/SQLite) keep the flat database → category tree. Queries and backups are
  stored per database, so they appear (duplicated) under every schema node. The object pane is scoped
  to the clicked schema for tables/views/functions; queries/backups stay database-scoped.
- **A plain click on an already-open database (or schema) shows its objects; opening stays on the
  double-click.** Double-click / Enter / 打开数据库 keep their original semantics
  (`open_database` opens a closed database and toggles an open one's expansion; the schema
  equivalent is `select_schema`). A *plain* click additionally calls
  `AppView::show_database_objects` (schema rows: `show_schema_objects`): a closed node is only
  highlighted, an open one shows its objects for the active main tab (表/视图/函数) through
  `show_objects_for_scope`, which reuses an object pane already on that scope (its scroll/selection
  survive) and just re-targets its category, or rebuilds it otherwise. 用户/查询/备份 are left alone
  — those panels follow the tree selection themselves. Without this click path a database whose pane
  had been replaced by another connection (or cleared when its connection was closed — `disconnect`
  clears a pane still scoped to it) looked empty until it was double-clicked.
- **The tree's 刷新 (connection / group / blank area / database row) also re-fetches the objects
  behind it.** `load_databases` takes the loaded list out and merges it back so open/expanded
  databases and the table cache survive a refresh — which by itself made 刷新 a no-op when a table
  had been dropped outside the app. It now records every database whose `tables`/`routines` were
  already `Loaded` (or `Failed`) and, once the merged node is in place, re-runs `load_tables` /
  `load_routines` for them **by their new list position**, so the tree and the object list show the
  server's current state. Keep that pairing if the merge (or the object cache) changes: reloading
  before the merge would be discarded by it, and reloading only the tables leaves the Functions tab
  stale.
- `rustgrid-mysql::map_connect_error` flags authentication failures as
  `rustgrid_core::Error::Authentication` by checking `MySqlDatabaseError::number()`
  (1044/1045/1698) — **not** `DatabaseError::code()`, which returns the SQLSTATE (e.g.
  `28000`). The app keys its password prompt off this variant.
- The MySQL account editor reads database/table/routine grants from the
  `information_schema.*_PRIVILEGES` views **including `IS_GRANTABLE`**
  (`rustgrid-mysql::user::row_privileges`). A database the account holds only an entry for
  (`GRANT USAGE ON db.* WITH GRANT OPTION`) is reported as the keyword `USAGE`, which the privilege
  catalog does not know; `IS_GRANTABLE` is the only sign of it, so dropping it hid that database
  from the editor (Navicat lists it, and `SHOW DATABASES` returns it). Never drop a row just because
  its `PRIVILEGE_TYPE` is unknown — read its grant option. `REVOKE GRANT OPTION ON db.*` is what
  actually removes such an entry; `REVOKE USAGE …` / `REVOKE ALL PRIVILEGES …` leave it in place.
- `rustgrid-mysql::decode_cell` uses sqlx's **checked** `try_get` (which enforces
  `Type::compatible`) for every known type, then falls back to `try_get_unchecked::<Vec<u8>>`
  + UTF-8 for text-encoded types (DECIMAL, JSON) and raw bytes otherwise. Do not reorder to
  put `String` first or use `try_get_unchecked` for numeric/binary types: checked decoding is
  what prevents raw bytes being misread as the wrong type.
- sqlx 0.9's `sqlx::query` only accepts `&'static str` (the `SqlSafeStr` bound). A
  dynamically built query must be wrapped: `sqlx::query(sqlx::AssertSqlSafe(sql))`. Keep
  identifiers escaped (`quote_identifier`) and use bind parameters for all values.
- Grid writes identify a row by its primary key, falling back to **every column** when the table
  has no primary key (`GridState` column `primary_key` flags). Key values are
  `Option<String>` (`CellValue::as_edit_value`): `None` is SQL `NULL` and must render `IS NULL`,
  never be bound as an empty string — binding `''` for a numeric `NULL` column fails with
  `1292 Truncated incorrect INTEGER value: ''`. The same applies to `delete_rows` keys.
  `Connection::execute_query` only returns result-set metadata, so editable query grids re-attach
  the catalog's `primary_key`/`data_type` flags both on first run (`run_query`) **and on every
  reload** (`GridView::reload_query`); missing the reload path silently degrades edits to the
  every-column key and reintroduces the `1292` above.
- MySQL DDL (`CREATE`/`DROP`/`ALTER DATABASE`) **must** run through the text protocol:
  `sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(&pool)`. Using `sqlx::query` prepares the
  statement and MySQL rejects it with `1295 ... not supported in the prepared statement
  protocol yet`. Reserve `sqlx::query` for parameterized DML/`SELECT`.
- **A pure column reorder is invisible to the MySQL designer diff.** `ColumnDef` carries no
  position, so `existing != column` is false when a field only moves; `schema_sql` therefore
  compares the relative order of the columns present in both schemas
  (`reordered_column_names`) and emits `MODIFY COLUMN ... FIRST` / `AFTER` for those. Without
  that, 上移/下移 changes the grid but 保存 reports 暂无更改.
- The query editor runs arbitrary SQL through `Connection::execute_query`
  (`rustgrid-mysql`), which uses the text protocol on a **pinned pooled connection** (so `USE db`
  and the statement share a session). Since MySQL cannot tell the caller whether a statement
  returns rows ahead of time, `returns_result_set` classifies it by the leading keyword: that path
  uses `raw_sql(..).fetch_all` (falling back to `prepare` for column metadata on a 0-row result),
  everything else uses `raw_sql(..).execute` for `rows_affected`. Keep the keyword list in sync
  when adding statements that produce result sets.
- Query results are stored as ordinary `GridState` entries in `AppView::grids`, flagged by
  `GridState::sql` being `Some` and `show_toolbar == false`; `QueryTab::grid_id` links back to
  one. This is what lets the editor reuse `render_grid` (scrollbars, controls, status, editing).
  Paging is client-side (`page_size = rows.len()`), and `load_page` re-runs the stored SQL via
  `reload_query_grid`. Editing is enabled only when `sqlparser` infers a single-table `FROM`
  (`sql::infer_single_table`).
- The export wizard is a separate OS window (like Backup), not a `Root` dialog, because the
  `rfd` folder/file pickers must not run on gpui's event thread: the **synchronous** `rfd` API
  shows a nested Windows message loop inside event handling and crashes the app. The pickers use
  `rfd::AsyncFileDialog` awaited from a gpui task, which runs the dialog on rfd's own thread.
  The wizard reads table rows through
  `Connection::fetch_page` in `EXPORT_PAGE_SIZE` pages (ordered by the primary key when there is
  one, for stable offsets) and feeds them to `rustgrid_export::TableWriter`, which streams CSV/SQL/
  TXT and buffers `.xlsx`. Field selection is applied by projecting each page's columns down to the
  chosen ones, so the writer's header and the row values stay aligned. A table with no primary key
  exports in the engine's natural order.
- The import wizard is likewise a real window (`ImportWindow` observing `AppView`). It reads the
  workbook on tokio's blocking pool through `Runtime::spawn_blocking` — `calamine` is synchronous
  and must not run on gpui's event thread. It opens the workbook once per read (`sheet_names`, then
  `read_sheet` per sheet), and the whole sheet (header + rows) is held in `ImportSheetFields` so a
  freshly created table can infer its column types. Keep new source formats inside
  `rustgrid-import`; the app only sees `SheetData`/`infer_columns`.
- DB work is tokio-based but gpui's executor is not tokio. Inside `cx.spawn`, run sqlx
  futures through `Runtime::spawn` and `.await` the returned `JoinHandle` (see
  `app.rs`). Awaiting sqlx directly in a gpui task panics with "there is no reactor running".
  **The same rule governs what may be dropped where.** A `sqlx::PoolConnection` returns itself to
  its pool by spawning onto the Tokio runtime and panics with "this functionality requires a Tokio
  context" when that happens outside one — so a value that owns a pooled connection and outlives a
  single `Runtime::spawn` call must be dropped *on the runtime*, never on gpui's executor. This bit
  the Run SQL File feature: its pinned `ScriptSession` (MySQL/MariaDB, SQLite, PostgreSQL) was
  dropped on the gpui thread when the run ended, aborting the app at the moment a script finished
  (silently in a release build, which has no console). `runsql.rs::ScriptSessionGuard` now spawns
  the release onto the runtime, so every exit path is covered; copy that shape for any future
  long-lived session/handle.
- **A driver that pins a `ScriptSession` must also override `ScriptSession::execute_many`.** The
  query editor runs every statement through the pinned session (so `SET`/`BEGIN` survive), and the
  trait's default implementation forwards to `execute`, which may only report a row count. That is
  exactly how PostgreSQL queries ended up showing `受影响行数` and no result tab at all: its
  `execute` drains rows for Run SQL File, while `collect_results` (now backing both
  `execute_query_many` and the session's `execute_many`) builds result-set grids. Keep the two
  paths on one decoder.
- Stored passwords are encrypted with XChaCha20-Poly1305 (`rustgrid-config/src/secrets.rs`);
  `secrets.json` holds ciphertext keyed by profile `id` and `secret.key` holds the key. The
  profile JSON must stay password-free (a test asserts the plaintext never hits disk).
- **Windows needs a linker and a C compiler** (sqlx's `ring`). On this machine the MSVC
  `link.exe` is **not** installed, so always build with the GNU toolchain by prepending
  `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`,
  `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=gcc`, `CC=gcc` (MinGW-w64 `gcc` is on `PATH`).
  A plain `cargo build` fails with `linker link.exe not found`. With the GNU toolchain the
  host *is* the GNU target, so the binary lands at `target/debug/RustGrid.exe` (not under a
  triple-named subdirectory). The gitignored `.cargo/config.toml` also carries this machine's
  settings (see "Commands"): `[build] jobs = 4` caps Cargo's parallelism — one job per core (12
  here) froze the machine — and `[env] GPUI_FXC_PATH` points at the release-build shader shim. A
  command line `-j N` still overrides the cap.
- Keep GPUI usage close to the shapes verified in `main.rs` and the `ui/` wrappers.
  `main.rs` opens the window as `gpui_kit::application().with_assets(Assets).run(|cx| { gpui_kit::init(cx); … })`
  and wraps the app view in `Root::new(view, window, cx)` — `Root` must be the window root or
  `window.open_dialog` panics.
- **gpui-pre API deltas vs the old `gpui 0.2`** (recurring sources of compile errors):
  `Window::focus(&handle, cx)` now takes `cx`; `ScrollHandle::max_offset()` returns
  `Point<Pixels>` (`.x`/`.y`, not `.width`/`.height`); `ShapedLine::paint` takes
  `TextAlign` + `Option<Pixels>`; `BoxShadow` gained an `inset` field; `uniform_list`'s
  `track_scroll` takes `&handle`; `Entity::update` returns `R` directly while
  `WeakEntity::update` returns `Result<R>`.
- **gpui-kit stateful widgets need a `Window` at construction** (`InputState::new`,
  `ComboboxState::new`, `TableState::new` all take `&mut Window`). The app builds its entities
  while there is no window yet, so `ui/text_input.rs` and `ui/combo.rs` create the inner state
  on the first `render` and queue setters/callbacks to that frame. Keep new gpui-kit-backed
  state behind the same lazy wrapper pattern.
- **Completion items must carry an explicit `text_edit` range.** gpui's `CompletionMenu` builds
  the accept range from `trigger_start_offset`, which `gpui-base` pins to the offset *after the
  first keystroke* that opened the menu. Accepting `SELECT` after typing `sele` therefore replaces
  only the tail and yields `seSELECT`. `sql_completion.rs` works around this by setting
  `CompletionTextEdit::Edit` on every item, anchored at `SqlCompletionContext::prefix_start ..
  caret` (the real identifier fragment). Keep that range in sync if the trigger/filter logic
  changes; do not fix it by editing the crates.io copy under `~/.cargo/registry`.
- **Window-edge resizing on Windows is app-owned.** gpui hides the OS titlebar, so its
  `WM_NCCALCSIZE` leaves only a 1px frame and the titlebar's `WindowControlArea::Drag` claims
  the top edge, making the window effectively unresizable. `src/win_resize.rs` installs a
  `SetWindowSubclass` hook that answers `WM_NCHITTEST` with the real frame-metric resize codes
  before gpui sees the message (`install(window)` is called from `open_window`). Do **not** edit
  the crates.io copy of gpui under `~/.cargo/registry` to fix this — it is not reproducible.
- **macOS uses the title bar's native traffic lights.** gpui suppresses the OS titlebar
  (`TitlebarOptions::appears_transparent`) so every window draws its own chrome from the window's
  top-left corner, but AppKit still paints the traffic lights (close/minimise/zoom) over that
  corner. `gpui_kit::component::TitleBar` accounts for them with its 80px left padding and positions
  them at `(9, 9)` through `TitleBar::title_bar_options()`; it draws no right-hand controls on
  macOS. `app_owns_titlebar_drag: true` (set by `ui::window_options` / the main window's options)
  makes the bar move the window through `start_window_move` and own double-click handling, so
  AppKit does not drag the titlebar or delay clicks while disambiguating. The native close button
  reaches `on_window_should_close`, which now holds the main window's unsaved-work confirmation
  (the transfer window keeps its own guard there too).
- **macOS builds the app *inaccessible*, on purpose.** `main.rs::application()` builds the
  `Application` through `gpui::Application::new_inaccessible` on macOS instead of
  `gpui_kit::application()`. GPUI installs `accesskit_macos::SubclassingAdapter` on **every**
  window: it swaps the window content view's Objective-C class at runtime and swaps it back when
  the window is torn down, which corrupts the KVO state AppKit's Touch Bar bookkeeping
  (`_NSTouchBarFinderObservation`, key path `nextResponder`) holds on that view. Closing **any**
  window on a Mac with a Touch Bar (Intel or Apple Silicon) therefore aborts the process with an
  uncaught `NSRangeException` from `-[_NSTouchBarFinderObservation invalidate]` raised during a
  display-cycle flush — no app frames on the stack, which is why 保存并连接 (saving closes the
  connection window) crashed the user in issue #4. Upstream knows (zed-industries/zed#64819,
  longbridge/gpui-kit#3192) and the fix — replacing the subclassing adapter,
  zed-industries/zed#65186 — is **not merged yet**, so no released `gpui-fast` (0.1.12 is newest)
  carries it. Skipping `a11y_init` is the only in-process way to avoid it; the cost is that
  VoiceOver cannot read the app on macOS. **Remove the macOS arm of `application()` as soon as
  `gpui-fast` ships the fix.**
- **The Dock icon is also set at runtime from the embedded logo.** `src/dock_icon.rs` (macOS-only,
  called from the launch callback) pushes `assets/logo.png` into
  `NSApplication.setApplicationIconImage:` through the `objc2` crates, which are already in the lock
  file via `gpui-fast-macos`. A real bundle takes its icon from `AppIcon.icns` (next bullet), so this
  is the fallback that keeps a bundle-less run — an install the updater left bare, or the executable
  copied out of `Contents/MacOS` — from showing the generic executable icon.
- **The macOS release is a `.app` bundle, and the updater swaps it as one.** `.github/workflows/
  release.yml` assembles `RustGrid.app` from `packaging/macos/` (an `Info.plist` template plus
  `make-icns.sh`, which turns `assets/logo.png` into `Contents/Resources/AppIcon.icns` with
  `sips`/`iconutil`) and ad-hoc signs it, so Finder, the menu bar and the Dock read their icon and
  app name from the bundle. `update.rs::configure_macos_layout` mirrors that layout: inside a bundle
  it uses `self_update`'s **bundle mode** (`bundle_path_in_archive("RustGrid.app")`, which swaps the
  whole directory in one rename and defaults its install path to the nearest `.app` ancestor of the
  running exe), and a bare-executable install instead extracts `RustGrid.app/Contents/MacOS/RustGrid`
  via `bin_path_in_archive` so updating from an older release layout still works (it just stays bare).
  The two modes are mutually exclusive in one config (`Error::ConflictingConfig`), so never set both.
  The bundle is **unsigned** (no Developer ID, no notarization), so a quarantined download shows the
  "unidentified developer" prompt, and a quarantined app runs translocated — from a read-only
  randomized mount, where `self_update` fails with `Error::AppTranslocated` naming the fix (move the
  app out of quarantine) rather than corrupting the install.
- **Windows release builds need `fxc.exe` for *both* GPUI copies.** `gpui-pre-windows` (pulled in by
  `gpui-kit`) and `gpui-fast-windows` (the engine the app actually renders with) each have a build
  script that compiles HLSL whenever `debug_assertions` is **off** (release); it is **not** gated by
  a feature, so `runtime_shaders` does not avoid it. On this machine the Windows SDK `fxc.exe` is
  absent, so a plain `cargo build --release` panics with `Failed to find fxc.exe`. The fallback is
  `tools/fxc-shim` (speaks the fxc subset gpui invokes; drives the system `d3dcompiler_47.dll`) built
  once with `gcc -O2 -o fxc.exe tools/fxc-shim/fxc.c -lkernel32`. The shim binary is gitignored, and
  the gitignored `.cargo/config.toml` wires it up automatically (`GPUI_FXC_PATH`, which both build
  scripts read):
  ```toml
  [env]
  GPUI_FXC_PATH = { value = "fxc.exe", relative = true }
  ```
  `relative = true` resolves against the **parent of `.cargo/`** (the repo root), *not* the
  `.cargo/` directory itself — `"../fxc.exe"` would therefore point one level too high and the
  build script silently falls through to the `Failed to find fxc.exe` panic. Debug builds are
  unaffected. The build script reads `GPUI_FXC_PATH` only at compile time and declares no
  `cargo:rerun-if-env-changed`, so after editing the entry run `cargo clean -p gpui-pre-windows
  --release` to force it to be re-read.
- **Cross-platform** (Windows/macOS/Linux). Avoid OS-only APIs; gate platform-specific code
  behind `#[cfg(target_os = ...)]`; watch per-OS native deps (e.g. Linux system libraries).
- **Small install size is a hard requirement.** The release profile sets `lto`, `strip`,
  `opt-level = "z"`, `codegen-units = 1` and `panic = "abort"`. Linux and Windows archives are
  additionally UPX-compressed (`--best --lzma`, UPX 5.3); macOS ships as built because
  UPX-packed Mach-O binaries do not run on macOS 13+. The workflow remaps the
  cargo-registry/workspace path prefixes to keep panic strings short. Prefer small crates;
  enable only the sqlx features actually used.
- UI must mirror Navicat's layout: connection tree on the left, content/details pane on the right.

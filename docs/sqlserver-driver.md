# SQL Server 驱动实现提示词

> 把本文正文直接交给 AI，即可按现有架构实现 SQL Server（Microsoft SQL Server / T-SQL）驱动。
> 目标是复用 RustGrid 已有的驱动抽象，不新造一套 UI。

## 任务

在 RustGrid 中新增 SQL Server（Microsoft SQL Server / T-SQL）驱动。

## 项目背景

仓库：`E:\code\github\rustgrid`，Rust workspace（edition 2024，`rust-version = "1.94"`），GUI 用 gpui-kit。
这是一个 Navicat 风格的数据库管理工具。目前已接入三类引擎：

- `crates/rustgrid-mysql`：MySQL + MariaDB（同一套 sqlx MySQL 协议实现，用 `Engine` 枚举区分方言）。
- `crates/rustgrid-sqlite`：SQLite（文件型，基于 sqlx 的 `sqlite` feature）。
- 核心抽象在 `crates/rustgrid-core`：`Driver` / `Connection` trait、模型（`CellValue`、`TablePage`、`TableSchema`、`ObjectDump`、`UserEdit`、`RoutineEdit`、`ViewEdit` 等）、`DriverRegistry`。
- 应用层 `crates/rustgrid-app` 只依赖 `rustgrid_core`，通过 `DriverRegistry` 注册驱动。

请先通读：

- `AGENTS.md`（尤其是 Repository layout / Extensibility requirements / UI conventions / Gotchas）。
- `crates/rustgrid-core/src/driver.rs`（要实现的 trait）。
- `crates/rustgrid-mysql/src/{driver,connection}.rs`（最完整的参考实现）。
- `crates/rustgrid-sqlite/src/{driver,connection}.rs`（新驱动的结构与测试写法）。

## 关键约束（务必先确认）

1. **sqlx 0.9 不支持 MSSQL**（0.7 之后官方移除了 mssql 驱动，等待重写）。因此**不要**给 workspace 的 `sqlx` 依赖加 `mssql` feature（不存在）。
2. SQL Server 驱动应基于 **`tiberius`**（纯 Rust 的 TDS 客户端，async），配 `tokio-util`（`compat`，把 tiberius 的 futures-io 适配到 tokio）、`futures-util`、`async-trait`。
   - 连接池二选一：`bb8` + `bb8-tiberius`，或 `deadpool-tiberius`，或先用 `Arc<tokio::sync::Mutex<tiberius::Client<Compat<TcpStream>>>>` 简化。
   - 具体 crate 版本以 crates.io 当前稳定版为准（不要臆造版本号），并遵守项目“依赖成熟、体积小”的策略。
3. 本项目默认用 rustls（`sqlx` 用 `tls-rustls-ring`），tiberius 请启用其 rustls 相关 feature，避免引入 OpenSSL。
4. `Connection` trait 是 `Send + Sync`，方法签名是 `&self`。tiberius 的 `Client` 不是 `Sync`，必须用连接池或 `Mutex` 之类的内部可变性包起来。
5. Windows 上编译要用 GNU 工具链环境变量：
   ```powershell
   $env:RUSTUP_TOOLCHAIN='stable-x86_64-pc-windows-gnu'
   $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER='gcc'
   $env:CC='gcc'
   ```
   仓库根的 `.cargo/config.toml` 已把 `build.jobs` 限到 2（本机设置，已 gitignore）。

## 交付物

1. 新 crate `crates/rustgrid-sqlserver`，加入根 `Cargo.toml` 的 `[workspace] members` 和 `[workspace.dependencies]`；实现 `Driver` 与 `Connection`（参考 `rustgrid-mysql`）。
   - `SqlServerDriver`：`id() = "sqlserver"`、`display_name() = "SQL Server"`、`default_port() = 1433`、`is_file_based() = false`。
   - 能力标志（core 的 `Driver` 已有）：`supports_database_management/users/routines` 均返回 `true`（SQL Server 有数据库、登录/用户、存储过程）。
2. `crates/rustgrid-app/src/main.rs`：`registry.register(Arc::new(rustgrid_sqlserver::SqlServerDriver::new()));`
3. 图标：新增 `crates/rustgrid-app/assets/icons/sqlserver.svg`，在 `assets.rs` 注册，并在 `app/mod.rs` 的 `driver_icon` 里加 `"sqlserver" => "icons/sqlserver.svg"`（决定是否走白字徽标，参照 mysql/mariadb 的 `tree_driver_icon`）。
4. `CONNECTION_TYPES`（`app/toolbar.rs`）里已有 `("sqlserver", "SQL Server...")`，注册后会自动变为可选，无需改。
5. 连接表单无需改结构（SQL Server 是网络引擎，`is_file_based = false`，用 host/port/user/password 那套）。默认端口从 `Driver::default_port()` 取，已是 1433。
6. 测试：
   - `crates/rustgrid-sqlserver/tests/live_sqlserver.rs`（默认 `#[ignore]`，靠环境变量 `RUSTGRID_SQLSERVER_HOST/PORT/USER/PASSWORD/DATABASE` 连接）。
   - 纯函数单元测试（标识符引用、分页 SQL、DDL 生成、过滤树翻译），风格参照 `rustgrid-sqlite` 的测试。
7. 更新 `AGENTS.md`：把 SQL Server 加入引擎列表、依赖说明与 “out of scope” 表述。

## `Connection` trait 实现要点（逐项）

T-SQL 方言、`[]` 标识符引用、`@P1` 参数占位、`sys.*` / `INFORMATION_SCHEMA.*` 编目：

- `list_databases`：`SELECT name FROM sys.databases`（按权限过滤可选）。
- `list_tables`：`INFORMATION_SCHEMA.TABLES`（TABLE / VIEW）。
- `columns`：`INFORMATION_SCHEMA.COLUMNS`（含可空、类型、`COLUMNPROPERTY(...,'IsIdentity')` 判自增；主键用 `sys.indexes` / `sys.index_columns` 或 `KEY_COLUMN_USAGE`）。
- `fetch_page`：`SELECT * FROM [db].[schema].[table] {WHERE} ORDER BY ... OFFSET n ROWS FETCH NEXT m ROWS ONLY`。
  - ⚠️ `OFFSET/FETCH` 必须有 `ORDER BY`；无排序时用主键，仍无则用 `ORDER BY (SELECT NULL)`。
- `update_rows` / `insert_rows` / `delete_rows`：主键优先、无主键退化为全列；`NULL` 键用 `IS NULL`，绝不要把 `NULL` 绑定成空串。用 `@P1..` 参数化，事务内执行。
- `execute_query` / `execute_query_many`：用 tiberius 的批处理（`simple_query` 或 `query`）。
  - 列元数据要从结果流里的 `Metadata`（`ColumnData`）取——**零行结果集也要能拿到列**（应用里查询网格依赖这一点）。
  - 多语句脚本要返回每个结果集一条 `QueryResult`，并带上各自的语句文本；切分脚本时注意 T-SQL `BEGIN...END` / `CASE...END`；无法切分时退化为整段。
- 数据库管理：`CREATE DATABASE` / `ALTER DATABASE ... COLLATE` / `DROP DATABASE`；`database_defaults` 读 `sys.databases.collation_name`；`create_database_sql` / `alter_database_sql` 返回预览脚本。
- 表操作：`DROP TABLE`、`DELETE FROM`（Empty）、`TRUNCATE TABLE`、重命名用 `sp_rename`。
- `server_version`：`SELECT @@VERSION` 或 `SERVERPROPERTY('ProductVersion')`。
- `session_count`：`SELECT COUNT(*) FROM sys.dm_exec_sessions`。
- `table_status` / `table_statuses`：`sys.tables` + `sys.partitions` / `sys.dm_db_partition_stats` 取行数与数据长度。
- `character_sets` / `collations`：`sys.fn_helpcollations()` 或 `INFORMATION_SCHEMA.CHARACTER_SETS`（可返回常用子集）。
- `column_types`：SQL Server 真实类型（`int` / `bigint` / `smallint` / `tinyint` / `bit` / `decimal` / `numeric` / `money` / `float` / `real` / `date` / `time` / `datetime` / `datetime2` / `datetimeoffset` / `char` / `varchar` / `nchar` / `nvarchar` / `text` / `ntext` / `binary` / `varbinary` / `uniqueidentifier` / `sql_variant` / `xml` 等）。
- 视图：`sys.views` + `OBJECT_DEFINITION(OBJECT_ID(...))`（`view_details` / `view_sql` / `save_view` / `drop_view`）。
- 存储过程 / 函数：`INFORMATION_SCHEMA.ROUTINES` + `sys.sql_modules.definition`（`list_routine_infos` / `routine_details` / `routine_sql` / `save_routine` / `drop_routine`）。SQL Server 没有“事件”，`list_events` 返回空。
- 备份：`rustgrid-backup` 是“每对象 DDL + 逐行元组”的引擎无关容器。
  - 表 DDL 可用 `sys` / `INFORMATION_SCHEMA` 手工拼 `CREATE TABLE`（SQL Server 没有 MySQL 的 `SHOW CREATE`，也没有 SQLite 的 `sqlite_master.sql`）。
  - 行数据用 `stream_table_rows` 流式 `SELECT`，元组按 T-SQL 字面量渲染（`N'...'`、`0x...`、`NULL`、`CAST(... AS datetime2)` 等）。
  - `restore_object` 回放 DDL + INSERT，必要时 `SET IDENTITY_INSERT ON/OFF`。
- 用户 / 权限：`sys.server_principals` / `sys.database_principals`、`CREATE LOGIN` / `CREATE USER`、`ALTER ROLE ... ADD MEMBER`、`GRANT/DENY/REVOKE`。这块最复杂，可放到最后阶段，先让默认实现报“不支持”或返回空。
- `close`：优雅关闭池 / 连接。

## 连接与 TLS

- 解析 `ConnectionConfig`：host、port(1433)、username、password、database、`settings.tls.mode`、`settings.connect_timeout`、`settings.read_only`（可用 `ApplicationIntent=ReadOnly`）、`settings.init_sql`（连接后执行）。
- tiberius `Config`：`host`、`port`、`database`、`authentication(AuthMethod::sql_server(user, pass))`、`trust_cert()` 仅用于自测；TLS 映射到 `EncryptionLevel`（Disabled→Off/NotSupported，Preferred→On，Required/VerifyCa/VerifyIdentity→Required；证书校验按 tiberius 能力处理，文档说明限制）。
- 支持命名实例 / 端口：至少支持 `host,port`；`instance_name` 可作为增强。
- 认证失败 / 网络错误要映射到 `rustgrid_core::Error::Authentication` / `Error::Connection`（参考 `rustgrid-mysql::map_connect_error`）。

## 分阶段（建议）

1. **只读浏览**：连接、`list_databases` / `list_tables` / `columns` / `fetch_page` / `execute_query`（多语句）/ `server_version` + 驱动注册与图标。先把“能连、能看表、能跑查询”打通。
2. **编辑**：`update` / `insert` / `delete_rows`、表 DDL（drop/empty/truncate/rename）、`table_schema` / `table_schema_sql`、视图详情 / 保存 / 删除。
3. **完整对齐**：存储过程、用户 / 权限、备份导出 / 还原、Options 页（schema / collation）。

## 验收标准

- `cargo build`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings` 全部通过（当前仓库是零告警，不要引入新告警）。
- 不影响 MySQL / MariaDB / SQLite 现有功能与测试。
- 连接表单：新建连接菜单里 SQL Server 可选、默认端口 1433、能测试连接与保存连接。
- 真机（或 Docker `mcr.microsoft.com/mssql/server`）跑通：列出库 / 表 / 列 → 分页取数 → 增删改 → 执行任意 SQL → 表设计器读改 → 视图列表 / 打开。
- 新增的 live 测试默认 `#[ignore]`，靠 `RUSTGRID_SQLSERVER_*` 环境变量启用，并写进 `AGENTS.md` 的命令一节。

## 代码风格 / 注意事项

- 引擎相关代码只在驱动 crate 内；UI 层不出现 `tiberius` 类型。
- 标识符一律 `[name]` 并转义 `]`（`]]`）；值一律参数化。
- 类型映射到 `CellValue`（`Int` / `Uint` / `Float` / `Text` / `Bytes` / `Null` / `Bool`）；`uniqueidentifier`、`datetime2`、`money`、`xml`、`sql_variant` 要有合理降级。
- 会话 / 批处理注意 `SET NOCOUNT`、`SET QUOTED_IDENTIFIER`、隔离级别；DDL 与 `CREATE PROCEDURE` 必须整批发送（不能中途断开）。
- 不要修改 `~/.cargo/registry` 下的 crate 源码；需要用上游能力时在项目内包装。
- 不确定的 tiberius / sqlx API，先查本地 `~/.cargo/registry` 源码或官方文档，不要臆造方法名。

先用 `cargo add` / workspace 依赖确认 tiberius 的确切 API 与版本，再动手；每一步用 `cargo check -p rustgrid-sqlserver`（jobs 已限 2）迭代，最后再跑全量 build / test / clippy。

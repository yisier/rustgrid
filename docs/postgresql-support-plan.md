# PostgreSQL 支持计划

> 本文是 RustGrid 新增 **PostgreSQL**（原生 sqlx 驱动）的实施方案与**详细任务清单**。  
> 目标：在**不新造 UI** 的前提下，复用 `rustgrid-core` 的 `Driver` / `Connection` 抽象，  
> 交付一个与 MySQL / MariaDB / SQLite / SQL Server 同级的原生引擎。
>
> 参考实现：`crates/rustgrid-mysql`（最完整的 sqlx 实现）、`crates/rustgrid-sqlserver`  
> （**schema 一等公民**与非 sqlx 驱动的参考）、`crates/rustgrid-sqlite`（crate 结构与测试写法）。  
> 相关文档：`docs/sqlserver-driver.md`、`docs/odbc-driver.md`。

---

## 1. 目标与范围

### 1.1 交付目标

| 能力  | 目标                                                                                            |
| --- | --------------------------------------------------------------------------------------------- |
| 连接  | host/port(5432)/user/password/database，TLS（rustls）、SSH/SOCKS5/HTTP 隧道、连接/查询超时、只读会话、`init_sql` |
| 浏览  | 数据库列表、**schema 列表**、表 / 视图列表、列信息、分页取数                                                         |
| 编辑  | 网格内增 / 删 / 改（主键优先，无主键退化为全列）、任意 SQL、多语句脚本                                                      |
| 库管理 | 建库（ENCODING / LC_COLLATE / OWNER）、改库（OWNER）、删库                                                |
| 表操作 | Drop / Empty / Truncate / Rename、表设计器（列 / 索引 / 外键 / 触发器）读写                                    |
| 视图  | 列表 / 详情 / 保存 / 删除                                                                             |
| 例程  | 函数与存储过程（PG 11+）列表 / 详情 / 保存 / 删除                                                              |
| 用户  | 角色列表 / 详情 / 保存 / 删除 / 重命名、对象级权限                                                               |
| 备份  | `.rgbak` 备份 / 还原（表 + 视图 + 例程）                                                                 |
| 导出  | `.xlsx` / `.csv` / `.txt` 已引擎无关；`.sql` 需新增 Postgres 方言（见 P6-T4）                              |

### 1.2 明确不做（沿用现有 out-of-scope）

- 不引入 `PostgreSQL` 专属的新 UI 页面或新 trait 方法（除非抽象确实缺失，见 §10）。
- 不实现逻辑复制、扩展管理、分区维护、物化视图设计器等长尾功能（列表可见即可，设计器不做）。
- 不修改 `~/.cargo/registry` 下的任何上游 crate 源码。
- 不破坏 MySQL / MariaDB / SQLite / SQL Server 的现有功能与测试。

---

## 2. 现状评估：为什么现在做

抽象层已经就绪，PostgreSQL 是**第一个"需要连接池按库分片"**&#x7684;引擎，但其余都能复用：

1. **菜单占位已存在。** `crates/rustgrid-app/src/app/toolbar.rs` 的 `PLANNED_ENGINES`  
   已把 `("postgresql", "PostgreSQL", 20)` 列为灰显项。  
   *注意*：`toolbar.rs:321-325` 遍历占位时有 `if self.registry.get(&driver).is_none()`，  
   **驱动注册后占位会自动跳过**，不会出现两个 PostgreSQL 条目。删除它只是清理（建议顺手删）。
2. **schema 机制已通用。** `crates/rustgrid-app/src/app/{tree,objects,sidebar,query}.rs`  
   全部通过 `DriverCapability::Schemas` 判断，不匹配引擎 id。PostgreSQL 设  
   `supports_schemas = true` 即自动获得 **数据库 → schema → 表/视图/函数** 的树结构，  
   以及 `schema_of` / `object_display`（`objects.rs:71-84`，按首个 `.` 拆分 `schema.name`）的名字处理。
3. **sqlx 已具备 Postgres 后端。** `Cargo.lock` 中已解析到 `sqlx-postgres 0.9.0`（crate 源码也已在
   本地 registry 缓存里，无需额外下载）；workspace 的 `sqlx` 依赖是 `default-features = false`，
   `rustgrid-sqlite` 已示范按 crate 追加 feature（`sqlx = { workspace = true, features = ["sqlite"] }`）。
   PostgreSQL 的基准是 `features = ["postgres"]`（是否再追加类型 feature 见 §3.8）。TLS 走仓库已开的
   `tls-rustls-ring`，**不新增原生编译依赖**——注意 rustls 的 `ring` 本身要 C 编译器（本仓库的 MySQL
   已在用，所以并非"PG 不需要 C 编译器"，只是不额外增加）。
4. **方言/图标/顺序都是数据驱动。** `DriverDescriptor`（`order` / `icon` / `icon_style` /  
   `capabilities`）、`DriverDialect`、`Assets::load` 均为声明式，新增引擎只改数据。
5. **两处方言硬编码需要一并处理。** `app/src/sql.rs` 有 4 处硬编码 `MySqlDialect`（行 34/79/135/254），  
   `app/src/app/export.rs:769` 硬编码 `SqlDialect::MySql`。前者影响语法高亮 / `view_select`（EXPLAIN）/  
   `infer_single_table`（**决定结果网格能否就地编辑**），后者让导出的 `.sql` 变成反引号的 MySQL 语法。  
   见 P6-T4 / P6-T5。

### 2.1 与现有引擎的关键差异（本计划的难点来源）

| 差异                                          | 影响                                                    | 应对                                                                                                |
| ------------------------------------------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| **Postgres 不能跨库查询**                         | 一个连接只属于一个数据库；`list_databases` 能列全部，但访问其它库的表必须**另开连接** | `PostgresConnection` 维护 **`HashMap<database, PgPool>`**，按需懒建池（见 §3.1）                             |
| **强类型：text→int 是显式转换**                     | core 传来的值全是 `String`，按 `String` 绑定会报类型不匹配                 | **生成 SQL 时按列的真实类型加 `CAST($n AS <type>)`**（见 §3.7）                                                  |
| 无 `SHOW CREATE TABLE` / `SHOW CREATE VIEW`  | 表 DDL、`object_ddl`、备份的表 DDL 必须**从 `pg_catalog` 拼**    | 复用 SQL Server 的思路，用 `pg_attribute` / `pg_constraint` / `pg_index` / `pg_attrdef` 拼 `CREATE TABLE`；参考实现是 `rustgrid-sqlserver/src/helpers.rs` |
| 标识符是 `"双引号"`、占位符是 `$1`                      | 所有 SQL 生成与过滤翻译都要换方言                                   | 独立 `helpers.rs`，与 `rustgrid-sqlserver::helpers` 的 `quote_identifier` / `filter_clause` 对应           |
| 分页是 `LIMIT n OFFSET m`（无需 `ORDER BY`）       | 但分页要稳定必须排序                                            | 无排序时按主键排，无主键的基表/分区表/物化视图用 `ORDER BY ctid`（视图/外部表无 `ctid`，见 P1-T7）                                                             |
| **PL/pgSQL 函数体用 `$$ ... $$` 美元引用**，内含分号     | 多语句脚本切分会被函数体里的分号骗到                                    | **交给服务端切分**（§3.4）；客户端切分只用于填 `statement` 与零行 `prepare`                                           |
| 函数重载：同名不同参数签名                               | `drop_routine(name)` / `routine_details(name)` 需要参数类型 | 用 `pg_get_function_identity_arguments` 把签名编进 routine 的 `name`（见 §3.5）                             |
| 角色模型与 MySQL 不同（无 `user@host`，统一 `pg_roles`） | `UserAccount.host` 无意义；`Privilege` 枚举是 MySQL 形状       | host 置空（注意 `UserAccount::label()` 的 `user@host` 渲染）；权限按子集映射（见 P4-T4 / R3）                               |
| 无存储引擎、无 charset（改为 encoding/locale）、无事件调度器  | `storage_engines` / `list_events` 返回空；库对话框字段语义变化      | `DatabaseEditorSpec` 复用 `charset`→`ENCODING`、`collation`→`LC_COLLATE`、`owner`=true（见 P2-T5 / P2-T6）        |
| 隧道代码在 `rustgrid-mysql::tunnel`（crate 私有）    | Postgres 想复用 SSH/SOCKS5/HTTP 隧道                       | 抽成共享 crate `rustgrid-tunnel`（见 §3.6）                                                              |
| `information_schema.*` 只暴露当前用户有权限的对象        | 非超级用户下 `list_tables` 会漏表                              | 目录查询一律走 `pg_class` / `pg_namespace` / `pg_proc`（权限视图仅用于 `is_updatable` 这类补充属性）                    |
| **结果格式由协议决定**：`sqlx::query`（带绑定）走扩展协议→二进制值 | MySQL 式"unchecked 字节转 UTF-8"兜底取不到 `numeric`/`uuid`/数组等的文本 | 读路径一律把列 `::text` 投影（或按类型解码），见 §3.8                                |
| app 层的例程/视图名解析是 MySQL 形状（`sql::routine_identity` / `view_identity` / `routine_call_sql` / `routine_template`） | PG 的 `schema.name` 限定定义会让它返回 schema；带签名/反引号的名字会拼出非法 SQL | P6-T5a：让这几个 helper 按 `DriverDialect` 产出方言 SQL，见 §3.5 |

---

## 3. 关键架构决策

> 以下决策请**在动手前确认**，它们决定 crate 的内部结构。

### 3.1 决策 A：按数据库分片的连接池（必须）

Postgres 的 `database` 不是 `USE db` 能切换的——它是连接级、不可变的。因此：

```rust
pub struct PostgresConnection {
    /// 建池模板：host/port/user/password/TLS/超时/init_sql 都烘进 options。
    options: PgConnectOptions,
    /// 连接 profile 里指定的默认库（空则回退 `postgres`）。
    default_database: String,
    /// 按库懒建的池；`tokio::sync::Mutex` 保证并发下只建一次。
    pools: Arc<Mutex<HashMap<String, PgPool>>>,
    /// 隧道句柄，随连接存活（`None` 表示直连）。
    _tunnel: Option<Tunnel>,
}

impl PostgresConnection {
    /// 解析目标库 → 取或建池。所有 trait 方法都先经过它。
    async fn pool(&self, database: Option<&str>) -> Result<PgPool> { /* ... */ }
}
```

- 默认库（`pool(None)`）在 `connect()` 里就建好并**做一次探活**，让错误密码在连接时即暴露  
  （与 SQL Server 的 `min_idle(1)` 等价意图）。
- 每个池 `max_connections = 5`，与现有驱动一致。
- **连接数放大必须有兜底**：PG 默认 `max_connections = 100`，而树里展开 N 个库就会建 N 个池。  
  非默认库（按需建出来的池）用 `max_connections(2)` + 短 `idle_timeout`（如 60s），并在池数超过
  上限时 LRU 驱逐；默认库保持 5。见风险 R1。
- `close()` 遍历关闭所有池。

### 3.2 决策 B：对象名统一 `schema.name` 限定

- `list_tables` / `list_routines` / `list_schemas` 的返回值**必须 schema 限定**（`public.users`），  
  与 SQL Server 完全一致；app 的 `objects.rs` 依赖这个约定做 schema 分组。
- 驱动内部把限定名解析回 `(schema, name)`——对应 SQL Server 的 `resolve_object`，  
  放到 `helpers.rs::resolve_object(database, table) -> (String, String)`，未带前缀时回退  
  `search_path` 的第一个用户 schema（通常是 `public`；可查一次 `SHOW search_path` 再缓存）。
- 生成 SQL 时一律 `"schema"."table"` 全限定，避免依赖会话 `search_path`。
- **例程名带签名**（§3.5）后形如 `public.get_user(integer)`；`objects.rs` 的 `schema_of`
  按首个 `.` 拆分，只要参数类型本身不带 `.`（即不写成 `public.my_type`）就安全。
  `list_routines`（备份树用）与 `list_routine_infos`（Functions 页用）**必须返回同样的名字**，
  否则两处对不上。

### 3.3 决策 C：系统 schema 默认隐藏

`pg_catalog` / `information_schema` / `pg_toast*` / `pg_temp*` 不进树、不进列表：

```sql
SELECT nspname FROM pg_namespace
WHERE nspname NOT LIKE 'pg\_%' AND nspname <> 'information_schema'
ORDER BY nspname
```

（可选增强：profile 的 `visible_databases` 已有类似机制，schema 过滤留到 Phase 6。）

### 3.4 决策 D：多语句执行交给服务端（不客户端切分）

`execute_query_many` **不要自己切分脚本**。sqlx-postgres 的 `fetch_many` 在 `raw_sql` 下走
**简单查询协议**，每个 `CommandComplete` 都会 yield 一个 `Either::Left(PgQueryResult)`
（`sqlx-postgres-0.9.0/src/connection/executor.rs:284` / `314-322`）——与 MySQL
`connection.rs:418-429` 的做法完全一致：

```rust
let mut connection = pool.acquire().await?;   // 整段脚本共用一条连接
let mut stream = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string())).fetch_many(&mut *connection);
while let Some(item) = stream.try_next().await? {
    match item {
        Either::Left(done) => { /* 语句结束 → push QueryResult(rows_affected) */ }
        Either::Right(row) => { /* 累积当前结果集的行 */ }
    }
}
```

好处：

- `$$ … $$` / `$tag$ … $tag$`、`E'…'`、注释、`BEGIN … END`、事务脚本全部由**服务端**解析，不会误切；
- 整段脚本跑在**同一条连接**上，`BEGIN/COMMIT`、`SET LOCAL`、`CREATE TEMP TABLE` 语义正确；
- 省掉一个高风险的自研切分器（原风险 R5 的根因）。

硬约束：**整段脚本必须 acquire 一条 `PoolConnection`**，绝不能每条语句各自从池里取连接，
否则事务 / `SET LOCAL` / 临时表全部失效。

客户端切分仍然需要，但**只用于两件事**：

1. 填 `QueryResult.statement`（每条语句自己的文本，供结果网格 reload 与 `infer_single_table`）。  
   MySQL 的 `execute_query_many` 把它留空；PG 填上反而是增强，照填。
2. 零行 `SELECT` 的列元数据兜底——对该条单独 `pool.prepare(stmt)` 取列
   （`Executor::prepare` 在 `&Pool` 上有实现，且**不受 feature 门控**）。

切分器（`helpers::split_statements`）只需识别：`'...'`（`''` 转义）、`"..."`（`""` 转义）、
`$$ … $$` 与 `$tag$ … $tag$` 美元引用、`--` 行注释、`/* */` 块注释（可嵌套）。

> ⚠️ **不要把 `BEGIN … END` 当 PL/pgSQL 块。** PG 里裸 `BEGIN;` 是**开启事务**，必须按分号切；  
> PL/pgSQL 的 `BEGIN … END` 永远位于 `$$ … $$` 或 `DO $$ … $$` 内部，已被美元引用规则覆盖。  
> 若照 SQL Server 那样等 `END`，`BEGIN; INSERT …; COMMIT;` 会被粘成一整坨。  
> `CASE … END` 同理不需要特殊处理（其内部不会出现分号）。

- 切分失败（不平衡）时**整段发送**，与 SQL Server 的兜底一致。

### 3.5 决策 E：例程标识带参数签名

Postgres 函数按 `(name, argtypes)` 唯一。core 的 `RoutineInfo.name` 是 `String`，  
因此驱动把**schema 限定 + 签名**编进名字：`public.get_user(integer)`。实现：

- `list_routine_infos` 返回 `format!("{}.{}({})", nspname, proname, pg_get_function_identity_arguments(oid))`；
- `list_routines`（备份树）返回**同样**的名字；
- `routine_details` / `drop_routine` / `save_routine` 从该字符串反解 `(schema, name, argtypes)`；
- `pg_get_functiondef(oid)` 返回的已是完整 `CREATE OR REPLACE FUNCTION`，直接用于  
  `RoutineDetails.definition` 与保存回放。
- `RoutineInfo.kind` 由 `pg_proc.prokind`（`f`=函数，`p`=过程）映射。

> ⚠️ 这是与 core 模型契合度最低的一处，且**与 app 层现有实现正面冲突**：
> `sql::routine_identity` 取的是 `FUNCTION`/`PROCEDURE` 后第一个 Word（会拿到 schema），
> `routine_call_sql` 会把 `(integer)` 当函数名、并用反引号拼 SQL。这些点的落地清单见
> **P6-T5a**；只有在确需时才给 `RoutineInfo` 增加可选 `signature` 字段（**最后手段，优先不改 core**）。

### 3.6 决策 F：隧道抽成共享 crate

`crates/rustgrid-mysql/src/tunnel.rs`（375 行，SSH/SOCKS5/HTTP，基于 `russh`）目前是  
MySQL crate 私有。PostgreSQL 要同样的隧道能力，方案二选一：

- **推荐**：新建 `crates/rustgrid-tunnel`，把 `tunnel.rs` 整体迁入并公开，  
  `rustgrid-mysql` 改为 `pub use rustgrid_tunnel::Tunnel;` 保持兼容。改动机械、可独立测试。  
  *配套*：`rustgrid-mysql/Cargo.toml` 用到隧道的只有 `russh` + `base64` 两个依赖
  （**没有 `sha1`**，`tunnel.rs` 也不引用它），把它们迁到新 crate；新 crate 另需
  `rustgrid-core` / `tokio`。并在根 `Cargo.toml` 的 `[workspace.dependencies]` 加 `rustgrid-tunnel`。
- 兜底：在 `rustgrid-postgresql` 内复制一份（**不推荐**，会造成两份漂移）。

Phase 0 先做直连，Phase 6 落地隧道（见任务 P0-T6 / P6-T1）。

### 3.7 决策 G：绑定参数必须带类型（**PG 特有，最容易踩**）

Postgres 是强类型引擎，`text → int` 属于**显式**转换（就是 PgJDBC 的
`stringtype=unspecified` 那个坑）。而 core 交给驱动的值**全都是字符串**：

- `RowUpdate.set` / `RowUpdate.keys`：`Vec<(String, Option<String>)>`
- `RowInsert.values`：`Vec<(String, Option<String>)>`
- `delete_rows` 的 `keys`、`FilterCondition` 的 `value`：同样是 `Option<String>`

sqlx 把 `String` 绑成 `PgTypeInfo::TEXT`（OID 25，`sqlx-postgres-0.9.0/src/types/str.rs:12`），
于是会直接报错：

```sql
UPDATE "public"."t" SET "n" = $1   -- ERROR: column "n" is of type integer but expression is of type text
WHERE "n" = $1                     -- ERROR: operator does not exist: integer = text
```

MySQL 有隐式转换、tiberius 发 NVARCHAR 也能隐式转，所以现有驱动**从未暴露**这个问题——
**PostgreSQL 是第一个**。两个方案：

- **A（推荐）生成 SQL 时加 `CAST`**：`SET "n" = CAST($1 AS integer)`、
  `WHERE "n" = CAST($2 AS integer)`。类型串直接取 `columns()` 的 `format_type`
  （`integer` / `character varying(50)` / `numeric(10,2)` / `timestamp with time zone`…），
  每条语句前查一次列元数据即可，**不需要改 core**。
- **B 发 unspecified 类型**：自定义 newtype 实现 `Type<Postgres>`，返回
  `PgTypeInfo::with_oid(Oid(0))`（`sqlx::postgres::types::Oid` 已公开导出），让 PG 自己推断。
  更优雅，但要自己实现 `Encode`，且与 `try_get` 的 checked 解码路径混用时容易出错。

`None` 仍然走 `IS NULL`，**绝不绑定空串**（与现有驱动一致）。

### 3.8 决策 H：结果解码必须区分协议（**PG 特有，第二个最容易踩**）

Postgres 的结果值格式由**这一条语句是否带绑定参数**决定，而不是由 SQL 决定：

- `sqlx::query(...)` 即使一次也不 `.bind()`，也带着 `arguments: Some(Default)`（`sqlx-core/src/query.rs`
  的 `query()` 构造），于是走**扩展协议 → 二进制格式**；
- 只有 `sqlx::raw_sql(...)` 的 `take_arguments()` 返回 `None`（`sqlx-core/src/raw_sql.rs`），
  才走**简单查询 → 文本格式**。

所以 §7 里"先 checked `try_get`，再 `try_get_unchecked` 取文本/字节 + UTF-8"的 MySQL 式兜底
**在二进制路径上不成立**：`Vec<u8>` 的 `Decode` 在二进制下返回原始字节、在文本下还要求 BYTEA 的
`\x` 前缀（`sqlx-postgres-0.9.0/src/types/bytes.rs:81`），`as_str()` 只做 UTF-8 校验
（`src/value.rs:71`），因此类型映射表里映射为 `Text` 的 `numeric` / `json(b)` / `uuid` / `inet` /
`cidr` / `macaddr` / `interval` / 数组 / `money` 会得到乱码或直接报错；`PgNumeric` 还是
`pub(crate)`（`src/types/numeric.rs:9`），从外部拿不到。

`fetch_page` 要用绑定值翻译过滤树，**必然走二进制**，所以必须显式选一条路：

- **A（推荐）投影成文本**：用 `columns()` 拿到列，把 `SELECT *` 写成
  `SELECT "n"::text AS "n", "d"::text AS "d", ...`，全部按 TEXT 读——二进制下 TEXT 的字节就是文本，
  `try_get_unchecked::<String>` 即可，**不新增 sqlx feature**。同一策略用于 `stream_table_rows`
  的字面量渲染（P5-T2）与任意 SQL 的结果网格。
- **B 按类型解码**：给 sqlx 追加 `bigdecimal`（`numeric` 的精确串）、`json`、`uuid`、`ipnet` 等
  feature，再用各自类型的 `Display` 格式化成字符串。依赖更多，且要保证与 checked 解码路径一致。

`execute_query` / `execute_query_many`（`raw_sql`）是文本格式，`numeric` / `jsonb` 天然就是文本，
可直接按 `String` 解；但 `decode_cell` 被两条路径共用，实现时要按 `PgValueFormat` 分别处理，
**不能假设"字节就是文本"**。

`date` / `time` / `timestamp` / `timestamptz` 用 `chrono`（feature 已开）解码后格式化。

---

## 4. 交付物清单（文件级）

```
crates/rustgrid-postgresql/
├── Cargo.toml                     # 新增 crate（含 [dev-dependencies] tokio）
├── src/
│   ├── lib.rs                     # pub use driver::PostgresDriver; pub use connection::PostgresConnection;
│   ├── driver.rs                  # PostgresDriver: id/descriptor/connect/TLS/池
│   ├── connection.rs              # impl Connection for PostgresConnection（分片池 + 全部方法）
│   ├── helpers.rs                 # 引用/占位/过滤树翻译/脚本切分/decode_cell/DDL 拼装 + #[cfg(test)] 单测
│   ├── schema.rs                  # 表设计器：catalog 读写 + CREATE/ALTER DDL 生成
│   ├── routine.rs                 # 例程（函数/过程）目录与 pg_get_functiondef
│   ├── view.rs                    # 视图目录与 pg_get_viewdef
│   ├── user.rs                    # 角色/权限目录与 GRANT/REVOKE 生成
│   └── backup.rs                  # ObjectDump 组装 + 行元组字面量渲染 + restore
└── tests/
    └── live_postgresql.rs         # #[ignore] 真机集成测试，靠 RUSTGRID_PG_* 环境变量

crates/rustgrid-app/assets/icons/postgresql.svg     # 品牌图标（大象，Brand(0x336791)）

修改：
- Cargo.toml                                        # members + workspace.dependencies
- crates/rustgrid-app/Cargo.toml                     # 依赖 rustgrid-postgresql
- crates/rustgrid-app/src/main.rs                    # BuiltinDriverSource 注册 PostgresDriver
- crates/rustgrid-app/src/assets.rs                  # 注册 icons/postgresql.svg
- crates/rustgrid-app/src/app/toolbar.rs             # 顺手删除 PLANNED_ENGINES 的 postgresql 占位
- crates/rustgrid-core/src/dialect.rs                # 新增 DriverDialect::Postgres + 函数表
- crates/rustgrid-export/src/lib.rs                  # SqlDialect 新增 Postgres 变体（P6-T4）
- crates/rustgrid-app/src/sql.rs                     # 按 DriverDialect 选 sqlparser dialect（P6-T5）；语句扫描器认美元引用（P6-T5b）
- crates/rustgrid-app/src/app/routine.rs             # 例程模板/调用 SQL 按方言生成（P6-T5a）
- crates/rustgrid-app/src/app/view.rs                # 视图名解析（P6-T5a，配合 sql.rs）
- crates/rustgrid-app/src/app/export.rs              # 按驱动选 SqlDialect（P6-T4）
- AGENTS.md / README.md / README_zh.md               # 引擎列表、命令、out-of-scope
- docs/postgresql-driver.md                          # 交付后补一份"实现说明"（可选，仿 odbc-driver.md）
```

> *注*：① 纯函数单测按仓库惯例放在**各个 `src/*.rs` 的 `#[cfg(test)] mod tests`** 里
> （参考 `rustgrid-sqlserver/src/helpers.rs:528`、`rustgrid-mysql/src/{routine,user,view}.rs`），
> **不要放 `tests/`**——`tests/` 下每个文件是独立二进制，链接不到 crate 私有的 `helpers` 模块。
> ② `crates/rustgrid-core/src/lib.rs` **无需改动**：`pub use dialect::DriverDialect` 已存在，
> 新增枚举变体自动可用。

---

## 5. 分阶段总览

| 阶段     | 主题       | 产出                                    | 依赖    |
| ------ | -------- | ------------------------------------- | ----- |
| **P0** | 脚手架与骨架   | crate 能编译、能连、菜单可见、图标就位                | —     |
| **P1** | 只读浏览     | 库 / schema / 表 / 视图 / 列 / 分页 / 任意 SQL | P0    |
| **P2** | 编辑与表 DDL | 增删改、Drop/Empty/Truncate/Rename、表设计器   | P1    |
| **P3** | 视图与例程    | 视图列表/详情/保存/删除；函数与过程                   | P2    |
| **P4** | 用户与权限    | 角色列表/详情/保存/删除/重命名、对象权限                | P2    |
| **P5** | 备份与还原    | `.rgbak` 备份/还原（表 + 视图 + 例程）           | P3    |
| **P6** | 收尾与对等    | 隧道、Options 页、导出/解析方言、i18n、文档          | P1–P5 |

每个阶段结束都要求：`cargo fmt --all` + `cargo build` + `cargo clippy --workspace --all-targets -- -D warnings`  
全绿，且**不引入新告警**（当前仓库零告警）。

---

## 6. 详细任务清单

> 勾选框用于落地时跟踪。每条任务都给出**文件**、**要点**与**验收**。

### Phase 0 — 脚手架与骨架

- [ ] **P0-T1 新增 workspace 成员。**  
  改根 `Cargo.toml`：`members` 加 `"crates/rustgrid-postgresql"`；  
  `[workspace.dependencies]` 加 `rustgrid-postgresql = { path = "crates/rustgrid-postgresql" }`。  
  *验收*：`cargo metadata` 通过。
- [ ] **P0-T2 建 `crates/rustgrid-postgresql/Cargo.toml`。**  
  仿 `rustgrid-mysql/Cargo.toml`（`rustgrid-sqlite` 只有 `core` + `sqlx`，依赖列表不够参考）：
  ```toml
  [dependencies]
  rustgrid-core.workspace = true
  async-trait.workspace = true
  chrono.workspace = true
  futures-util.workspace = true
  tokio.workspace = true
  sqlx = { workspace = true, features = ["postgres"] }

  [dev-dependencies]
  tokio.workspace = true
  ```
  *注意*：`postgres` 走仓库已开的 `tls-rustls-ring`，**不新增原生编译依赖**（rustls 的 `ring`
  仍要用 C 编译器，但 MySQL 已经引入，不是 PG 带来的）。  
  是否再追加 sqlx 类型 feature（`bigdecimal` 等）取决于 §3.8 选方案 A 还是 B——**选 A 就到此为止**。  
  `[dev-dependencies] tokio` 是 `#[tokio::test]` 必需的（`rustgrid-mysql` / `rustgrid-sqlite` 都有）。  
  *验收*：`cargo check -p rustgrid-postgresql` 通过（此时 lib.rs 可为空）。
- [ ] **P0-T3 `driver.rs`：`PostgresDriver` 基本实现。**
  - `id() = "postgresql"`、`display_name() = "PostgreSQL"`、`default_port() = 5432`、  
    `is_file_based() = false`。
  - `supports_database_management/users/routines/schemas` 全部 `true`  
    （**`supports_schemas()` 必须与 descriptor 的 capabilities 一致**：默认 `descriptor()`
    就是拿这些 flag 构造 capabilities 的）。
  - `database_editor()`：
    ```rust
    DatabaseEditorSpec { charset: true, collation: true, owner: true, ..Default::default() }
    ```
    （`charset`→`ENCODING`、`collation`→`LC_COLLATE`、`owner`→`OWNER TO`；PG 无 recovery/compat。）
  - `descriptor()`：
    ```rust
    DriverDescriptor {
        id: DriverId::new("postgresql"),
        display_name: "PostgreSQL".to_string(),
        default_port: 5432,
        is_file_based: false,
        icon: "icons/postgresql.svg",
        icon_style: DriverIconStyle::Brand(0x336791), // PostgreSQL 品牌蓝
        capabilities: DriverCapabilities::none()
            .with(DriverCapability::DatabaseManagement)
            .with(DriverCapability::Users)
            .with(DriverCapability::Routines)
            .with(DriverCapability::Schemas),
        database_editor: self.database_editor(),
        connection_form: Default::default(),
        order: 20, // 与 PLANNED_ENGINES 的槽位一致
    }
    ```
  - `dialect() = DriverDialect::Postgres`（需先加变体，见 P0-T5）。
  - `connect()`：建 `PgConnectOptions`（host/port/username/password/database 可选，默认 `postgres`）、  
    `PgSslMode` 映射、`PgPoolOptions`（`max_connections(5)`、`acquire_timeout`、`idle_timeout`、  
    `after_connect` 跑 `init_sql` / `SET default_transaction_read_only` / `SET statement_timeout`）、  
    默认库探活；返回 `Box::new(PostgresConnection::new(...))`。
  - `map_connect_error`：SQLSTATE `28P01`（invalid_password）/`28000`（invalid_authorization_specification）  
    → `Error::Authentication`；其余 → `Error::Connection`。仿  
    `rustgrid-mysql::map_connect_error` 的 downcast 写法（用 `PgDatabaseError` 的 `code()`）。  
    *验收*：`cargo check -p rustgrid-postgresql` 通过。


- [ ] **P0-T4 注册驱动 + 图标 + 菜单占位。**
  - `crates/rustgrid-app/Cargo.toml` 加 `rustgrid-postgresql.workspace = true`。
  - `main.rs`：在 `BuiltinDriverSource::new()` 链上加一行
    `.with(Arc::new(rustgrid_postgresql::PostgresDriver::new()))`（放在 `SqlServerDriver` 之后、
    `#[cfg(feature = "driver-odbc")]` 那次重绑定之前）。
  - 新增 `assets/icons/postgresql.svg`（单色可染色，参照 `sqlite.svg`/`sqlserver.svg`），  
    在 `assets.rs` 的 `match` 里注册 `"icons/postgresql.svg"`。
  - `toolbar.rs` 的 `PLANNED_ENGINES`：删掉 `("postgresql", "PostgreSQL", 20)`，数组长度 3→2。  
    （**非必需**：`toolbar.rs:321-325` 已按 registry 过滤，注册后占位会自动跳过；但留着是死代码。）  
    *验收*：启动 app，**连接**菜单里 PostgreSQL 可点（非灰显）且**只出现一次**，其余占位
    （Oracle/MongoDB）仍灰显。
- [ ] **P0-T5 新增方言 `DriverDialect::Postgres`。**  
  `crates/rustgrid-core/src/dialect.rs`：加变体 `Postgres`；在 `builtin_functions()` 里  
  接 `POSTGRES_FUNCTIONS`（新增常量：`now()`/`current_date`/`coalesce`/`nullif`/`greatest`/`least`/  
  `count`/`sum`/`avg`/`min`/`max`/`array_agg`/`string_agg`/`jsonb_build_object`/`to_char`/`to_date`/  
  `date_trunc`/`extract`/`substring`/`split_part`/`regexp_replace`/`generate_series`/`row_number`/  
  `rank`/`dense_rank`/`lag`/`lead`/`lower`/`upper`/`trim`/`length`/`md5`/`encode`/`decode`/`uuid_generate_v4`…）。  
  更新 `Match` 里的穷尽分支（`SqlServer | Sqlite | Generic => &[]`）。  
  *验收*：`cargo check --workspace` 通过；查询编辑器补全能提示 Postgres 函数。
- [ ] **P0-T6（可选，推荐）抽取共享隧道 crate。**  
  新建 `crates/rustgrid-tunnel`，迁入 `rustgrid-mysql/src/tunnel.rs`，`rustgrid-mysql` 改  
  `pub use rustgrid_tunnel::Tunnel;`；同时迁移 `russh` / `base64` 依赖（没有 `sha1`）。  
  若本阶段时间紧，可延到 P6-T1，先只支持直连。  
  *验收*：MySQL 的隧道测试/功能不变。

### Phase 1 — 只读浏览（先把"能连、能看、能查"打通）

- [ ] **P1-T1 `connection.rs` 骨架 + 分片池。**  
  实现 §3.1 的 `PostgresConnection`（`options` + `default_database` + `pools` + `_tunnel`），  
  `pool(database)` 取或建池（非默认库 `max_connections(2)` + 短 `idle_timeout`，见 R1），
  `driver_id()` 返回 `"postgresql"`。  
  *验收*：能 `connect()` 到一个真库，`pool(None)` 可用。
- [ ] **P1-T2 `helpers.rs`：引用 / 限定名 / 参数。**
  - `quote_identifier(s)`：`"..."`，内部 `"` → `""`。
  - `qualify(schema, name)`：`"s"."n"`。
  - `resolve_object(database, table) -> (schema, name)`：解析 `schema.name`；无前缀回退  
    `search_path` 的第一个用户 schema（查一次并缓存，通常 `public`）。
  - `placeholder(i) -> "$i"`（1-based）。  
    *验收*：`#[cfg(test)]` 单测覆盖带引号、带点、大小写混合的标识符。
- [ ] **P1-T3 `list_databases`。**  
  `SELECT datname FROM pg_database WHERE datistemplate = false AND datallowconn ORDER BY datname`。  
  *验收*：树里列出全部库。
- [ ] **P1-T4 `list_schemas`。**  
  见 §3.3 的查询（排除系统 schema）。  
  *验收*：树里数据库下出现 schema 层，且无 `pg_catalog`/`information_schema`。
- [ ] **P1-T5 `list_tables`。**  
  **不要查 `information_schema.tables`**——它只列出当前用户有权限的对象，非超级用户会漏表。
  改查 catalog（`relkind`：`r` 普通表 / `p` 分区表 / `v` 视图 / `m` 物化视图 / `f` 外部表）：
  ```sql
  SELECT n.nspname, c.relname, c.relkind
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname NOT LIKE 'pg\_%' AND n.nspname <> 'information_schema'
    AND c.relkind IN ('r','p','v','m','f')
  ORDER BY 1, 2
  ```
  映射 `r`/`p`→`ObjectKind::Table`、`v`/`m`/`f`→`ObjectKind::View`；名字返回 `schema.name`；  
  `updatable` 另查 `information_schema.views.is_updatable`（视图）。  
  *验收*：表 / 视图两栏都正确，名字带 schema 前缀；**普通用户连接下表不缺失**。
- [ ] **P1-T6 `columns`。**  
  用 `format_type` 拿完整类型（`character varying(50)` / `numeric(10,2)`），主键来自 `pg_index.indisprimary`，  
  默认值 `pg_get_expr(adbin, adrelid)`，注释 `col_description`。  
  **这个完整类型串后面要给 §3.7 的 `CAST` 用，务必原样保留。**
  ```sql
  SELECT a.attname,
         format_type(a.atttypid, a.atttypmod) AS data_type,
         NOT a.attnotnull AS nullable,
         col_description(a.attrelid, a.attnum) AS comment,
         EXISTS (SELECT 1 FROM pg_index i
                 WHERE i.indrelid = a.attrelid AND i.indisprimary
                   AND a.attnum = ANY(i.indkey::int2[])) AS primary_key
  FROM pg_attribute a
  JOIN pg_class c ON c.oid = a.attrelid
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = $1 AND c.relname = $2
    AND a.attnum > 0 AND NOT a.attisdropped
  ORDER BY a.attnum
  ```
  （`indkey` 是 `int2vector`，显式转成 `int2[]` 再 `ANY` 更保险。）  
  *验收*：列名/类型/可空/主键/注释正确；`varchar` 带长度。
- [ ] **P1-T7 `fetch_page` + 过滤/排序翻译。**
  - `helpers::filter_clause(&[FilterNode]) -> (String, Vec<String>)`：递归过滤树（与 SQL Server 的  
    `filter_clause` 同构），产出 `WHERE` 片段 + 绑定值序列，**绑定顺序与 `$n` 严格一致**。  
    注意 Postgres 的大小写敏感 `LIKE`（`Contains` 用 `ILIKE` 还是 `LIKE` 需与 MySQL 行为对齐后决定）。
  - **每个绑定值都要带类型**（§3.7）：`WHERE "n" = CAST($1 AS integer)`，
    类型串取自 P1-T6 的 `format_type`；取不到列元数据时退化为不带 `CAST`（并在文档注明限制）。
  - `order_clause`：`PageRequest.order_by` → `ORDER BY "c" ASC/DESC`。
  - 分页 SQL：`SELECT {cols} FROM "s"."t" {WHERE} {ORDER BY} LIMIT $n OFFSET $m`；
    `{cols}` 按 §3.8 方案 A 投影成 `"c"::text`（否则二进制解码取不到 `numeric`/`uuid`/数组的文本）。
  - **稳定分页**：无显式排序时按主键排序；无主键的**基表/分区表/物化视图**用 `ORDER BY ctid`
    （Postgres 特有，保证稳定）。**视图与外部表没有 `ctid`**——它们退化为不带 `ORDER BY` 的
    `LIMIT/OFFSET`（按 `relkind` 判断，`fetch_page` 也会被用来打开视图，见 `app/grid.rs:263`）。
  - `total_rows`：`SELECT count(*) FROM ... {WHERE}`（同样注意绑定类型，但只读计数、无需解码）。
    大表上 `count(*)` 慢，文档注明。
    *验收*：翻页顺序稳定；过滤树（含嵌套组、`IN`、`BETWEEN`、`IS NULL`）结果正确；
    **在 integer / boolean / numeric / timestamptz 列上过滤都不报类型错误**。
- [ ] **P1-T8 `decode_cell`：类型映射。**  
  仿 `rustgrid-mysql::decode_cell` 的**先 checked 后兜底**策略，但**必须按 §3.8 区分协议**：
  1. 已知类型用 sqlx checked `try_get`（强制 `Type::compatible`）：`bool` / `i16` / `i32` / `i64` /
     `f32` / `f64` / `String`（TEXT/VARCHAR/CHAR，二进制下字节即文本）；
  2. `numeric`/`decimal` 解码为 `CellValue::Text`（精确字符串，**不丢精度**，同 SQL Server 的做法）：
     **走 §3.8 方案 A（`::text` 投影）或方案 B（`bigdecimal` feature）**，不能靠 unchecked 字节兜底；
  3. `json`/`jsonb`/`xml`/`uuid`/`inet`/`cidr`/`macaddr`/`interval`/数组 → `Text`（同上，依赖 §3.8 的选择）；
  4. `bytea` → `Bytes`；
  5. `date`/`time`/`timestamp`/`timestamptz` → `Text`（按 `chrono` 解码后格式化）；
  6. 其它未知类型：文本格式下 `try_get_unchecked::<String>` 兜底；二进制格式下不能假设字节是文本
     （回落到 `Bytes` 显示十六进制，并在文档注明该类型按原始字节展示）。  
     *验收*：`#[cfg(test)]` 单测覆盖 §8 的映射表；`numeric` 大数不丢精度；
     在**带过滤（二进制路径）**的分页结果里 `numeric`/`uuid` 也不出乱码。
- [ ] **P1-T9 `execute_query` / `execute_query_many`。**
  - **`execute_query_many` 照抄 MySQL 的写法（§3.4）**：acquire **一条** `PoolConnection`，
    `sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).fetch_many(&mut *connection)`，
    按 `Either::Left(PgQueryResult)` 边界切结果集。**不做客户端切分执行。**
  - `execute_query`（单条）走同一条路径，取第一个结果集。
  - **零行 `SELECT` 的列元数据**：`fetch_many` 只在收到 `DataRow` 时才看得到列，零行时列会丢。
    对该条语句 `pool.prepare(stmt)` 补元数据（`Executor::prepare` 在 `&Pool` 上有实现）。
    **不要用 `Executor::describe`**：它在 sqlx 0.9 里被 `#[cfg(feature = "offline")]` 门控
    （`sqlx-core/src/executor.rs:200`、`pool/executor.rs:65`、`sqlx-postgres/.../executor.rs:463`），
    而仓库的 sqlx 是 `default-features = false`，直接用会编译不过。`prepare` 只能传单条语句——
    这正是客户端切分器存在的理由之一。
  - **客户端切分器**（仅用于 `statement` 文本与零行 `prepare`）识别：单引号字符串、`"..."` 标识符、
    `$$…$$` / `$tag$…$tag$`、`--` / `/* */` 注释。**不要**把 `BEGIN … END` 当块，见 §3.4。
  - `returns_result_set` 分类：以 `SELECT`/`WITH`/`VALUES`/`TABLE`/`SHOW`/`EXPLAIN`/`RETURNING`  
    开头视为结果集（`INSERT ... RETURNING` 两者都要）。
  - `statement` 字段填**该条**语句文本（MySQL 留空；PG 填上，让 `infer_single_table`
    能判定结果网格可编辑——注意 P6-T5 的 dialect 修正，否则 PG 的双引号名解析不对）。
  - `last_insert_id`：Postgres 无 `LAST_INSERT_ID`；从 `RETURNING` 或置 `None`（文档说明）。  
    *验收*：单条 `SELECT`、多语句脚本、`CREATE FUNCTION $$...$$`、`INSERT ... RETURNING`、
    `BEGIN; INSERT …; COMMIT;`（**必须真的在一个事务里**）、`CREATE TEMP TABLE` + 后续 INSERT，均正确。
- [ ] **P1-T10 `server_version` / `session_count` / `close`。**
  - `SHOW server_version`（或 `SELECT version()`）。
  - `SELECT count(*) FROM pg_stat_activity WHERE datname IS NOT NULL AND pid <> pg_backend_pid()`
    （不加 `datname` 条件会把后台进程也算进去，数字虚高；注意非超级用户看不到他人的会话，
    数字会偏小，**文档注明**）。
  - `close()`：关所有池。  
    *验收*：连接信息面板显示版本与会话数。
- [ ] **P1-T11 `create_schema` / `drop_schema`（必须实现）。**  
  `CREATE SCHEMA "s"` / `DROP SCHEMA "s"`。**默认实现是报错**（`core/driver.rs:284/291` 返回
  `Err(Error::Query("schema management is not supported"))`），而 P0-T3 设了
  `supports_schemas = true` 后 UI 会显示"新建模式/删除模式"（`app/widgets.rs:306`），
  不实现就是坏按钮。`DROP SCHEMA` 非空默认报错，错误透传即可。  
  *验收*：新建/删除空 schema 成功；删除非空 schema 给出可读错误。

### Phase 2 — 编辑与表 DDL

- [ ] **P2-T1 `update_rows`。**  
  主键优先、无主键退化全列；`None` 键 → `IS NULL`（**绝不绑定空串**）；`$n` 参数化；事务内执行。  
  **每个 `SET` / `WHERE` 绑定都要加 `CAST($n AS <format_type>)`**（§3.7），否则强类型列会报错。  
  *验收*：单列/多列主键、含 `NULL` 键的行都能改；**integer / boolean / numeric / timestamptz
  列都能改成功**（这是最容易漏的验收点）。
- [ ] **P2-T2 `insert_rows`。**  
  `INSERT INTO "s"."t" ("c1","c2") VALUES (CAST($1 AS ...),CAST($2 AS ...))`；未列出的列走默认值；
  `None` 是显式 `NULL`。  
  *验收*：自增（`serial`/`identity`）列不写值也能插入。
- [ ] **P2-T3 `delete_rows`。**  
  与 `update_rows` 同样的键处理与 `CAST`；批量事务。  
  *验收*：多选删除正确，`NULL` 键行能删。
- [ ] **P2-T4 表操作：`drop_table` / `empty_table` / `truncate_table` / `rename_table`。**
  - `DROP TABLE "s"."t"`；
  - `DELETE FROM "s"."t"`（Empty，可回滚）；
  - `TRUNCATE TABLE "s"."t"`（可加 `RESTART IDENTITY`？先不加，保持语义最小；  
    被外键引用时需 `CASCADE`，错误要透传）；
  - `ALTER TABLE "s"."old" RENAME TO "new"`（`new_name` 可能带 schema 前缀，**要先剥掉**）。  
    *验收*：右键菜单四项都生效。
- [ ] **P2-T5 库管理：`create_database` / `create_database_sql` / `drop_database` / `database_options` / `alter_database_options` / `alter_database_sql`。**
  - 建库：`CREATE DATABASE "x" WITH TEMPLATE template0 ENCODING '<enc>' LC_COLLATE '<loc>' LC_CTYPE '<loc>' OWNER "<owner>"`；  
    空字段省略（用引擎默认）。
  - **库上下文（易漏）**：`CREATE DATABASE` / `DROP DATABASE` **不能在事务里**，也**不能连着目标库
    DROP 它**。两个操作都走**维护库**的连接——优先 `postgres` 库，不存在时回退
    `default_database`（若 `default_database` 就是目标库，则回退 `template1`）。
    删库前有活动连接会失败，错误要透传并给出可懂的提示。
  - 读选项：`SELECT pg_encoding_to_char(encoding), datcollate, datctype, pg_get_userbyid(datdba) FROM pg_database WHERE datname = $1`。
  - 改选项：**只支持 OWNER**（`ALTER DATABASE "x" OWNER TO "y"`）；  
    encoding/LC_COLLATE 建库后不可改 → 明确返回错误信息（不静默成功）。
  - `database_owners()`：`SELECT rolname FROM pg_roles ORDER BY rolname`。  
    *验收*：建/删库可用（含删除**非当前**库）；改库只改 owner；尝试改 encoding 时对话框给出清晰错误。
- [ ] **P2-T6 `character_sets` / `collations`。**
  - encoding：PG 没有编码目录表，用
    `SELECT pg_encoding_to_char(i) FROM generate_series(0, 50) i WHERE pg_encoding_to_char(i) <> ''`
    枚举服务端支持的全部编码，再 ∪ 服务器当前编码（比手工策展表更准）。
  - collation：**`LC_COLLATE` 要的是 locale 名**（`en_US.utf8` / `C`），不是 `pg_collation.collname`。
    用 `SELECT DISTINCT collcollate FROM pg_collation WHERE collcollate <> '' AND collprovider = 'c' ORDER BY 1`
    （`collcollate`/`collctype` 才是 libc locale 串）。**必须显式过滤 `collprovider = 'c'`**：ICU 条目
    （`collprovider = 'i'`）的 `collcollate` 是 ICU locale 名，不是合法的 `LC_COLLATE`，混进去会导致
    建库失败。  
    `pg_collation.collname` 会重复（同名的 libc/ICU 条目），直接用会出重复项。  
    *验收*：库对话框下拉非空、无重复项，选出来的值能真的建库成功。
- [ ] **P2-T7 `column_types` / `storage_engines`。**
  - `column_types()`：`smallint`/`integer`/`bigint`/`serial`/`bigserial`/`smallserial`/`numeric`/`real`/  
    `double precision`/`boolean`/`text`/`varchar`/`char`/`bytea`/`date`/`time`/`timetz`/`timestamp`/  
    `timestamptz`/`interval`/`uuid`/`json`/`jsonb`/`xml`/`money`/`inet`/`cidr`/`macaddr`/`bit`/`varbit`。
  - `storage_engines()`：`Vec::new()`（Postgres 无存储引擎）。  
    **注意**：`format_type` 回显的是 `character varying(50)` / `timestamp with time zone` 这种
    规范名，而下拉里是 `varchar` / `timestamptz`。表设计器要能匹配（在驱动侧统一成一种写法，或让
    设计器做映射），否则打开已有表时类型下拉会选不中。  
    *验收*：表设计器类型下拉正确；打开已有表时类型能正确回显；Options 页不显示引擎字段。
- [ ] **P2-T8 `table_status` / `table_statuses`。**
  - 行数用 `pg_class.reltuples`（**估算值**，PG 的 `count(*)` 太慢，不用）；  
    **未 ANALYZE 时 `reltuples` 是 `-1`，必须转成 `0`/`None`**，否则 UI 会显示负数。
  - `pg_total_relation_size` / `pg_relation_size` / `pg_indexes_size`；  
    注释 `obj_description(oid, 'pg_class')`；引擎填 `None` 或 `"PostgreSQL"`；  
    created/updated 无对应 → `None`。
  - `table_statuses`：按 schema 批量查（一次 JOIN `pg_class` + `pg_namespace`），
    key 用**与 `list_tables` 一致的 `schema.name`**。  
    *验收*：表列表的 行/数据长度/注释 列有值（估算），新建未分析的表不显示 `-1`。
- [ ] **P2-T9 `object_ddl`。**
  - 表：**从 catalog 拼 `CREATE TABLE`**（列 + `NOT NULL` + `DEFAULT` + 主键 + 唯一/检查约束 +  
    索引 + 外键），复用 P2-T10 的生成器。参考实现看 `rustgrid-sqlserver`
    （它同样没有 `SHOW CREATE`，是从系统表拼的）；**不是 `rustgrid-mysql`**——它有 `SHOW CREATE`，
    且表设计器逻辑全在 2677 行的 `connection.rs` 里，没有独立的 `schema.rs`。
  - 视图：`pg_get_viewdef(oid, true)` → `CREATE VIEW "s"."v" AS <def>`。  
    *验收*：信息面板能看到可读的 DDL。
- [ ] **P2-T10 `table_schema` / `table_schema_sql` / `save_table_schema`（表设计器核心）。**
  - `table_schema`：从 `pg_attribute` / `pg_attrdef` / `pg_constraint`（主键、唯一、外键、检查）/  
    `pg_index` / `pg_trigger` 组装 `TableSchema`（列、索引、外键、触发器、options）。  
    `serial` 列识别：默认值 `nextval('..._seq'::regclass)` → `auto_increment = true`；
    `GENERATED ALWAYS AS IDENTITY` 同样识别。
  - `table_schema_sql`：
    - 新建：`CREATE TABLE "s"."t" (...)`；
    - `auto_increment`（`ColumnDef::auto_increment`，`core/model.rs:474`）：生成
      `GENERATED BY DEFAULT AS IDENTITY`（PG 10+ 推荐）或 `serial`；改动已有列的自增属性要
      相应地 `ADD GENERATED ... AS IDENTITY` / `DROP IDENTITY`。**计划里必须钉死用哪一种。**
    - 修改：生成 `ALTER TABLE` 序列——`ADD COLUMN` / `DROP COLUMN` / `ALTER COLUMN TYPE ... USING` /  
      `SET|DROP NOT NULL` / `SET|DROP DEFAULT` / `ADD|DROP CONSTRAINT` / `RENAME COLUMN`；  
      索引用 `CREATE INDEX` / `DROP INDEX`；外键用 `ADD CONSTRAINT` / `DROP CONSTRAINT`；  
      触发器用 `CREATE TRIGGER ... EXECUTE FUNCTION` / `DROP TRIGGER`。
  - `save_table_schema`：默认实现即可（拼 SQL 后走 `execute_query`）。  
    *验收*：设计器能读出现有表结构、改列/加索引/加外键后保存成功且结构正确。

### Phase 3 — 视图与例程

- [ ] **P3-T1 `list_views`（经 `list_tables`）+ 视图详情。**
  - `view_details`：`ViewInfo`（name/updatable）+ `definition`（`CREATE VIEW "s"."v" AS <pg_get_viewdef>`）。
  - Postgres 无 MySQL 的 definer/algorithm/check_option/security_type 元数据 → 这些字段留空或从  
    `reloptions`/定义文本尽力推导，**在文档里注明限制**。
  - `updatable`：`information_schema.views.is_updatable`。  
    *验收*：视图列表与详情页可用。
- [ ] **P3-T2 `view_sql` / `save_view` / `drop_view`。**
  - 保存：`CREATE OR REPLACE VIEW "s"."v" AS ...`；若改了列集/列名导致无法 OR REPLACE，  
    退化为 `DROP VIEW` + `CREATE VIEW`（`view_sql` 里体现）。
  - 删除：`DROP VIEW "s"."v"`。  
  - ⚠️ app 的 `sql::view_identity`（`sql.rs:240`）对 `CREATE VIEW "s"."v"` 会返回 `"s"`，
    `ViewEdit.name` 因此拿不到真正的视图名；**驱动不要依赖 `edit.name`，按定义文本解析**
    （或先在 P6-T5a 修 `view_identity`）。  
    *验收*：设计视图→保存→预览数据链路通；改名/改列集后仍保存到正确的视图。
- [ ] **P3-T3 `list_routines` / `list_routine_infos`。**
  ```sql
  SELECT p.proname, p.prokind,
         pg_get_function_identity_arguments(p.oid) AS args,
         pg_get_function_result(p.oid) AS result,
         obj_description(p.oid, 'pg_proc') AS comment,
         p.provolatile, p.prosecdef
  FROM pg_proc p
  JOIN pg_namespace n ON n.oid = p.pronamespace
  WHERE n.nspname = $1 AND p.prokind IN ('f','p')
  ORDER BY p.proname
  ```
  `name` = `nspname.proname(args)`（§3.5，schema 限定 + 签名）；`kind` 由 `prokind`；`return_type` 用 `pg_get_function_result`；  
  `deterministic` = `provolatile = 'i'`；`security_type` = `prosecdef` → `DEFINER`/`INVOKER`。  
  **`list_routines`（备份树）必须返回与 `list_routine_infos` 完全一致的名字（含签名）**，
  否则备份树里选的对象在驱动里反解不出来。  
  *验收*：Functions 页列出函数与过程，重载函数各占一行；备份树同名可解析。
- [ ] **P3-T4 `routine_details`。**  
  `SELECT pg_get_functiondef(p.oid) ...` → `RoutineDetails.definition`（已是 `CREATE OR REPLACE FUNCTION`）。  
  *验收*：设计函数页的"定义"子页显示完整定义。


- [ ] **P3-T5 `routine_sql` / `save_routine` / `drop_routine`。**
  - 保存：直接回放 `pg_get_functiondef` 形式的定义（`CREATE OR REPLACE`）；若**类型**变了  
    （函数↔过程）或签名变了，先 `DROP` 旧对象再 `CREATE`（用 `original` 的 `(name, kind)`）。  
    注意：**这些语句要走 `execute_query`（单连接、简单协议）跑**，理由是保证同一会话、避免逐句
    prepare，并让 `$$` 原样交给服务端解析（扩展协议的 `prepare` 只能单条语句，多句脚本会失败）。
  - 删除：`DROP FUNCTION "s"."name"(argtypes)` 或 `DROP PROCEDURE ...`。
  - `list_events`：`Ok(Vec::new())`（Postgres 无事件调度器）。  
    *验收*：新建/修改/删除函数与过程均成功。

### Phase 4 — 用户与权限

- [ ] **P4-T1 `list_users`。**
  ```sql
  SELECT rolname, rolsuper, rolcreaterole, rolcreatedb, rolcanlogin,
         rolreplication, rolbypassrls, rolconnlimit, rolvaliduntil
  FROM pg_roles
  WHERE rolname NOT LIKE 'pg\_%'     -- PG 14+ 有十几个 pg_* 内置角色，默认隐藏
  ORDER BY rolname
  ```
  映射到 `UserAccount`（`host` 置空字符串；**检查 `UserAccount::label()` 的 `user@host` 渲染，
  空 host 时不要显示成 `postgres@`**）。  
  *验收*：Users 页列出全部**用户**角色，不出现 `pg_read_all_data` 之类内置角色。
- [ ] **P4-T2 `user_details`。**  
  角色属性 + 成员关系（`pg_auth_members`）+ 对象授权 →  
  `UserDetails`（`UserEditSection` 分组：常规 / 权限 / 角色）。  
  *验收*：编辑用户窗口能回显属性、成员、授权。
- [ ] **P4-T3 `user_edit_sql` / `user_edit_groups` / `save_user` / `drop_user` / `rename_user`。**
  - 保存：`CREATE ROLE ... LOGIN PASSWORD '...'`（新建）/ `ALTER ROLE ...`（修改）；  
    `GRANT/REVOKE <role> TO/FROM <user>`；`GRANT/REVOKE <priv> ON ...`。
  - **密码与标识符必须转义**：口令里的 `'` 要加倍（或走 `quote_literal`），
    角色名一律 `"..."` 引用且内部 `"` 加倍。不要裸拼字符串。
  - 删除：`DROP ROLE "x"`。
  - 重命名：`ALTER ROLE "old" RENAME TO "new"`（保留授权，与 MySQL 的 `RENAME USER` 意图一致）。
  - `authentication_plugins` / `ssl_types`：Postgres 用 `scram-sha-256`/`md5`/`password`；  
    可返回策展列表或空（空则表单隐藏）。  
    *验收*：新建/改/删/改名角色都成功；SQL 预览正确；口令含 `'` 不产生注入/语法错误。
- [ ] **P4-T4 `object_privilege_matrix` / `set_object_privileges` / `object_privileges_sql`。**
  - Postgres 对象权限：`SELECT`/`INSERT`/`UPDATE`/`DELETE`/`TRUNCATE`/`REFERENCES`/`TRIGGER`/  
    `CREATE`/`CONNECT`/`TEMPORARY`/`EXECUTE`/`USAGE`/`MAINTAIN`。
  - **不要用 `information_schema.role_table_grants`**——它只显示与当前用户相关的授权。
    直接读 ACL：`(aclexplode(c.relacl)).grantee` / `.privilege_type`，再用
    `pg_get_userbyid(grantee)` 映射回角色名。
  - 映射到 core 的 `Privilege` 枚举**存在的子集**；无法表达的（`TRUNCATE`/`USAGE`/`CONNECT`/…）  
    在驱动内部用字符串处理，**不改 core 枚举**（若 UI 必须展示，见 §10 风险 R3）。  
    *验收*：权限管理器能读/写表级授权，且能看到**其他用户**的授权。

### Phase 5 — 备份与还原

- [ ] **P5-T1 `backup_object_metadata`（表 / 视图 / 函数）。**
  - 表：DDL 走 P2-T9 的拼装器；`fields` 取列名；`trigger_ddl` 取 `pg_get_triggerdef`。
  - 视图：`pg_get_viewdef` → `CREATE VIEW`。
  - 函数：`pg_get_functiondef`。
  - `rows` 恒为空（表数据走 `stream_table_rows`）。  
    *验收*：备份对象树能取到每类对象的 DDL。
- [ ] **P5-T2 `stream_table_rows`：Postgres 字面量渲染。**  
  流式 `SELECT * FROM "s"."t"`（用 `fetch` 边收边渲染，**不整表缓冲**），逐行渲染元组：  
  `'...'`（`'`→`''`，含反斜杠用 `E'...'`）、`NULL`、数字裸值、`true`/`false`、  
  `bytea` → `'\x...'`、时间戳 → `'...'::timestamp`、`jsonb` → `'...'::jsonb`、数组 → `ARRAY[...]`。  
  *验收*：`#[cfg(test)]` 单测覆盖转义与各类型字面量。
- [ ] **P5-T3 `restore_object`。**  
  回放 DDL + 批量 `INSERT`。两点易漏：
  - **identity 列**：`GENERATED ALWAYS AS IDENTITY` 显式插入会报错，批量 INSERT 必须加  
    `OVERRIDING SYSTEM VALUE`（`serial` 不需要）。可在回放建表 DDL 后按列属性决定。
  - **序列重置**：还原后对 `serial`/`identity` 列
    `setval(pg_get_serial_sequence('"s"."t"', 'id'), coalesce(max(id), 1), max(id) IS NOT NULL)`，
    否则后续插入会主键冲突。注意 **空表时 `max(id)` 为 NULL**，`setval` 会报错，必须 `coalesce`；
    `pg_get_serial_sequence` 的表名参数要按大小写正确引用。  
  *验收*：备份→还原→再插入新行不冲突（含 `GENERATED ALWAYS` 表）；**空表还原后也不报错**。

### Phase 6 — 收尾与对等

- [ ] **P6-T1 隧道支持。** 若 P0-T6 未做，此时抽取 `rustgrid-tunnel` 并接入  
  `PostgresDriver::connect`（与 MySQL 的 `Tunnel::start` 用法一致）。
- [ ] **P6-T2 连接 Options 页对齐。** 确认 TLS 五档映射到 `PgSslMode`  
  （Disabled→`Disable`、Preferred→`Prefer`、Required→`Require`、VerifyCa→`VerifyCa`、  
  VerifyIdentity→`VerifyFull`），`ssl_root_cert`/`ssl_client_cert`/`ssl_client_key` 接线；  
  注明 rustls 下 `VerifyFull` 需要 root cert 的限制。  
  （core 的 `TlsMode` 五个变体与 `PgSslMode` 一一对应，PG 多一个用不到的 `Allow`。）
- [ ] **P6-T3 只读 / 超时 / init_sql 复核。** `SET default_transaction_read_only = on`、  
  `SET statement_timeout = <ms>`、`init_sql` 在 `after_connect` 全部生效。
- [ ] **P6-T4 导出 `.sql` 方言。** `rustgrid_export::SqlDialect` 目前只有 `MySql`（反引号），
  且 `app/src/app/export.rs:769` 硬编码 `SqlDialect::MySql`——PG 导出的 `.sql` 会是 MySQL 语法。  
  新增 `SqlDialect::Postgres`（`"..."` 引用等），并让 `export.rs` 按驱动的 `DriverDialect`
  选择；`.xlsx` / `.csv` / `.txt` 引擎无关，不用动。
- [ ] **P6-T5 SQL 解析 / 高亮方言。** `app/src/sql.rs` 有 4 处硬编码 `MySqlDialect`
  （行 34 / 79 / 135 / 254）。sqlparser 的 `MySqlDialect::is_delimited_identifier_start`
  **只认反引号**，`"public"."users"` 不会被当标识符，会影响：语法高亮、`view_select`（视图 EXPLAIN）、
  `infer_single_table`（**决定查询结果网格能否就地编辑**）。  
  改为按 `DriverDialect` 选 dialect：`Postgres` → `PostgreSqlDialect`，其余按需回落
  `GenericDialect`（它认 `"`，至少不会比现在更差）。**注意 `highlight` / `infer_single_table` /
  `view_select` / `meaningful_tokens` 都是无 dialect 参数的纯函数，要改成接收 `DriverDialect`
  或 `&dyn Dialect`，并更新调用点**（`query_editor.rs` 已能拿到 `driver.dialect()`）。
- [ ] **P6-T5a app 层例程/视图标识符与签名适配（§3.5 的落地）。** 这是 R2 的具体化：
  - `sql::routine_identity`（`sql.rs:157`）与 `sql::view_identity`（`sql.rs:240`）取的是关键字后
    **第一个 Word**。PG 的 `pg_get_functiondef` 与 `CREATE VIEW "s"."v"` 都带 schema 限定，于是返回
    **schema**（`public` / `s`）而不是对象名。调用点：`routine.rs:350`（save）、`routine.rs:431`（run）、
    `view.rs:179/208`（save/preview）。要么让二者跳过限定符，要么让驱动按定义自行解析——**二选一并钉死**。
  - `routine_call_sql`（`routine.rs:572`）与其 `quote_identifier`（`routine.rs:591`，硬编码反引号）
    把 name 当无签名、无 schema 的名字拼成 `` `db`.`name`() ``。PG 要 `SELECT "schema"."f"(...)` /
    `CALL "schema"."p"(...)`，且**不能把签名 `(integer)` 当成函数名的一部分**（§3.5 的 name 带签名）。
  - `routine_template`（`routine.rs:557`）是 MySQL 模板（反引号 + `#` 注释 + `BEGIN…END` 体）。
    PG 新建函数要给 PL/pgSQL 模板（`LANGUAGE plpgsql` + `$$ ... $$`）。
  - 这三个 helper 都要按 `DriverDialect` 分派（或由驱动注入模板），否则"运行函数/新建函数"
    会直接产出非法 SQL。  
  *验收*：PG 上 新建函数 / 运行函数 / 运行过程 / 保存例程 / 保存视图 都生成正确方言的 SQL。
- [ ] **P6-T5b 语句扫描器支持美元引用。** `sql::current_statement`（`sql.rs:526`）与
  `in_string_or_comment`（`sql.rs:603`）只认 `'` / `"` / 反引号 / `--` / `/* */`，**不认 `$$ … $$`**；
  PG 的 `CREATE FUNCTION ... $$ BEGIN ... ; ... $$` 会在函数体里的 `;` 处被切错，影响函数编辑器的
  补全上下文。`helpers::split_statements` 同理，且**不要把 `$1`/`$2` 占位符当美元引用**。  
  *验收*：`$$`/`$tag$` 内部的 `;` 不切分；`$1` 不被误判。
- [ ] **P6-T6 i18n 复核。** 新增文案（若有）在 `locales/{en,zh-CN}.yml` **两处都加**；  
  库对话框若需 Postgres 专属标签，走 `DatabaseEditorSpec` 扩展（优先复用现有 key）。
- [ ] **P6-T7 文档。**
  - 更新 `AGENTS.md`：Repository layout 加 `rustgrid-postgresql`；Tech stack 加 sqlx `postgres`
    feature；Commands 加 live 测试；**"Still out of scope" 那句
    "engines other than MySQL, MariaDB, SQLite and SQL Server" 要改写**（原文里并没有
    "PostgreSQL" 字样，别去找它）；`:469` 的 "schema-qualified names (SQL Server)" 补上 PG。
  - 更新 `README.md` / `README_zh.md` 引擎表：PostgreSQL → Supported。
  - 新增 `docs/postgresql-driver.md`（仿 `odbc-driver.md`）：连接串、TLS、限制、已知差异，
    重点写明**分页行数是估算值**、**强类型 CAST 策略**、**库管理走维护库**、
    **读路径解码（§3.8）** 与 **视图分页无 `ctid`** 这几条。
- [ ] **P6-T8 全量校验。** `cargo fmt --all` → `cargo build` → `cargo test --workspace` →  
  `cargo clippy --workspace --all-targets -- -D warnings` 全绿。

---

## 7. `Connection` trait 实现映射表

> 逐方法对照，作为落地时的检查表。带 ⚠️ 的是有非平凡实现成本的项。

| 方法                                                                                                               | Postgres 实现要点                                                                 |
| ---------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `driver_id`                                                                                                      | `"postgresql"`                                                                |
| `list_databases`                                                                                                 | `pg_database`（排除 template、不可连）                                                |
| `list_tables`                                                                                                    | ⚠️ `pg_class`+`pg_namespace`（**不用 `information_schema.tables`**），返回 `schema.name`（表+视图+物化/外部表） |
| `columns`                                                                                                        | `pg_attribute` + `format_type` + `pg_index` + `col_description`（类型串要留给 CAST 用）       |
| `fetch_page`                                                                                                     | `LIMIT/OFFSET`；无排序按主键，无主键的基表/分区表/物化视图 `ORDER BY ctid`，**视图/外部表无 `ctid`**；⚠️ 过滤树翻译 + ⚠️ 绑定 `CAST` + ⚠️ 列 `::text` 投影（§3.8） |
| `update_rows`                                                                                                    | 主键优先，`None`→`IS NULL`，`$n` 参数化 + ⚠️ `CAST`，事务                                    |
| `insert_rows`                                                                                                    | `INSERT ... VALUES (CAST($1 AS ...),...)`；identity 列不写值                        |
| `delete_rows`                                                                                                    | 同 update 的键处理与 `CAST`                                                          |
| `execute_query`                                                                                                  | `raw_sql` 简单协议；⚠️ 零行结果集用 `prepare()` 补列元数据（**不是 `describe()`**，它被 `offline` feature 门控） |
| `execute_query_many`                                                                                             | ⚠️ **单连接 + `fetch_many` 服务端切分**（§3.4），不客户端切分执行                                  |
| `create_database` / `_sql`                                                                                       | ⚠️ 走维护库、`CREATE DATABASE ... ENCODING/LC_COLLATE/OWNER TEMPLATE template0`  |
| `drop_database`                                                                                                  | ⚠️ 走维护库、不能在事务里、不能连着目标库删                                                        |
| `drop_table` / `empty_table` / `truncate_table` / `rename_table`                                                 | `DROP TABLE` / `DELETE FROM` / `TRUNCATE TABLE` / `ALTER TABLE ... RENAME TO`（新名去 schema 前缀） |
| `database_options`                                                                                               | `pg_database`（encoding/collate/ctype/owner）                                   |
| `database_owners`                                                                                                | `pg_roles`                                                                    |
| `server_version`                                                                                                 | `SHOW server_version`                                                         |
| `session_count`                                                                                                  | `pg_stat_activity` + `datname IS NOT NULL`                                    |
| `table_status` / `table_statuses`                                                                                | `pg_class.reltuples`（`-1` 要归零）+ `pg_*_size` + `obj_description`（行数为**估算**）        |
| `object_ddl`                                                                                                     | ⚠️ 表：拼装（参考 SQL Server）；视图：`pg_get_viewdef`                                       |
| `character_sets`                                                                                                 | `pg_encoding_to_char` 枚举（非策展表）                                                  |
| `collations`                                                                                                     | ⚠️ `pg_collation` 的 `collcollate`（locale 名），`DISTINCT`                        |
| `alter_database_options` / `_sql`                                                                                | ⚠️ 仅 OWNER；encoding/collate 不可改 → 报错                                          |
| `list_schemas`                                                                                                   | `pg_namespace`（排除系统 schema）                                                   |
| `create_schema` / `drop_schema`                                                                                  | `CREATE SCHEMA` / `DROP SCHEMA`（**默认实现是报错，必须覆写**，见 P1-T11）                   |
| `column_types`                                                                                                   | Postgres 类型表                                                                  |
| `list_routines` / `list_routine_infos`                                                                           | `pg_proc`（`prokind IN ('f','p')`），名字带签名且**两处一致**                                |
| `list_events`                                                                                                    | 空                                                                             |
| `backup_object_metadata`                                                                                         | ⚠️ 表拼装 / `pg_get_viewdef` / `pg_get_functiondef`                              |
| `stream_table_rows`                                                                                              | ⚠️ 流式 SELECT + Postgres 字面量渲染                                                 |
| `restore_object`                                                                                                 | 回放 DDL+INSERT，⚠️ `OVERRIDING SYSTEM VALUE` + 序列 `setval`                       |
| `storage_engines`                                                                                                | 空                                                                             |
| `table_schema` / `table_schema_sql`                                                                              | ⚠️ 设计器核心（catalog 读 + `CREATE/ALTER TABLE` 生成）                                 |
| `close`                                                                                                          | 关闭所有池                                                                         |
| `list_users` / `user_details` / `user_edit_sql` / `user_edit_groups` / `save_user` / `drop_user` / `rename_user` | ⚠️ `pg_roles`（过滤 `pg_*`）+ `CREATE/ALTER/DROP ROLE`；host 置空；口令转义                 |
| `authentication_plugins` / `ssl_types`                                                                           | `scram-sha-256`/`md5`/`password`（或空）                                          |
| `object_privilege_matrix` / `set_object_privileges` / `object_privileges_sql`                                    | ⚠️ `aclexplode(relacl)` 读全量授权 + 权限子集映射（见 §8）                                   |
| `view_details` / `view_sql` / `save_view` / `drop_view`                                                          | `pg_get_viewdef` + `CREATE OR REPLACE VIEW`                                   |

---

## 8. 类型映射表（Postgres → `CellValue`）

| Postgres 类型                                              | `CellValue`      | 备注                             |
| -------------------------------------------------------- | ---------------- | ------------------------------ |
| `bool`                                                   | `Bool`           |                                |
| `int2` / `int4` / `int8`                                 | `Int`            |                                |
| `oid` / `xid` / `cid`                                    | `Uint`           |                                |
| `float4` / `float8`                                      | `Float`          |                                |
| `numeric` / `decimal`                                    | `Text`           | **精确字符串，不转 f64**（同 SQL Server） |
| `money`                                                  | `Float`          | 或 `Text`，二选一并文档化               |
| `text` / `varchar` / `char` / `name` / `citext`          | `Text`           |                                |
| `bytea`                                                  | `Bytes`          | 显示为十六进制                        |
| `uuid`                                                   | `Text`           |                                |
| `date` / `time` / `timetz` / `timestamp` / `timestamptz` | `Text`           | `chrono` 解码后格式化                |
| `interval`                                               | `Text`           |                                |
| `json` / `jsonb` / `xml` / `tsvector`                    | `Text`           |                                |
| `inet` / `cidr` / `macaddr` / `macaddr8`                 | `Text`           |                                |
| 数组（`int4[]` 等）                                           | `Text`           | Postgres 数组字面量文本               |
| 其它 / 未知                                                  | `Text` 或 `Bytes` | `try_get_unchecked` 兜底         |

> 反向（写入）不走这张表：驱动把 `Option<String>` 直接绑定，靠 `CAST($n AS <format_type>)`
> 让 PG 自己解析（§3.7）。`format_type` 给出的类型串可原样用于 `CAST`。
>
> ⚠️ **正向（读取）的可行性取决于结果格式**：`numeric` / `json(b)` / `uuid` / `inet` / 数组 / `money`
> 这些映射为 `Text` 的类型，在 `sqlx::query` 的二进制路径上**不能**靠"unchecked 字节转 UTF-8"拿到文本。
> 按 §3.8 选方案 A（列 `::text` 投影）或方案 B（追加 sqlx 类型 feature），再回填本表的实现方式。

---

## 9. 测试计划

### 9.1 纯函数单元测试（放在 `src/**` 的 `#[cfg(test)] mod tests`）

> 仓库惯例：驱动 crate 的单测都在源文件里（`rustgrid-sqlserver/src/helpers.rs:528`、
> `rustgrid-mysql/src/{routine,user,view}.rs`、`rustgrid-sqlite/src/connection.rs:1646`），
> `tests/` 只放 live 集成测试。**不要建 `tests/helpers.rs`**——`tests/` 下每个文件是独立的
> 测试二进制，链接不到 crate 私有的 `helpers` 模块。

- 标识符引用与转义（`a"b`、大小写、含点）。
- `resolve_object` 的 `schema.name` / 裸名 / `search_path` 回退。
- 语句切分：`$$ ... ; ... $$`、`$tag$ ... $tag$`、字符串内分号、注释内分号；
  **`BEGIN; INSERT …; COMMIT;` 必须切成 3 条**（回归 §3.4 那条错误规则）；不平衡兜底；
  **`$1`/`$2` 占位符不被当作美元引用**。
- 过滤树翻译：嵌套组、`IN`、`BETWEEN`、`IS NULL`、绑定顺序与 `$n` 对齐，
  以及**每个绑定都带 `CAST(...)`**。
- 分页 SQL：有/无排序、有/无主键（`ctid`）、**视图（无 `ctid`）走不带 `ORDER BY` 的分支**；
  分页投影按 §3.8 生成 `::text`。
- DDL 生成：`CREATE TABLE`、`ALTER TABLE` 增删列/改类型、索引、外键、
  **`auto_increment` → identity/serial**。
- 字面量渲染：`'` 转义、反斜杠、`bytea`、时间戳、`jsonb`、数组、`NULL`。
- 类型映射：§8 全覆盖；**二进制路径（带绑定）下 `numeric`/`uuid` 也正确**。
- app 层 helper（`sql.rs` 单测）：`routine_identity`/`view_identity` 在
  `public.f(integer)`、`"s"."v"` 上的结果；`routine_call_sql` 对 PG 生成合法方言。

### 9.2 真机集成测试（`crates/rustgrid-postgresql/tests/live_postgresql.rs`，默认 `#[ignore]`）

环境变量：`RUSTGRID_PG_HOST` / `PORT` / `USER` / `PASSWORD` / `DATABASE`  
（默认 `localhost` / `5432` / `postgres` / — / `postgres`）。  
运行：`cargo test -p rustgrid-postgresql -- --ignored`。

覆盖：连接与认证失败映射 → 列库/列 schema/列表/列列 → 分页（含过滤与排序）→  
增删改 → 任意 SQL（含多语句、`$$` 函数、**事务脚本**）→ 库管理（含删非当前库）→ 表 DDL →  
表设计器读写 → 视图列表/打开 → 函数列表/详情 → 角色列表。

**必须覆盖的强类型用例**（§3.7，最容易回归）：对 `integer` / `boolean` / `numeric` /
`timestamptz` 列分别做 过滤、UPDATE、INSERT、按主键 DELETE。

**必须覆盖的读路径用例**（§3.8）：在**带过滤 / 绑定参数（二进制路径）**的分页结果里，
`numeric` / `uuid` / `jsonb` / 数组列显示的仍是正确文本；零行 `SELECT` 的列标题仍在
（`prepare()` 兜底）；打开一个**视图**分页不因 `ctid` 报错。

参考 `crates/rustgrid-sqlserver/tests/live_sqlserver.rs` 的结构。

Docker 起库参考：

```bash
docker run -d --name rg-pg -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:17
```

### 9.3 回归

- `cargo test --workspace`：MySQL / SQLite / SQL Server / ODBC 现有测试不得回归。
- `cargo clippy --workspace --all-targets -- -D warnings`：零告警。

---

## 10. 风险与未决问题

| #  | 风险 / 问题                                                    | 影响 | 建议                                                                   |
| -- | ---------------------------------------------------------- | -- | -------------------------------------------------------------------- |
| R1 | **分片池**使连接语义比其它引擎复杂；树里展开 N 个库会建 N 个池，PG 默认 `max_connections=100` | 中→高 | §3.1：非默认库 `max_connections(2)` + 短 `idle_timeout` + LRU 驱逐上限；默认库保持 5 |
| R2 | 例程签名编进 `name` 与 app 层的 `sql::routine_identity` / `routine_call_sql` 冲突（返回 schema、把签名当函数名） | 中→高 | P6-T5a 落地：先按 §3.5 实现驱动，再让这几个 app helper 方言化；确需时给 `RoutineInfo` 加可选 `signature`（**最后手段**） |
| R3 | core 的 `Privilege` 枚举是 MySQL 形状，Postgres 权限无法一一对应          | 中  | 驱动内做子集映射 + 字符串兜底；**不轻易改 core**；必要时单列一条 core 变更任务                     |
| R4 | 无 `SHOW CREATE`，表 DDL 拼装工作量大（列/约束/索引/外键/触发器/生成列）           | 高  | 分两步：先够用的 `CREATE TABLE`（P2-T9），再补全设计器 ALTER（P2-T10）；照 SQL Server 的写法   |
| R5 | ~~多语句切分对 `$$` 处理不当会破坏函数创建~~                                | 高→低 | **已化解**：§3.4 改为服务端 `fetch_many` 切分，不自研执行用切分器，`$$` 由服务端解析。残留风险只剩"填 `statement` 文本"时的切分偏差 |
| R6 | `raw_sql`/`fetch_many` 下零行 SELECT 的列元数据                    | 中  | 用 `pool.prepare()` 补元数据。**不能用 `describe()`**：sqlx 0.9 把它放在 `#[cfg(feature = "offline")]` 后面，而仓库未开该 feature（`sqlx-core/src/executor.rs:200`）。P1-T9 专门验证 |
| R7 | rustls 下 `VerifyFull` 需要 root cert；`VerifyCa` 与 MySQL 语义差异 | 低  | P6-T2 文档化限制                                                          |
| R8 | 新增 `sqlx-postgres` 依赖使体积增大                                 | 低  | 已知代价；`opt-level="z"`+`lto`+`strip` 已就位；优先只加 `postgres`（§3.8 选方案 A 即可，选 B 需再评估 `bigdecimal` 等） |
| R9 | 行数只能给估算值（`reltuples`），未 ANALYZE 时为 `-1`                    | 低  | 文档注明；`-1` 归零；需要精确时提供"刷新统计"可选增强                                       |
| **R10** | **强类型：按 `String` 绑定会让非文本列的增删改查直接报错**                 | **高** | §3.7 方案 A（`CAST($n AS <format_type>)`）；P1-T7 / P2-T1~T3 逐条验收，live 测试必须覆盖 int/bool/numeric/timestamptz |
| **R11** | **`information_schema` 只暴露有权限的对象**，非超级用户下表/授权列表不全   | 中  | 目录一律查 `pg_*` 系统表；权限读 `aclexplode(relacl)`                            |
| **R12** | **库管理的事务/上下文约束**（不能连着目标库 DROP、不能在事务里 CREATE/DROP DATABASE） | 中  | P2-T5 走维护库连接；错误透传                                                    |
| **R13** | **`CREATE FUNCTION $$…$$` / `CREATE-DROP DATABASE` 不能走会分连接的批量执行路径** | 中 | 一律走 `execute_query`（单连接 + 简单协议）                                      |
| **R14** | **读路径解码**：`sqlx::query`（带绑定）走扩展协议→二进制，MySQL 式"unchecked 字节转 UTF-8"兜底取不到 `numeric`/`uuid`/`json(b)`/数组的文本 | **高** | §3.8：`fetch_page` 列 `::text` 投影（方案 A，推荐）或补 `bigdecimal`/`json`/`uuid` feature（方案 B）；P1-T8 / 9.2 专门验收 |
| **R15** | **schema 管理默认实现是报错**，而 `supports_schemas=true` 会暴露"新建/删除模式"按钮 | 中 | P1-T11 必须实现 `create_schema`/`drop_schema`，不能依赖默认实现 |

**动手前需拍板**：§3.1 分片池方案（含连接数上限）、§3.5 签名编码方案 +
app 层 helper 的方言化落点（P6-T5a）、§3.6 隧道抽取（P0-T6 是否本阶段做）、
§3.7 绑定类型方案（A 还是 B）、**§3.8 读路径解码方案（A 投影 `::text` 还是 B 加类型 feature）**、
§3.3 系统 schema 是否可配置显示。

---

## 11. 验收标准（Definition of Done）

- [ ] `cargo fmt --all`、`cargo build`、`cargo test --workspace`、  
  `cargo clippy --workspace --all-targets -- -D warnings` 全绿，**零新告警**。
- [ ] 不影响 MySQL / MariaDB / SQLite / SQL Server / ODBC 现有功能与测试。
- [ ] 连接菜单里 PostgreSQL **可点**（不再灰显）且**只出现一次**，默认端口 5432，能测试连接与保存。
- [ ] 真机（或 Docker `postgres:17`）跑通：  
  列库 → 列 schema → 列表/视图 → 列列 → 分页取数（含过滤/排序）→ 增删改 →  
  执行任意 SQL（含多语句、`$$` 函数、`BEGIN…COMMIT` 事务脚本）→ 表设计器读改 → 视图列表/打开 →  
  函数列表/详情 → 角色列表。
- [ ] **强类型回归**：integer / boolean / numeric / timestamptz 列的 过滤 + 增删改 全部成功（R10）。
- [ ] **读路径回归**：带过滤（二进制路径）的分页里 `numeric` / `uuid` / `jsonb` / 数组显示正确文本；
  零行 `SELECT` 列标题仍在；打开视图分页不报 `ctid` 错（R14 / §3.8）。
- [ ] **schema 管理**：新建 / 删除空 schema 成功；删除非空 schema 报可读错误（P1-T11 / R15）。
- [ ] **app 层方言**：PG 上 新建函数 / 运行函数 / 运行过程 / 保存例程 / 保存视图 生成合法 SQL（P6-T5a）。
- [ ] 备份 → 还原 → 再插入新行不冲突（序列已重置，含 `GENERATED ALWAYS` 表、**空表**）。
- [ ] 新增 live 测试默认 `#[ignore]`，靠 `RUSTGRID_PG_*` 启用，并写入 `AGENTS.md`。
- [ ] `AGENTS.md` / `README.md` / `README_zh.md` 已更新（引擎列表、依赖、命令、out-of-scope 那句泛写）。
- [ ] `PLANNED_ENGINES` 中 PostgreSQL 占位已删除（并确认菜单无重复条目）。
- [ ] 导出的 `.sql` 在 PG 上可执行（P6-T4）；查询编辑器高亮 / EXPLAIN / 结果网格可编辑判定
      对 PG 双引号语法正确（P6-T5）；语句扫描器认美元引用（P6-T5b）。

---

## 12. 建议排期（里程碑）

| 里程碑 | 内容      | 可演示结果               |
| --- | ------- | ------------------- |
| M1  | P0 + P1 | 能连、能看表、能跑查询（只读可用）   |
| M2  | P2      | 网格可编辑、表可建改删         |
| M3  | P3      | 视图与函数/过程设计器可用       |
| M4  | P4 + P5 | 用户/权限 + 备份/还原       |
| M5  | P6      | 隧道、TLS、导出/解析方言、文档、全量校验（可发布） |

> 落地时以 `cargo check -p rustgrid-postgresql` 逐步迭代，每完成一个 Phase 跑一次  
> `cargo fmt` + `cargo build`；全部完成后跑全量 `test` + `clippy`。  
> **建议先把 §3.7（绑定 CAST）、§3.8（读路径解码）和 §3.4（服务端切分）三条在 P1 就钉死**——
> 它们会反向影响 P2 全部编辑任务与 P6 的 app 层方言改动，返工成本最高。

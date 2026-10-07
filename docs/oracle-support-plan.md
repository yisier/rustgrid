# Oracle 支持计划

> 本文是 RustGrid 新增 **Oracle**（原生 Rust thin 驱动）的实施方案与**详细任务清单**。
> 目标：在**不新造 UI** 的前提下，复用 `rustgrid-core` 的 `Driver` / `Connection` 抽象，
> 交付一个与 MySQL / MariaDB / SQLite / SQL Server / PostgreSQL 同级的原生引擎。
>
> 参考实现：`crates/rustgrid-postgresql`（最近的同级原生引擎，本文多处与之对照）、
> `crates/rustgrid-sqlserver`（**schema 一等公民**与非 sqlx 驱动的参考）、
> `crates/rustgrid-mysql`（最完整的 sqlx 实现）、`crates/rustgrid-odbc`（**同步阻塞驱动**如何塞进 async trait 的参考）。
> 相关文档：`docs/postgresql-support-plan.md`、`docs/odbc-driver.md`、`docs/sqlserver-driver.md`。
>
> ⚠️ **与 PostgreSQL 计划最大的不同**：PG 计划落地时 Rust 生态**没有**纯 Rust 的 Oracle 驱动，
> 所以 Oracle 一直是"走通用 ODBC 兜底"的定位（见 `docs/odbc-driver.md`、`README.md` 的引擎表）。
> **现在情况变了**：Oracle 官方发布了纯 Rust 的 thin 驱动 **`oracledb`（rust-oracledb）**，
> 无需 OCI / Instant Client。本计划据此把 Oracle 从"ODBC 兜底"提升为**原生引擎**。

---

## 1. 目标与范围

### 1.1 交付目标

| 能力  | 目标                                                                                             |
| --- | ---------------------------------------------------------------------------------------------- |
| 连接  | host/port(1521)/user/password/service（或 TNS 别名）、TCPS/钱包、SSH/SOCKS5/HTTP 隧道、连接/查询超时、只读会话、`init_sql` |
| 浏览  | 当前库/PDB、**schema（用户）列表**、表 / 视图列表、列信息、分页取数                                                        |
| 编辑  | 网格内增 / 删 / 改（主键优先，无主键退化为全列）、任意 SQL、**多语句 / PL/SQL 脚本**                                          |
| 表操作 | Drop / Empty / Truncate / Rename、表设计器（列 / 索引 / 外键）读写                                            |
| 视图  | 列表 / 详情 / 保存 / 删除                                                                               |
| 例程  | 函数 / 过程 / 包 / 触发器列表 / 详情 / 保存 / 删除                                                              |
| 用户  | 用户（schema）与角色列表 / 详情 / 保存 / 删除 / 重命名、系统与对象级权限                                                  |
| 备份  | `.rgbak` 备份 / 还原（表 + 视图 + 例程）                                                                   |
| 导出  | `.xlsx` / `.csv` / `.txt` 已引擎无关；`.sql` 需新增 Oracle 方言（见 P6-T4）                                     |

### 1.2 明确不做（沿用现有 out-of-scope）

- 不引入 Oracle 专属的新 UI 页面或新 trait 方法（除非抽象确实缺失，见 §10）。
- 不实现 Data Pump（`expdp`/`impdp`）、RMAN、物化视图刷新、分区维护、DB Link 管理、ASM 等长尾功能。
- **不做 `CREATE DATABASE`**：Oracle 建库是重量级 DBA 操作（需 `init.ora` 参数、非连接级），
  且表空间/数据文件无法映射到现有 `DatabaseEditorSpec`。`supports_database_management = false`，
  库管理入口隐藏（与 ODBC 驱动的取舍一致）。
- 不修改 `~/.cargo/registry` 下的任何上游 crate 源码。
- 不破坏 MySQL / MariaDB / SQLite / SQL Server / PostgreSQL / ODBC 的现有功能与测试。

---

## 2. 现状评估：为什么现在做

### 2.1 三条可选路线（**动手前必须拍板**）

| 方案                                    | 载体                                   | 需要外部依赖                | 能力上限                       | 结论          |
| ------------------------------------- | ------------------------------------ | --------------------- | -------------------------- | ----------- |
| **A 官方 thin 驱动（推荐）**                  | `oracledb`（rust-oracledb，Oracle 官方） | **无**（纯 Rust，无 Instant Client） | 高：SQL/PL-SQL/LOB/池/批量/加密网络 | **本计划采用**   |
| B OCI 封装                              | `sibyl`（或 `oracle`/`oracle-rs`）      | **Oracle Instant Client**（原生库） | 高                          | 兜底：A 卡壳时启用  |
| C ODBC 提级                             | 扩展现有 `rustgrid-odbc`                 | 系统 ODBC 驱动 + Oracle 客户端 | 中：受驱动质量限制，元数据弱             | 长期保留为回退路径   |

**选 A 的理由**：

1. **纯 Rust、零外部依赖**——完全符合仓库"ODBC 驱动管理器运行时加载、不引链接期原生依赖"的一贯取向
   （见 `docs/odbc-driver.md` 开头）。比 PostgreSQL 还干净（PG 至少要用 `ring` 的 C 编译器）。
2. **许可证兼容**：`UPL-1.0 OR Apache-2.0` 双许可，仓库本身是 Apache-2.0。
3. **工具链兼容**：MSRV **1.89**（workspace 是 **1.94**）、**edition 2024**、依赖链含 `rustls`/`chrono`/`uuid`
   ——`rustls 0.23`、`aws-lc-rs`、`webpki-roots` 已在 `Cargo.lock` 里（`rustls 0.23.44`），
   **不新增 TLS 原生编译负担**。
4. **能力覆盖本计划所需**：SQL/PL-SQL、LOB（CLOB/BLOB/NCLOB）、JSON、批量执行、**连接池**、语句缓存、加密网络。

**选 A 的代价（必须正视，见 §10 R1/R2）**：

- 目前是 **26.0.0-beta.4 预发布版**（2026-09-23），官方明示"API 与功能仍在变动"→ 必须**锁定精确版本**，
  并把它包在薄封装层后，降低升级面。
- **同步阻塞 API**（所有示例都是 `fn main()` + 无 `.await`）→ 需要 §3.2 的桥接方案。
- 连接串**只读 `tnsnames.ora`**，不读 `sqlnet.ora` / `oraaccess.xml`；SQL 语句**结尾不能带分号**。

> **决策点 D-A**：是否采用方案 A。若采纳，`rustgrid-odbc` 的 `odbc.engine = "oracle"` 分支（已存在，
> `connection.rs:532`）**保留不动**，作为"没装原生驱动时的备选"与回归对照。

### 2.2 抽象层现状（可直接复用，无需新造）

1. **菜单占位已存在。** `crates/rustgrid-app/src/app/toolbar.rs:12-13` 的 `PLANNED_ENGINES`
   已把 `("oracle", "Oracle", 30)` 列为灰显项；`toolbar.rs:318-323` 遍历占位时按 registry 过滤，
   **驱动注册后占位自动跳过**，不会出现两个 Oracle 条目（删除占位只是清理死代码）。
2. **schema 机制已通用。** 与 PG 计划 §2 完全一致：`tree/objects/sidebar/query` 全走
   `DriverCapability::Schemas`，Oracle 设 `supports_schemas = true` 即自动获得
   **库 → schema → 表/视图/函数** 的树结构，以及 `objects.rs` 的 `schema_of` / `object_display`
   （按首个 `.` 拆分 `schema.name`）。
3. **方言已数据驱动。** `DriverDialect`（`crates/rustgrid-core/src/dialect.rs`）已有
   `Mysql`/`SqlServer`/`Sqlite`/`Postgres`/`Generic`；`app/src/sql.rs` 已有
   `sqlparser_dialect()` 映射（`Postgres → PostgreSqlDialect`），`rustgrid-export` 已有
   `SqlDialect::Postgres`，`app/export.rs:771-777` 已按驱动选方言。**Oracle 只需各加一个变体**，
   不是 PG 计划里那种"从硬编码 MySQL 拆出来"的大改。
4. **同步阻塞驱动有先例。** `rustgrid-odbc` 的 `Connection` 实现全部是**同步调用直接写在 async fn 里**
   （`connection.rs:60-104`），证明"阻塞驱动 + async trait"这条路在仓库里走得通；
   但见 §3.2——Oracle 是**网络库**，直接阻塞 tokio worker 不可接受，需 `spawn_blocking`。
5. **`Connection` trait 已完整**（`crates/rustgrid-core/src/driver.rs:143-573`），
   含用户/例程/视图/权限/备份全部方法，默认实现报"unsupported"。
   驱动只需覆写真正支持的方法，**core 无需改动**（除 `dialect.rs` 加变体）。

### 2.3 与现有引擎的关键差异（本计划的难点来源）

| 差异                                                     | 影响                                                     | 应对                                                          |
| ------------------------------------------------------ | ------------------------------------------------------ | ----------------------------------------------------------- |
| **Oracle 实例 = 一个库**（PDB 体系下连接落在某个 PDB），没有"库列表"          | `list_databases` 语义与其他引擎不同；连接树顶层不该是空的                     | 返回**当前库/PDB 名**（`SYS_CONTEXT`），树保持 `库 → schema → 对象`（§3.3）      |
| **无跨库连接池需求**（一个连接可见本 PDB 全部 schema）                    | 不需要 PG 那种 `HashMap<db, Pool>`                           | **单连接池**即可（比 PG 简单，§3.1）                                     |
| **标识符默认折叠为大写**，`"users"` ≠ `USERS`                       | 引用/解析错一个大小写就 `ORA-00942`                                | 一律按**目录返回的原样名字**加双引号引用（§3.4）                                 |
| **无 `LIMIT`**，分页用 `OFFSET … FETCH`（12c+）                 | 分页 SQL 要换写法，且**强制要求 `ORDER BY`**                        | `ORDER BY` 主键 / `ROWID`，`OFFSET n ROWS FETCH NEXT m ROWS ONLY`（§3.5） |
| **无 `SHOW CREATE`**                                     | 表/视图/例程 DDL 要另找来源                                      | 一律 `DBMS_METADATA.GET_DDL`（§3.6，**比 PG 手工拼装省事**）              |
| **PL/SQL 块 `BEGIN … END;` 内含分号**，无 `$$` 引用               | 多语句脚本切分会被块内分号骗到；**驱动没有服务端多语句能力**                        | **必须客户端切分**（PG 可交给服务端，Oracle 不行，§3.7）                         |
| **`q'[…]'` 替代引用语法**                                    | 常规引号扫描器认不出，字符串里的 `;`/`'` 会切错                            | 切分器识别 `q'X…X'`（§3.7）                                         |
| **语句结尾不能带分号**（`oracledb` 限制）                            | 查询编辑器里带 `;` 的语句会直接报错                                    | 切分后**逐条剥掉行尾分号**再发送（§3.7）                                     |
| **`''` 就是 `NULL`**                                      | VARCHAR2 存不下空串；读写语义与 MySQL/PG 不同                        | 读 NULL→`CellValue::Null`；写 `Some("")` 等价于 NULL，文档注明（§3.9）        |
| **绑定用 `:1` / `:name`**，且 DATE/TIMESTAMP 隐式转换不可靠           | core 传来的值全是 `String`                                    | 按列类型**类型化绑定**或 `TO_DATE`/`TO_TIMESTAMP` 包裹（§3.8）              |
| **schema = user**（`CREATE USER` 即建 schema）                | `create_schema`/`drop_schema` 实为建/删用户                     | `CREATE USER … NO AUTHENTICATION` / `DROP USER`（§3.10）        |
| **例程含 package / trigger / type**，可重载                     | 名字与唯一性比 PG 复杂                                           | 名字带 schema；包内子程序用 `schema.package.name`；`kind` 由 `ALL_OBJECTS` 判（§3.5） |
| 类型体系独特：`NUMBER`/`VARCHAR2`/`DATE` 含时间/`CLOB`/`BLOB`/`RAW` | 类型映射表与字面量渲染要重写                                         | 见 §8 与 P5-T2                                                  |
| 无存储引擎、无 charset（改 NLS）、无事件调度器                          | `storage_engines`/`list_events` 返回空；库对话框字段语义变化            | `DatabaseEditorSpec::default()`；库管理整体关闭（§1.2）                   |
| **驱动同步阻塞**                                             | 直接跑在 tokio worker 上会阻塞整个 runtime                        | `spawn_blocking` + 共享 `Pool`（§3.2）                            |
| app 层例程模板/调用是 MySQL 形状（反引号 + `BEGIN…END`）                | Oracle 新建/运行函数会产出非法 SQL                                 | `routine_template_for` / `routine_call_sql_for` 按方言分派（P6-T5a）  |

---

## 3. 关键架构决策

> 以下决策请**在动手前确认**，它们决定 crate 的内部结构。

### 3.1 决策 A：单一连接池（不是 PG 的分片池）

Oracle 的连接绑定到**服务/PDB**，但**一个连接就能看到本 PDB 里所有 schema 的对象**——
不存在 PG"换库必须换连接"的问题。因此：

```rust
pub struct OracleConnection {
    /// 建池模板：host/port/service/user/password/TCPS 都烘进 connect string / PoolConfig。
    ///
    /// ⚠️ **`oracledb::Pool` 没有实现 `Clone`**（docs.rs 只有 `Send` + `Sync` + `Freeze` + `Unpin`），
    /// 所以必须套 `Arc`。`Arc<Pool>` 才是 `Send + Sync + Clone`，满足 `spawn_blocking` 的
    /// `FnOnce() -> R + Send + 'static` 约束。
    pool: Arc<oracledb::Pool>,
    /// 连接 profile 指定的默认 schema（用户），用于裸名回退。
    default_schema: String,
    /// 隧道句柄，随连接存活（`None` 表示直连）。
    _tunnel: Option<Tunnel>,
}
```

- `connect()` 里用 `oracledb::create_pool(PoolConfig)` 建池，`set_max_connections(5)`、
  `set_min_connections(1)`，并做**一次探活**（`SELECT 1 FROM dual`）让错误密码在连接时即暴露
  （与 SQL Server 的 `min_idle(1)` 同意图）。
- 每个 trait 方法 `pool.acquire()` 取一条连接，用完自动归还。
- `close()`：**拿不到 `&mut Pool`**（`Pool::close(&mut self)`，而 trait 是 `async fn close(&self)`，
  且并发下 `Arc` 不可能给出可变借用）→ **依赖 `Pool` 的 `Drop`**（docs 明确 Drop 会关闭池），
  `close()` 本身返回 `Ok(())` 并只丢弃句柄。若将来确实需要显式关闭，改为 `Mutex<Option<Pool>>`
  并接受"`acquire` 期间持有锁"的代价。
- **不需要** LRU 驱逐 / 按库分片（PG 的 R1 风险在 Oracle 不存在）。

> 注意：`oracledb` 的池类型叫 `Pool`（`oracledb::create_pool` + `Pool::acquire`），**不是** `ConnectionPool`。
>
> ⚠️ **`create_pool()` 收的是 `PoolConfig`，不是 `Config`。** `Config` 只给 `connect()` 建立**独立连接**用；
> `PoolConfig` 内部持有一个 `Config`，但只能读（`PoolConfig::connection_config() -> &Config`），
> 没有替换整个 `Config` 的入口。建池一律从 `PoolConfig::default()` 起链式设置。
> `PoolConfig` 是 builder，**部分 setter 返回 `Result`**：
> ```rust
> let config = PoolConfig::default()
>     .set_credentials(&user, &password)                     // -> Self
>     .set_connect_string(&connect_string)                   // -> Result<Self, Error>（会解析连接串）
>     .map_err(map_err)?
>     .set_min_connections(1)                                // -> Self
>     .set_max_connections(5)                                // -> Self
>     .set_wallet_location(wallet)                           // -> Self（TCPS/钱包，可选）
>     .set_wallet_password(pwd)?;                            // 可选
> let pool = Arc::new(oracledb::create_pool(config).map_err(map_err)?);
> ```
> 另：`PoolConfig` **没有** `set_tcp_connect_timeout`（只有 `set_ping_interval` / `set_ping_timeout`），
> 连接超时要另找途径（见 P6-T3 的"接线确认"）。

### 3.2 决策 B：同步阻塞 API 的桥接（**本计划最核心的工程决策**）

`oracledb` 是**同步阻塞** API，而 `Connection` trait 是 `async_trait`。三条路：

- **A（推荐）`spawn_blocking` + 共享 `Arc<Pool>`。** 每个 trait 方法体是一段
  `tokio::task::spawn_blocking(move || { let conn = pool.acquire()?; … })`，
  闭包内完成全部阻塞调用，返回**owned** 结果（`Vec<CellValue>`、`String` 等）。
  ```rust
  async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
      let pool = self.pool.clone();          // Arc<Pool>：Clone + Send + Sync
      let database = database.to_string();
      tokio::task::spawn_blocking(move || {
          let conn = pool.acquire().map_err(map_err)?;
          let cursor = conn.query(SQL, &[&database]).map_err(map_err)?;
          /* 组装 Vec<TableInfo> */
          Ok(tables)
      })
      .await
      .map_err(|e| Error::Query(format!("oracle worker panicked: {e}")))?
  }
  ```
  **前置校验（P0-T1）**：编译期断言 `Arc<oracledb::Pool>: Send + Sync + Clone`。
  上游事实已可从 docs.rs 确认——`oracledb::Pool` **只有 `Send + Sync`，没有 `Clone`**，
  `oracledb::Connection` 也只有 `Send + Sync`，**都没有 `Clone`**。所以：
  - `Pool` 必须套 `Arc`（`Arc<Pool>` 自动满足 `Send + Sync + Clone`）；
  - 连接**不能**跨方法复用，每个 `spawn_blocking` 闭包内 `acquire()` 一次、用完 drop 归还。
  → 闸门从"`Pool` 是否 `Clone`"改为"**`Arc<Pool>` + 闭包内 `acquire()` 是否真能跑通**"。
- **B（兜底）每连接一个专属线程的 actor。** 连接对象在线程内创建、在线程内使用，
  通过 `mpsc`/`oneshot` 收发请求；不需要 `Send`，但代码量更大、生命周期管理更繁。
- **C（不推荐）直接阻塞调用线程**（照抄 ODBC）。**网络库不能用这条**：一次慢查询会占死一个 tokio
  worker，多连接并发时 runtime 直接饿死。

> **决策点 D-B**：A 还是 B。**先做 P0-T1 的探针**（真连一次库，跑通 `acquire` + `query`）。
> 以上文档事实已把 `Clone` 这条不确定性排除掉了，所以 B 只在"阻塞线程里 `acquire` 抛
> 非 `Send` 相关的运行时错误"时才需要。不要在没有探针前把整 crate 写成 A 的形状。

### 3.2.1 `spawn_blocking` 与"流式"方法的冲突（`stream_table_rows`）

`stream_table_rows`（`core/driver.rs:325`）要求**边收边渲染、不整表缓冲**，而 `oracledb` 的
`Cursor` 是**同步 `Iterator<Item = Result<Row, Error>>`**。方案 A 下不能简单地在闭包里
`collect()` 回 `Vec`——那等于把整表读进内存，把 R5 的风险坐实。

可行解：`spawn_blocking` + channel，闭包内边迭代边发：

```rust
let (tx, rx) = tokio::sync::mpsc::channel(64);
let pool = self.pool.clone();
tokio::task::spawn_blocking(move || {
    let conn = pool.acquire()?;
    let cursor = conn.query(&sql, &[])?;
    for row in cursor {                       // 同步 Iterator
        let rendered = render_row(row?)?;     // 阻塞线程内完成解码 / 格式化
        if tx.blocking_send(rendered).is_err() { break; }   // 消费端已丢弃
    }
    Ok::<_, Error>(())
});
// async 侧：`while let Some(item) = rx.recv().await { … }`
```

- 用 `tokio::sync::mpsc::Sender::blocking_send`（阻塞线程里不能用 `.await`）。
- `break` 语义：接收端 drop 时 `send` 失败 → 停止取数（等价取消）。
- 若 Phase 5 实测太麻烦，可接受"整表读进阻塞线程"作为**临时实现**，但必须在文档注明
  （大表内存风险），并留 TODO。

> 同理 `backup_object_metadata` 里的 DDL 与行数据也走同一条路；`execute_query_many` 不受影响
> （一次闭包 = 一次 `acquire` = 顺序执行，天然在同一条连接上）。

### 3.3 决策 C：连接模型——单库 + schema 一等公民

- `supports_schemas = true`（与 SQL Server / PG 一致）。
- `list_databases()`：Oracle 没有"库列表"。返回**当前库/PDB** 一个条目，名字取
  `SELECT SYS_CONTEXT('USERENV','DB_NAME') FROM dual`（多租户下可用 `'CON_NAME'` 取 PDB 名）。
  这样连接树保持 `库 → schema → 对象` 的形状，其余代码无需特判。
- `list_schemas()`：`SELECT username FROM all_users ORDER BY username`。
  可选：默认隐藏 Oracle 维护账号（`SYS`/`SYSTEM`/`XDB`/`MDSYS`/`CTXSYS`/… 一小组），
  留到 Phase 6 做成可配置（对齐 PG §3.3 的做法）。
- **`fetch_page`/`columns` 等拿到的 `database` 参数在 Oracle 下是"当前库名"，不参与限定**；
  对象限定只用 `schema`。

### 3.4 决策 D：标识符大小写与引用

- Oracle 把**未加引号的标识符折叠为大写**存储。所以 `"users"` 与 `USERS` 是两个对象。
- **一律引用目录返回的原样名字**：`list_tables` 从 `ALL_TABLES` 拿到的 `table_name` 本就是大写，
  驱动生成 SQL 时 `quote("USERS") → "USERS"`，**永远不要 `to_lowercase()`**。
- `quote_identifier(s)`：`"..."`，内部 `"` → `""`。
- `qualify(schema, name)`：`"SCHEMA"."NAME"`（schema 省略时用 `default_schema`）。
- `resolve_object(input) -> (Option<schema>, name)`：按 `.` 拆分，**最多 3 段**
  （`schema.package.name`，例程才用第三段；表名最多 2 段）。无 schema 前缀时回退 `default_schema`。
- 占位符 `:1`、`:2`（1-based），与 `oracledb` 的绑定风格一致。

### 3.5 决策 E：分页 `OFFSET … FETCH` + `ROWID` 稳定排序

```sql
SELECT * FROM "S"."T" {WHERE} ORDER BY <key> OFFSET :1 ROWS FETCH NEXT :2 ROWS ONLY
```

- **Oracle 12c+ 才支持 `OFFSET/FETCH`**（`oracledb` 官方支持 12c/18/19/21/26；文档另处写 19+，
  落地时按 12c 为下限、在文档注明）。**11g 及更早不支持**——不在本计划范围（如需支持，退化为
  `ROWNUM` 子查询，作为 Phase 6 可选增强）。
- **`OFFSET/FETCH` 强制要求 `ORDER BY`**（这点比 PG 更硬）。稳定排序：
  - 有主键 → `ORDER BY "PK"…`；
  - 无主键的**表** → `ORDER BY ROWID`（Oracle 的物理行标识，等价 PG 的 `ctid`）；
  - **视图 / 无 ROWID 的对象** → 退化为 `ORDER BY 1`（不稳定，文档注明），
    与 PG"视图无 `ctid`"是同一类限制。
  - ⚠️ **`ROWID` 也不是万能的**：**索引组织表（IOT）没有 `ROWID`**，
    某些对象（全局临时表、含 `ROWID` 伪列受限的视图、`XMLTYPE` 表）同样拿不到，
    会报 `ORA-01445` / `ORA-00904`。落地时按"取 `ORDER BY` 键"顺序尝试：
    主键 → `ROWID`（先 `SELECT ROWID FROM … FETCH FIRST 1 ROW ONLY` 探一次，失败即跳过）
    → 首个可排序的普通列 → `ORDER BY 1`。别把 `ROWID` 当成无主键表的无条件答案。
- `total_rows`：`SELECT count(*) FROM … {WHERE}`（只读计数，不做类型解码）。大表慢，文档注明。

### 3.6 决策 F：DDL 一律走 `DBMS_METADATA.GET_DDL`

Oracle 没有 `SHOW CREATE`，但**有比 PG 手工拼装更省事的官方接口**：

```sql
SELECT DBMS_METADATA.GET_DDL('TABLE', 'T', 'S') FROM dual
```

- 对象类型串：`'TABLE'`/`'VIEW'`/`'FUNCTION'`/`'PROCEDURE'`/`'PACKAGE'`/`'PACKAGE BODY'`/
  `'TRIGGER'`/`'INDEX'`/`'SEQUENCE'`/`'TYPE'`。
- 返回 **CLOB**，按文本读入 `String`，用于 `object_ddl` / `backup_object_metadata` / `view_details` /
  `routine_details`。
- **DDL 净化**：默认输出会带 `SEGMENT ATTRIBUTES`/`TABLESPACE`/`STORAGE` 等 DBA 细节。
  建议在 `init_sql` 或每次取 DDL 前设置会话转换参数，得到更干净的脚本：
  ```sql
  BEGIN
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM,'STORAGE',FALSE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM,'TABLESPACE',FALSE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM,'SEGMENT_ATTRIBUTES',FALSE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM,'PRETTY',TRUE);
  END;
  ```
- **表设计器**（结构化编辑）不能只靠 DDL 文本，仍需读目录：
  `ALL_TAB_COLUMNS` / `ALL_CONSTRAINTS` / `ALL_CONS_COLUMNS` / `ALL_INDEXES` / `ALL_IND_COLUMNS` /
  `ALL_TAB_COMMENTS` / `ALL_COL_COMMENTS`（见 P2-T10）。
- `DBMS_METADATA` 的 `EXECUTE` 权限默认授予 `PUBLIC`，普通用户可用；若被回收，错误透传。

### 3.7 决策 G：多语句 / PL/SQL 脚本**必须客户端切分**

PG 能靠服务端 `fetch_many` 解析多语句；**Oracle 不行**——`oracledb` 一次 `execute`/`query` 只跑一条语句。
因此切分器是**执行路径的刚需**（不只是"填 `statement` 文本"）。切分器必须识别：

1. **单引号字符串** `'...'`，转义是 `''`；
2. **替代引用** `q'X…X'`：`X` 可为 `[ ] { } ( ) < >` 或任意定界符（`q'!abc!'`），
   括号类需配对（`q'[a]b]'` 到第一个 `]` 结束）。**忽略它会把字符串里的 `;` 当分隔**；
3. **双引号标识符** `"..."`（`""` 转义）；
4. `--` 行注释、`/* */` 块注释（不嵌套）；
5. **PL/SQL 块**：以 `BEGIN`/`DECLARE` 开头，或以 `CREATE [OR REPLACE] PROCEDURE|FUNCTION|PACKAGE
   [BODY]|TRIGGER|TYPE [BODY]` 开头，其**内部含多个 `;`**，必须以 `END;` 收尾算**一条语句**。
   实现：在外层扫描时对块做 `BEGIN…END` 计数（`CASE … END`、`IF … END IF` 也含 `END`，
   计数要能容忍），或先按"起始关键字"判定为块、再扫描到与之配对的收尾 `END;`；
6. **SQL\*Plus 分隔符**：单独一行的 `/` 视为语句结束（编辑器里常见），发送前去掉。

**发送前处理**：

- 逐条**剥掉行尾分号**（`oracledb` 拒绝 `;`）——`SELECT 1;` → `SELECT 1`。
- 整段脚本在**同一条池连接**上顺序执行，最后 `commit()`；`execute_query_many` 按语句边界切结果集。
- **切分失败（不平衡）时整段发送**（会报错但不丢数据），与 PG/SQL Server 的兜底一致。

> ⚠️ **不要把 `IF … END IF;` / `CASE … END;` 当成块边界**：它们只在 PL/SQL 块**内部**出现，
> 只要外层已经识别出"这是个 PL/SQL 块"，就一直扫到块收尾即可，不必单独处理它们。

### 3.8 决策 H：绑定参数 `:n` 与类型

core 交给驱动的值**全是 `String`**（`RowUpdate.set` / `keys`、`RowInsert.values`、
`FilterCondition.value` 都是 `Option<String>`）。Oracle 对 `VARCHAR2 ↔ NUMBER` 有隐式转换，
但对 **DATE/TIMESTAMP 的隐式转换依赖 `NLS_DATE_FORMAT`**，很脆弱。两个方案：

- **A（推荐）按列类型做"类型化绑定"**：`oracledb` 暴露 `OracleNumber` / `OracleTimestamp` /
  `OracleDate` 等类型。生成 SQL 时按列类型把 `String` 解析成对应绑定值：
  `:1` 绑 `OracleNumber`（NUMBER 列）/ `OracleTimestamp`（TIMESTAMP 列）/ 文本（字符列）。
  好处：**不用改 SQL 文本**，绕开 `NLS_DATE_FORMAT`，无注入面。代价：要写解析器 + 类型回退。
- **B 文本绑定 + 函数包裹**：全部按文本绑，DATE 列用 `TO_DATE(:1,'YYYY-MM-DD HH24:MI:SS')`、
  TIMESTAMP 用 `TO_TIMESTAMP(...)`（PG 计划 `CAST` 的 Oracle 版）。实现简单，但要按列类型重写 SQL。
- `None` 仍走 `IS NULL`，**绝不绑定空串**（Oracle 下空串即 NULL，见 §3.9）。

> **决策点 D-H**：A 还是 B。**先做 P0-T1 的类型探查**（确认 `oracledb` 的绑定值类型 API），
> 再定。两条路都**不改 core**。

### 3.9 决策 I：空串 = NULL 的读写语义

Oracle 里 `''` **就是 NULL**，VARCHAR2 无法保存"空字符串"。

- **读**：NULL → `CellValue::Null`（**不是** `CellValue::Text("")`），让网格显示空而非 `''`。
- **写**：`Some("")` 绑定后 Oracle 存成 NULL——**不可逆**。文档明确写出这条差异；
  网格里"清空单元格"在 Oracle 上等价于"置 NULL"，不做特殊提示（与其他引擎行为对齐到"结果是 NULL"）。
- 导出 `.sql` 字面量时，`CellValue::Text("")` 应渲染成 `NULL`（而非 `''`），否则回放语义不一致。

### 3.10 决策 J：schema 管理 = 用户管理

Oracle 的 schema 就是用户。`Connection::create_schema` / `drop_schema` 的签名里**没有密码**，
而 `CREATE USER` 需要认证方式。方案：

- **`create_schema(db, s)`** → `CREATE USER "S" NO AUTHENTICATION`（Oracle 12c+ 允许建"无认证用户"，
  即纯 schema，不产生登录凭据）。若目标库版本不支持 `NO AUTHENTICATION`，报可读错误并提示去 Users 页建用户。
- **`drop_schema(db, s)`** → `DROP USER "S"`（**不带 `CASCADE`**）。用户拥有对象时 Oracle 会报
  `ORA-01922`（非空 schema 不可删），正好匹配 trait 文档"引擎拒绝删除非空 schema"的约定，错误透传。
- 这套映射**不引入新 trait 方法**，与 SQL Server/PG 的"新建/删除模式"按钮复用同一 UI。

---

## 4. 交付物清单（文件级）

```
crates/rustgrid-oracle/
├── Cargo.toml                     # 新增 crate（含 [dev-dependencies] tokio）
├── src/
│   ├── lib.rs                     # pub use driver::OracleDriver; pub use connection::OracleConnection;
│   ├── driver.rs                  # OracleDriver: id/descriptor/connect/池/TCPS/错误映射
│   ├── connection.rs              # impl Connection for OracleConnection（spawn_blocking + 池 + 全部方法）
│   ├── helpers.rs                 # 引用/限定名/:n 占位/过滤树翻译/脚本切分(q'[]'、PL/SQL 块)/decode_cell + #[cfg(test)] 单测
│   ├── schema.rs                  # 表设计器：ALL_* 目录读写 + CREATE/ALTER TABLE 生成
│   ├── routine.rs                 # 例程（函数/过程/包/触发器）目录与 DBMS_METADATA.GET_DDL
│   ├── view.rs                    # 视图目录与 DBMS_METADATA
│   ├── user.rs                    # 用户/角色目录与 GRANT/REVOKE 生成
│   └── backup.rs                  # ObjectDump 组装 + 行元组字面量渲染 + restore
└── tests/
    └── live_oracle.rs             # #[ignore] 真机集成测试，靠 RUSTGRID_ORACLE_* 环境变量

crates/rustgrid-app/assets/icons/oracle.svg    # 品牌图标（红色，Brand(0xC74634)）

修改：
- Cargo.toml                                        # members + workspace.dependencies + oracledb = "=26.0.0-beta.4"
- crates/rustgrid-app/Cargo.toml                     # 依赖 rustgrid-oracle
- crates/rustgrid-app/src/main.rs                    # BuiltinDriverSource 注册 OracleDriver
- crates/rustgrid-app/src/assets.rs                  # 注册 icons/oracle.svg
- crates/rustgrid-app/src/app/toolbar.rs             # 删除 PLANNED_ENGINES 的 oracle 占位（order 30）
- crates/rustgrid-core/src/dialect.rs                # 新增 DriverDialect::Oracle + ORACLE_FUNCTIONS 表
- crates/rustgrid-export/src/lib.rs                  # SqlDialect 新增 Oracle 变体（P6-T4）
- crates/rustgrid-app/src/sql.rs                     # sqlparser_dialect 映射 Oracle→OracleDialect（P6-T5）；语句扫描器认 q'[]' 与 PL/SQL 块（P6-T5b）
- crates/rustgrid-app/src/app/routine.rs             # 例程模板/调用 SQL 按方言生成（P6-T5a）
- crates/rustgrid-app/src/app/export.rs              # 按驱动选 SqlDialect（P6-T4，现只有 Postgres/MySql 两分支）
- AGENTS.md / README.md / README_zh.md               # 引擎列表、命令、out-of-scope（Oracle 从 "via ODBC" 移到 Supported）
- docs/oracle-driver.md                              # 交付后补一份"实现说明"（仿 odbc-driver.md）
```

> *注*：① 纯函数单测按仓库惯例放在**各个 `src/*.rs` 的 `#[cfg(test)] mod tests`** 里
> （参考 `rustgrid-sqlserver/src/helpers.rs:528`、`rustgrid-postgresql/src/helpers.rs`），
> **不要放 `tests/`**——`tests/` 下每个文件是独立二进制，链接不到 crate 私有的 `helpers` 模块。
> ② `crates/rustgrid-core/src/lib.rs` **无需改动**：`pub use dialect::DriverDialect` 已存在，
> 新增枚举变体自动可用（PG 已证明这条路）。

---

## 5. 分阶段总览

| 阶段     | 主题       | 产出                                    | 依赖    |
| ------ | -------- | ------------------------------------- | ----- |
| **P0** | 脚手架与骨架   | crate 能编译、能连、菜单可见、图标就位、**`Arc<Pool>` + `spawn_blocking` 探针通过**  | —     |
| **P1** | 只读浏览     | 库 / schema / 表 / 视图 / 列 / 分页 / 任意 SQL | P0    |
| **P2** | 编辑与表 DDL | 增删改、Drop/Empty/Truncate/Rename、表设计器   | P1    |
| **P3** | 视图与例程    | 视图列表/详情/保存/删除；函数/过程/包/触发器             | P2    |
| **P4** | 用户与权限    | 用户/角色列表/详情/保存/删除/重命名、对象权限                | P2    |
| **P5** | 备份与还原    | `.rgbak` 备份/还原（表 + 视图 + 例程）           | P3    |
| **P6** | 收尾与对等    | 隧道、TCPS/钱包、导出/解析方言、i18n、文档          | P1–P5 |

每个阶段结束都要求：`cargo fmt --all` + `cargo build` + `cargo clippy --workspace --all-targets -- -D warnings`
全绿，且**不引入新告警**（当前仓库零告警）。

---

## 6. 详细任务清单

> 勾选框用于落地时跟踪。每条任务都给出**文件**、**要点**与**验收**。

### Phase 0 — 脚手架与骨架

- [ ] **P0-T1 `oracledb` 可行性与 `Arc<Pool>` + `spawn_blocking` 探针（先于一切编码）。**
  - 建一个**临时最小 bin**（建议 `crates/rustgrid-oracle/examples/probe.rs`，探完即删）
    验证：`oracledb = "=26.0.0-beta.4"` 能在本机（Windows + stable 1.94 + edition 2024）**编译通过**。
  - **静态断言对象是 `Arc<oracledb::Pool>`，不是 `Pool`**：
    ```rust
    fn assert_bridge<T: Send + Sync + Clone + 'static>() {}
    fn _probe() {
        assert_bridge::<std::sync::Arc<oracledb::Pool>>();
        // oracledb::Pool 只有 Send + Sync（无 Clone）；oracledb::Connection 只有 Send + Sync（无 Clone）
        fn assert_send<T: Send>() {}
        assert_send::<oracledb::Pool>();
        assert_send::<oracledb::Connection>();
    }
    ```
    > 上游事实（docs.rs 26.0.0-beta.4，已核实）：`Pool` 实现 `Send` + `Sync` + `Freeze` + `Unpin`，
    > **未实现 `Clone`**；`Connection` 实现 `Send` + `Sync`，**未实现 `Clone`**。
    > 所以 §3.1 的字段类型是 `Arc<Pool>`，§3.2 方案 A 的闭包里每次 `acquire()` 一条新连接。
  - 探针必须真跑一次：`Arc::new(create_pool(cfg))` → `spawn_blocking(Arc::clone)` 里
    `pool.acquire()?` → `conn.query("SELECT 1 FROM dual", &[])?` → 取一行 → `commit`。
    **确认阻塞线程里 `acquire` 没有非 `Send` 相关的运行时限制**。
  - 顺带钉死这些签名（与 docs.rs 核对过，落地时以实拉版本为准）：
    `create_pool(PoolConfig) -> Pool`、`Pool::acquire(&self) -> Result<Connection>`、
    `Pool::close(&mut self)`、`Connection::query(&self, sql, &[&dyn ToDbValue]) -> Cursor`、
    `Connection::execute(&self, sql, &[&dyn ToDbValue]) -> ExecResult`、
    `execute_batch(sql, impl Into<BindParameters>)`（**不是 `execute_many`**）、
    `Connection::statement(sql) -> StatementBuilder`（**需再 `.build()?` 才是 `Statement`**）、
    `commit` / `rollback` / `ping`。
  - 验证依赖链没有引入**新的原生编译需求**（`rustls` 0.23.44 已在 `Cargo.lock` 中）。
  - *验收*：探针能连上（或明确失败于网络/认证）一个真库；静态断言通过；决策点 D-B/D-H 有结论。
  - ⚠️ 若 crate 无法编译、或阻塞线程里 `acquire` 被拒 → **转方案 B（sibyl/OCI）或 C（ODBC 提级）**，
    本计划后续 Phase 需重估。（仅"`Pool` 不是 `Clone`"**不算**失败——用 `Arc` 即可。）
- [ ] **P0-T2 新增 workspace 成员。**
  改根 `Cargo.toml`：`members` 加 `"crates/rustgrid-oracle"`；
  `[workspace.dependencies]` 加 `rustgrid-oracle = { path = "crates/rustgrid-oracle" }`
  与 `oracledb = "=26.0.0-beta.4"`（**锁定精确版本**，beta 期不接受浮动）。
  *验收*：`cargo metadata` 通过。
- [ ] **P0-T3 建 `crates/rustgrid-oracle/Cargo.toml`。**
  ```toml
  [dependencies]
  rustgrid-core.workspace = true
  rustgrid-tunnel.workspace = true     # 隧道（与 PG 共用，Phase 6 接线）
  async-trait.workspace = true
  chrono.workspace = true
  tokio.workspace = true
  oracledb.workspace = true

  [dev-dependencies]
  tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
  ```
  *验收*：`cargo check -p rustgrid-oracle` 通过（此时 lib.rs 可为空）。
- [ ] **P0-T4 `driver.rs`：`OracleDriver` 基本实现。**
  - `id() = "oracle"`、`display_name() = "Oracle"`、`default_port() = 1521`、`is_file_based() = false`。
  - `supports_database_management() = false`（§1.2）、`supports_users() = true`、
    `supports_routines() = true`、`supports_schemas() = true`。
  - `database_editor()`：`DatabaseEditorSpec::default()`（全 false，库对话框不显示；库管理已关）。
  - `descriptor()`：
    ```rust
    DriverDescriptor {
        id: DriverId::new("oracle"),
        display_name: "Oracle".to_string(),
        default_port: 1521,
        is_file_based: false,
        icon: "icons/oracle.svg",
        icon_style: DriverIconStyle::Brand(0xC74634), // Oracle 品牌红
        capabilities: DriverCapabilities::none()
            .with(DriverCapability::Users)
            .with(DriverCapability::Routines)
            .with(DriverCapability::Schemas),          // 无 DatabaseManagement
        database_editor: self.database_editor(),
        connection_form: Default::default(),
        order: 30, // 与 PLANNED_ENGINES 的槽位一致
    }
    ```
  - `dialect() = DriverDialect::Oracle`（需先加变体，见 P0-T6）。
  - `connect()`：用 `ConnectionConfig` 拼 **`oracledb::PoolConfig`**（不是 `Config`——`Config`
    只给 `connect()` 独立连接用）：
    ```rust
    let config = PoolConfig::default()
        .set_credentials(&config.username, password.as_deref().unwrap_or(""))
        .set_connect_string(&connect_string)?      // "host:port/service" 或 TNS 别名；返回 Result
        .set_min_connections(1)
        .set_max_connections(5);
    // TCPS/钱包（Phase 6）：.set_wallet_location(...) / .set_wallet_password(...)
    let pool = Arc::new(oracledb::create_pool(config).map_err(map_err)?);
    ```
    service 取自 `database`/`options`；建池后 **`SELECT 1 FROM dual`** 探活；
    返回 `Box::new(OracleConnection::new(pool, default_schema, tunnel))`。
  - `map_connect_error`：把 `ORA-01017`（invalid username/password）/`ORA-28000`（account locked）
    映射为 `Error::Authentication`（触发密码提示）；其余 → `Error::Connection`。
    *验收*：`cargo check -p rustgrid-oracle` 通过；错误密码给出认证错误而非通用连接错误。
- [ ] **P0-T5 注册驱动 + 图标 + 菜单占位。**
  - `crates/rustgrid-app/Cargo.toml` 加 `rustgrid-oracle.workspace = true`。
  - `main.rs`：在 `BuiltinDriverSource::new()` 链上加
    `.with(Arc::new(rustgrid_oracle::OracleDriver::new()))`（放在 `PostgresDriver` 之后、
    `#[cfg(feature = "driver-odbc")]` 那次重绑定之前）。
  - 新增 `assets/icons/oracle.svg`（单色可染色，参照 `postgresql.svg`/`sqlserver.svg`），
    在 `assets.rs` 的 `match` 里注册 `"icons/oracle.svg"`。
  - `toolbar.rs` 的 `PLANNED_ENGINES`：删掉 `("oracle", "Oracle", 30)`，数组长度 2→1。
    *验收*：启动 app，**连接**菜单里 Oracle 可点（非灰显）且**只出现一次**，其余占位（MongoDB）仍灰显。
- [ ] **P0-T6 新增方言 `DriverDialect::Oracle`。**
  `crates/rustgrid-core/src/dialect.rs`：加变体 `Oracle`；`builtin_functions()` 接 `ORACLE_FUNCTIONS`
  （新增常量：`NVL`/`NVL2`/`DECODE`/`COALESCE`/`NULLIF`/`SYSDATE`/`SYSTIMESTAMP`/`TRUNC`/`ROUND`/
  `TO_CHAR`/`TO_DATE`/`TO_NUMBER`/`TO_TIMESTAMP`/`ADD_MONTHS`/`MONTHS_BETWEEN`/`LAST_DAY`/`NEXT_DAY`/
  `INSTR`/`SUBSTR`/`LENGTH`/`UPPER`/`LOWER`/`TRIM`/`LPAD`/`RPAD`/`REPLACE`/`REGEXP_LIKE`/`REGEXP_REPLACE`/
  `REGEXP_SUBSTR`/`LISTAGG`/`ROW_NUMBER`/`RANK`/`DENSE_RANK`/`LAG`/`LEAD`/`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/
  `CAST`/`EXTRACT`/`GREATEST`/`LEAST`/`SYS_GUID`/`DBMS_RANDOM`…）。
  更新 `Match` 里的穷尽分支。
  *验收*：`cargo check --workspace` 通过；查询编辑器补全能提示 Oracle 函数。

### Phase 1 — 只读浏览（先把"能连、能看、能查"打通）

- [ ] **P1-T1 `connection.rs` 骨架 + 池 + `spawn_blocking` 辅助。**
  实现 §3.1 的 `OracleConnection`（`pool` + `default_schema` + `_tunnel`），
  写一个 `fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T> + Send) -> impl Future<Output = Result<T>>`
  之类的辅助，把 §3.2 的 `spawn_blocking` 样板收敛到一处；`driver_id()` 返回 `"oracle"`。
  *验收*：能 `connect()` 到一个真库，`with_conn` 可跑 `SELECT 1 FROM dual`。
- [ ] **P1-T2 `helpers.rs`：引用 / 限定名 / 占位符。**
  - `quote_identifier(s)`：`"..."`，内部 `"` → `""`（**不做大小写折叠**）。
  - `qualify(schema, name)`：`"S"."N"`。
  - `resolve_object(default_schema, input) -> (Option<schema>, name)`：按 `.` 拆，最多 3 段。
  - `placeholder(i) -> ":i"`（1-based）。
  - `strip_trailing_semicolon(stmt) -> &str`（§3.7 必需）。
    *验收*：`#[cfg(test)]` 单测覆盖带引号、带点、**全大写/混合大小写**的标识符。
- [ ] **P1-T3 `list_databases`。**
  `SELECT SYS_CONTEXT('USERENV','DB_NAME') FROM dual`（多租户可加 `'CON_NAME'` 取 PDB）。
  *验收*：连接树顶层出现且仅出现当前库/PDB 一个节点。
- [ ] **P1-T4 `list_schemas`。**
  `SELECT username FROM all_users ORDER BY username`（可选隐藏维护账号，见 §3.3）。
  *验收*：树里库下出现 schema 层。
- [ ] **P1-T5 `list_tables`。**
  ```sql
  SELECT t.owner, t.table_name
  FROM all_tables t
  UNION ALL
  SELECT v.owner, v.view_name FROM all_views v
  ORDER BY 1, 2
  ```
  或统一走 `all_objects`（`object_type IN ('TABLE','VIEW')`）以便把物化视图也带上
  （`'MATERIALIZED VIEW'`）。名字返回 `OWNER.NAME`（**原样大写**）；`updatable` 另查
  `all_updatable_columns` / `all_views`（尽力，拿不到给 `false`）。
  *验收*：表 / 视图两栏都正确，名字带 schema 前缀；普通用户连接下表不缺失。
- [ ] **P1-T6 `columns`。**
  ```sql
  SELECT c.column_name, c.data_type, c.data_length, c.data_precision, c.data_scale,
         c.nullable, c.data_default, c.char_length, c.char_used,
         cc.comments
  FROM all_tab_columns c
  LEFT JOIN all_col_comments cc
    ON cc.owner = c.owner AND cc.table_name = c.table_name AND cc.column_name = c.column_name
  WHERE c.owner = :1 AND c.table_name = :2
  ORDER BY c.column_id
  ```
  主键来自 `all_constraints`（`constraint_type='P'`）+ `all_cons_columns`。
  **完整类型串（含 `(p,s)`）后面要给 §3.8 的类型化绑定用，务必原样保留。**
  *验收*：列名/类型/可空/主键/注释正确；`NUMBER(10,2)`、`VARCHAR2(50 CHAR)` 保留精度/长度。
- [ ] **P1-T7 `fetch_page` + 过滤/排序翻译。**
  - `helpers::filter_clause(&[FilterNode], types) -> (String, Vec<Bind>)`：递归过滤树
    （与 SQL Server/PG 的 `filter_clause` 同构），产出 `WHERE` 片段 + 绑定序列，
    **绑定顺序与 `:n` 严格一致**。
  - `Contains` 用 `LIKE`（Oracle 的 `LIKE` 大小写敏感）；`ILIKE` 无对应 → 需要不区分大小写时
    用 `UPPER(col) LIKE UPPER(:n)`（与 MySQL 行为对齐后决定，**钉死**）。
  - 绑定按 §3.8 方案处理（类型化或 `TO_DATE` 包裹）。
  - `order_clause`：`PageRequest.order_by` → `ORDER BY "C" ASC/DESC`。
  - 分页 SQL（§3.5）：`SELECT * FROM "S"."T" {WHERE} ORDER BY <key> OFFSET :n ROWS FETCH NEXT :m ROWS ONLY`。
  - **稳定分页**：有主键按主键；无主键表 `ORDER BY ROWID`（**注意 IOT / 全局临时表没有 `ROWID`**，
    见 §3.5 的四级回退）；视图/无 ROWID 退化为 `ORDER BY 1`。
  - `total_rows`：`SELECT count(*) FROM … {WHERE}`。大表慢，文档注明。
    *验收*：翻页顺序稳定；过滤树（含嵌套组、`IN`、`BETWEEN`、`IS NULL`）结果正确；
    **在 NUMBER / DATE / TIMESTAMP 列上过滤都不报类型错误**。
- [ ] **P1-T8 `decode_cell`：类型映射。**
  按 §8 的表映射。要点：
  1. `NUMBER`：用 `OracleNumber` 的**精确文本**（`CellValue::Text`）或按精度/标度转 `Int`/`Float`
     ——**二选一并钉死**（§8 给了推荐：整数标度→`Int`，否则→`Text` 精确串，**绝不用 f64 丢精度**）；
  2. `VARCHAR2`/`NVARCHAR2`/`CHAR`/`NCHAR`/`CLOB`/`NCLOB`/`LONG` → `Text`；
  3. `DATE`/`TIMESTAMP*`/`INTERVAL*` → `Text`（`OracleTimestamp` 格式化；`DATE` 含时间部分）；
  4. `RAW`/`LONG RAW`/`BLOB` → `Bytes`；
  5. `ROWID`/`UROWID`/`XMLTYPE`/JSON/`VECTOR` → `Text`；
  6. `BOOLEAN`（23c）→ `Bool`；
  7. 未知类型 → 文本兜底；取不到文本时回落 `Bytes`（十六进制展示），文档注明。
  - **LOB 注意**：⚠️ `StatementBuilder::fetch_lobs()` 的语义是**让 LOB 以 locator 方式取**
    （拿到句柄而不是内容）——**默认才是内联取内容**。所以要"读到内容"就**不要**调 `fetch_lobs()`；
    要"延迟 / 分段读大 LOB"才调它，且之后必须自己解析 locator。这点与多数驱动的直觉相反，
    落地时先写一个最小用例确认默认行为。
    大 CLOB 截断策略要明确（网格只预览前 N KB，全文另取）。
    *验收*：`#[cfg(test)]` 单测覆盖 §8 映射表；`NUMBER` 大数不丢精度；CLOB/BLOB 能正确显示/下载。
- [ ] **P1-T9 `execute_query` / `execute_query_many`（§3.7 的核心）。**
  - **必须客户端切分**：`helpers::split_statements` 识别单引号串、`q'X…X'`、双引号标识符、
    `--`/`/* */`、`/` 分隔符，以及 **`BEGIN…END;` / `CREATE PROCEDURE…END;` 等 PL/SQL 块**。
  - 每条语句**剥掉行尾分号**后经 `pool.acquire()` 的**同一条连接**顺序执行；
    DML 结束 `commit()`。
  - `execute_query_many` 返回**每个产生结果集的语句**一个 `QueryResult`（按顺序）；
    `SELECT` 零行也要返回。列元数据的取法（上游已核实）：
    - 首选 **`Cursor::columns() -> &[Metadata]`**——零行 `SELECT` 也能拿到列信息，最简单；
      游标不指向查询时返回空切片。
    - `Statement::out_metadata()` 也可用，但**语句完全解析前返回空切片**；
      要强制解析得 `ensure_fully_parsed(&mut self)`，而它对 **DDL 会顺带执行语句**——
      不要在"只想知道是不是查询"的路径上乱用。
  - `returns_result_set` 分类：`is_query()` 在 **`Statement`** 上，不在 `StatementBuilder` 上，
    所以要多一步 `build()`：
    ```rust
    let stmt = conn.statement(&sql).map_err(map_err)?.build().map_err(map_err)?;
    let is_query = stmt.is_query();   // 另有 is_ddl / is_dml / is_dml_returning / is_plsql
    ```
    备选：按 `SELECT`/`WITH`/`RETURNING` 前缀判断（省一次服务端往返，但 PL/SQL 块里可能不准）。
    **二选一并钉死**（推荐前缀判断为主、`is_query()` 兜底，因为 DDL 的 `ensure_fully_parsed`
    会执行语句）。
  - `statement` 字段填**该条**语句文本（供 `infer_single_table` 判定结果网格可编辑，
    注意 P6-T5 的 dialect 修正）。
  - `last_insert_id`：Oracle 无 `LAST_INSERT_ID`；从 `RETURNING … INTO` 或序列 `CURRVAL` 尽力，
    否则 `None`（文档说明）。
    *验收*：单条 `SELECT`、多语句脚本、`CREATE OR REPLACE PROCEDURE … BEGIN…END;`、
    `INSERT`、`BEGIN; …; COMMIT;` 语义正确；**带 `;` 的语句不再报驱动错误**。
- [ ] **P1-T10 `server_version` / `session_count` / `close`。**
  - `server_version`：`SELECT banner FROM v$version WHERE rownum = 1`（无权限时退
    `SELECT version FROM product_component_version WHERE rownum = 1`）。
  - `session_count`：`SELECT COUNT(*) FROM v$session WHERE type = 'USER'`（无 `v$` 权限时返回 0，文档注明）。
  - `close()`：**不能调 `Pool::close()`**（需要 `&mut`，而 trait 是 `async fn close(&self)`，
    字段又是 `Arc<Pool>`）→ 返回 `Ok(())`，靠 `Pool` 的 `Drop` 关闭（docs 明确 Drop 会 close）。
    代码里要写注释说明这个取舍，别让人后来"补上"一个编译不过的 `close`。
    *验收*：连接信息面板显示版本与会话数。
- [ ] **P1-T11 `create_schema` / `drop_schema`（必须实现，§3.10）。**
  `CREATE USER "S" NO AUTHENTICATION` / `DROP USER "S"`。**默认实现是报错**
  （`core/driver.rs:284/291`），而 `supports_schemas = true` 后 UI 会显示"新建模式/删除模式"
  （`app/widgets.rs:306`），不实现就是坏按钮。`DROP USER` 非空默认报错（`ORA-01922`），错误透传即可。
  *验收*：新建/删除空 schema 成功；删除非空 schema 给出可读错误。

### Phase 2 — 编辑与表 DDL

- [ ] **P2-T1 `update_rows`。**
  主键优先、无主键退化全列；`None` 键 → `IS NULL`（**绝不绑定空串**）；`:n` 参数化；事务内执行、
  结束时 `commit()`。**每个 `SET` / `WHERE` 绑定按 §3.8 处理类型**。
  *验收*：单列/多列主键、含 `NULL` 键的行都能改；**NUMBER / DATE / TIMESTAMP 列都能改成功**。
- [ ] **P2-T2 `insert_rows`。**
  `INSERT INTO "S"."T" ("C1","C2") VALUES (:1,:2)`；未列出的列走默认值；
  `None` 是显式 `NULL`；可考虑用 `execute_batch` 提效。
  *验收*：identity（`GENERATED … AS IDENTITY`）列不写值也能插入。
- [ ] **P2-T3 `delete_rows`。**
  与 `update_rows` 同样的键处理与类型处理；批量事务。
  *验收*：多选删除正确，`NULL` 键行能删。
- [ ] **P2-T4 表操作：`drop_table` / `empty_table` / `truncate_table` / `rename_table`。**
  - `DROP TABLE "S"."T"`；
  - `DELETE FROM "S"."T"`（Empty，可回滚）；
  - `TRUNCATE TABLE "S"."T"`（DDL，不可回滚；被外键引用时需 `CASCADE` 或先禁约束，错误透传）；
  - `ALTER TABLE "S"."OLD" RENAME TO "NEW"`（`new_name` 可能带 schema 前缀，**要先剥掉**）。
    *验收*：右键菜单四项都生效。
- [ ] **P2-T5 `character_sets` / `collations`。**
  - Oracle 无"库级字符集"（字符集建库时定，改需重建库）→ 库对话框已隐藏。
  - `character_sets()`：可返回 `SELECT value FROM nls_database_parameters WHERE parameter='NLS_CHARACTERSET'`
    的单值，或 `Vec::new()`（**二选一**，与库对话框是否显示对齐）。
  - `collations()`：`SELECT value FROM nls_database_parameters WHERE parameter='NLS_SORT'`，或空。
    *验收*：Options 页不显示不支持的字段；返回空时控件隐藏。
- [ ] **P2-T6 `column_types` / `storage_engines`。**
  - `column_types()`：`NUMBER`/`INTEGER`/`FLOAT`/`BINARY_FLOAT`/`BINARY_DOUBLE`/`VARCHAR2`/`NVARCHAR2`/
    `CHAR`/`NCHAR`/`CLOB`/`NCLOB`/`BLOB`/`RAW`/`LONG`/`DATE`/`TIMESTAMP`/`TIMESTAMP WITH TIME ZONE`/
    `TIMESTAMP WITH LOCAL TIME ZONE`/`INTERVAL YEAR TO MONTH`/`INTERVAL DAY TO SECOND`/`ROWID`/`XMLTYPE`/`JSON`。
    ⚠️ **列表首项就是"新建列"的默认类型**（`app/design.rs:992` 先找 `int`、找不到取 `first()`；
    Oracle 列表里没有 `INT`，只有 `INTEGER`，且只做 `eq_ignore_ascii_case` 全等匹配）→
    上面这份顺序会让新列默认 `NUMBER`，**这是期望行为，不要误改**。
  - `storage_engines()`：`Vec::new()`（Oracle 无存储引擎）。
  - **注意**：`all_tab_columns.data_type` 回显的是 `VARCHAR2` / `NUMBER` / `TIMESTAMP(6)` 这种规范名，
    表设计器要能匹配下拉项（在驱动侧统一写法，或让设计器做映射），否则打开已有表时类型下拉选不中。
    *验收*：表设计器类型下拉正确；打开已有表时类型能正确回显；`column_types` 首项为 `NUMBER`。
  - ⚠️ **导入向导也走这条链路**（`app/import.rs:932` → `save_table_schema`，`app/design.rs:433`
    读 `column_types`）。P2-T9 做对后导入建表自动可用，但**必须单独验收**：
    *验收*：用导入向导把一份 Excel/CSV 建成 Oracle 表，列类型映射合理，导入成功。
- [ ] **P2-T7 `table_status` / `table_statuses`。**
  - 行数：`SELECT num_rows FROM all_tables WHERE owner=:1 AND table_name=:2`（**统计信息估算值**，
    未 `ANALYZE`/`GATHER_STATS` 时为 `NULL`，**必须转成 `0`/`None`**，别显示空/负）；
  - 数据长度：`all_segments.bytes`（需权限）或 `all_tables.blocks * block_size`；
  - 注释：`all_tab_comments.comments`；引擎填 `None`；created/updated 无对应 → `None`。
  - `table_statuses`：按 schema 批量查（一次 JOIN），key 用**与 `list_tables` 一致的 `OWNER.NAME`**。
    *验收*：表列表的 行/数据长度/注释 列有值（估算），未分析的表不显示异常值。
- [ ] **P2-T8 `object_ddl`。**
  - 表：`DBMS_METADATA.GET_DDL('TABLE', :1, :2)`；
  - 视图：`DBMS_METADATA.GET_DDL('VIEW', :1, :2)`。
  *验收*：信息面板能看到可读的 DDL。
- [ ] **P2-T9 `table_schema` / `table_schema_sql` / `save_table_schema`（表设计器核心）。**
  - `table_schema`：从 `all_tab_columns` / `all_constraints`（主键、唯一、外键、检查）/
    `all_cons_columns` / `all_indexes` / `all_ind_columns` / 注释组装 `TableSchema`
    （列、索引、外键、options）。
    `identity` 列识别：`all_tab_columns.identity_column = 'YES'` → `auto_increment = true`；
    序列 + 触发器实现的自增尽力识别（`all_triggers` + `all_sequences`）。
  - `table_schema_sql`：
    - 新建：`CREATE TABLE "S"."T" (...)`；
    - `auto_increment`（`ColumnDef::auto_increment`）：生成
      `NUMBER GENERATED BY DEFAULT AS IDENTITY`（12c+ 推荐）；
    - 修改：生成 `ALTER TABLE` 序列——`ADD (…)` / `DROP COLUMN` / `MODIFY (col type)` /
      `MODIFY (col NULL|NOT NULL)` / `ADD|DROP CONSTRAINT` / `RENAME COLUMN`；
      索引用 `CREATE INDEX` / `DROP INDEX`；外键用 `ADD CONSTRAINT … FOREIGN KEY` / `DROP CONSTRAINT`。
  - `save_table_schema`：默认实现即可（拼 SQL 后走 `execute_query`）。
    *验收*：设计器能读出现有表结构、改列/加索引/加外键后保存成功且结构正确。
    *验收*：`auto_increment` 在 Oracle 上生成合法 identity DDL。

### Phase 3 — 视图与例程

- [ ] **P3-T1 视图列表（经 `list_tables`）+ 视图详情。**
  - `view_details`：`ViewInfo`（name/updatable）+ `definition`
    （`DBMS_METADATA.GET_DDL('VIEW', :1, :2)`）。
  - Oracle 无 MySQL 的 definer/algorithm/check_option/security_type 元数据 → 留空或从
    `all_views` / 定义文本尽力推导，**文档注明限制**。
  - `updatable`：`all_updatable_columns`（尽力）。
    *验收*：视图列表与详情页可用。
- [ ] **P3-T2 `view_sql` / `save_view` / `drop_view`。**
  - 保存：`CREATE OR REPLACE VIEW "S"."V" AS …`（Oracle 支持 `OR REPLACE`，改列集可能失败 → 报错透传）。
  - 删除：`DROP VIEW "S"."V"`。
  - ⚠️ app 的 `sql::view_identity_for` 要能解析 `"S"."V"`（P6-T5a 一并处理）。
    *验收*：设计视图→保存→预览数据链路通。
- [ ] **P3-T3 `list_routines` / `list_routine_infos`。**
  ```sql
  SELECT owner, object_name, object_type
  FROM all_objects
  WHERE object_type IN ('FUNCTION','PROCEDURE','PACKAGE','TRIGGER','TYPE')
  ORDER BY owner, object_name
  ```
  `name` = `OWNER.NAME`（**包内子程序**用 `OWNER.PACKAGE.NAME`，见 §3.4 的三段解析）；
  `kind` 由 `object_type` 映射到 `RoutineKind`；`return_type` 对 FUNCTION 取 `all_arguments`
  （`argument_name IS NULL` 的返回参数）。
  **`list_routines`（备份树）必须返回与 `list_routine_infos` 完全一致的名字**。
  *验收*：Functions 页列出函数/过程/包/触发器；备份树同名可解析。
- [ ] **P3-T4 `routine_details`。**
  `DBMS_METADATA.GET_DDL('FUNCTION'|'PROCEDURE'|'PACKAGE'|'PACKAGE BODY'|'TRIGGER', :1, :2)` →
  `RoutineDetails.definition`。
  *验收*：设计函数页的"定义"子页显示完整定义。
- [ ] **P3-T5 `routine_sql` / `save_routine` / `drop_routine`。**
  - 保存：**先 `DROP` 旧对象再 `CREATE`**（Oracle 无 `CREATE OR REPLACE TRIGGER` 之外的通用替换；
    FUNCTION/PROCEDURE 有 `OR REPLACE`，但改签名/类型时仍需先删）。用 `original` 的 `(name, kind)`。
    ⚠️ **这些语句必须整段作为一条语句发送**（PL/SQL 块内含分号），走 §3.7 的"块"识别路径，
    **不能被切分器切碎**。
  - 删除：`DROP FUNCTION "S"."F"` / `DROP PROCEDURE …` / `DROP PACKAGE …` / `DROP TRIGGER …`。
  - `list_events`：`Ok(Vec::new())`（Oracle 无事件调度器）。
    *验收*：新建/修改/删除函数、过程、包、触发器均成功。
- [ ] **P3-T6 app 层例程模板与调用 SQL 方言化（§2.3 最后一行）。**
  `routine_template_for` / `routine_call_sql_for`（`app/routine.rs:575/605`）当前是 MySQL 形状
  （反引号 + `#` 注释 + `BEGIN…END`）。为 Oracle 生成 PL/SQL 模板：
  ```sql
  CREATE OR REPLACE FUNCTION "S"."F"(...) RETURN ... AS
  BEGIN
    ...
  END;
  ```
  调用用 `SELECT "S"."F"(...) FROM dual` / `BEGIN "S"."P"(...); END;`。
  *验收*：Oracle 上 新建函数 / 运行函数 / 运行过程 生成合法 SQL。

### Phase 4 — 用户与权限

- [ ] **P4-T1 `list_users`。**
  ```sql
  SELECT username, account_status, default_tablespace, profile, created
  FROM dba_users          -- 无 DBA 权限时退 all_users（字段更少）
  ORDER BY username
  ```
  映射到 `UserAccount`（**host 置空**；检查 `UserAccount::label()` 的 `user@host` 渲染，
  空 host 时不要显示成 `SYS@`）。**schema 与用户同源**——Users 页与树里的 schema 会重叠，这是 Oracle 的正常现象，文档注明。
  *验收*：Users 页列出用户；不出现明显内部账号（可选过滤）。
- [ ] **P4-T2 `user_details`。**
  账号属性 + 系统权限（`dba_sys_privs`）+ 角色（`dba_role_privs`）+ 对象授权（`dba_tab_privs`）→
  `UserDetails`（`UserEditSection` 分组：常规 / 权限 / 角色）。
  *验收*：编辑用户窗口能回显属性、角色、授权。
- [ ] **P4-T3 `user_edit_sql` / `user_edit_groups` / `save_user` / `drop_user` / `rename_user`。**
  - 保存：`CREATE USER … IDENTIFIED BY "…"`（新建）/ `ALTER USER … IDENTIFIED BY "…"`（改密码）/
    `ALTER USER … DEFAULT TABLESPACE …`；`GRANT/REVOKE <priv> TO/FROM <user>`；
    `GRANT/REVOKE <role> TO/FROM <user>`；`GRANT/REVOKE <priv> ON <obj> TO <user>`。
  - **口令与标识符必须转义**：口令里的 `"` 要加倍（Oracle 的 `IDENTIFIED BY "pwd"` 用双引号），
    用户名一律 `"..."` 引用且内部 `"` 加倍。不要裸拼字符串。
  - 删除：`DROP USER "X"`（有对象时 `CASCADE` 需二次确认——**危险操作，UI 侧确认**）。
  - 重命名：Oracle **不能 `RENAME USER`**；改名需 `CREATE USER 新名` + 迁移对象 + `DROP USER 旧名`，
    代价高 → `rename_user` **返回明确"不支持"错误**（不静默成功），文档注明。
  - `authentication_plugins` / `ssl_types`：Oracle 无此概念 → 返回空（表单隐藏）。
    *验收*：新建/改密码/删除用户成功；SQL 预览正确；口令含特殊字符不产生注入/语法错误；
    `rename_user` 给出清晰错误。
- [ ] **P4-T4 `object_privilege_matrix` / `set_object_privileges` / `object_privileges_sql`。**
  - Oracle 对象权限：`SELECT`/`INSERT`/`UPDATE`/`DELETE`/`ALTER`/`INDEX`/`REFERENCES`/`EXECUTE`/
    `READ`/`WRITE`/`DEBUG`/`FLASHBACK`…
  - 读：`SELECT grantee, privilege, grantable FROM dba_tab_privs WHERE owner=:1 AND table_name=:2`
    （无 DBA 权限时退 `all_tab_privs` / `user_tab_privs`）。
  - 映射到 core 的 `Privilege` 枚举**存在的子集**；无法表达的（`ALTER`/`INDEX`/`EXECUTE`/…）
    在驱动内部用字符串处理，**不改 core 枚举**（若 UI 必须展示，见 §10 风险 R4）。
    *验收*：权限管理器能读/写表级授权，且能看到**其他用户**的授权。

### Phase 5 — 备份与还原

- [ ] **P5-T1 `backup_object_metadata`（表 / 视图 / 例程）。**
  - 表：DDL 走 `DBMS_METADATA.GET_DDL('TABLE', …)`；`fields` 取列名；
    `trigger_ddl` 取该表上的 `DBMS_METADATA.GET_DDL('TRIGGER', …)`。
  - 视图：`GET_DDL('VIEW', …)`。
  - 函数/过程/包：`GET_DDL('FUNCTION'|'PROCEDURE'|'PACKAGE'|'PACKAGE BODY', …)`。
  - `rows` 恒为空（表数据走 `stream_table_rows`）。
    *验收*：备份对象树能取到每类对象的 DDL。
- [ ] **P5-T2 `stream_table_rows`：Oracle 字面量渲染。**
  ⚠️ 先读 §3.2.1：**`spawn_blocking` 与"流式"天然冲突**，`Cursor` 是同步 `Iterator`。
  按 §3.2.1 的 `spawn_blocking` + `mpsc::channel` + `blocking_send` 实现，才能做到
  边收边渲染、**不整表缓冲**；不要在闭包里 `collect()` 回 `Vec`。
  流式 `SELECT * FROM "S"."T"`，逐行渲染元组：
  - `'...'`（`'`→`''`）、`NULL`、数字裸值；
  - `DATE`/`TIMESTAMP` → `TO_DATE('…','YYYY-MM-DD HH24:MI:SS')` / `TO_TIMESTAMP(…)`；
  - `RAW`/`BLOB` → `HEXTORAW('…')` / 二进制分块（BLOB 大对象策略要定，见 R5）；
  - `CLOB` → `TO_CLOB('…')`（超长需分块或走 `DBMS_LOB`）；
  - **空串渲染成 `NULL`**（§3.9，Oracle 无空串）。
    *验收*：`#[cfg(test)]` 单测覆盖转义与各类型字面量；空串→NULL 有专门用例。
- [ ] **P5-T3 `restore_object`。**
  回放 DDL + 批量 `INSERT`（可用 `execute_batch`）。三点易漏：
  - **identity 列**：`GENERATED ALWAYS AS IDENTITY` 显式插入会报错——建表 DDL 回放时若带
    `GENERATED ALWAYS`，需改写为 `GENERATED BY DEFAULT`（或插入时按列属性决定）；
  - **序列重置**：还原后对 identity/序列列
    `ALTER TABLE … MODIFY (id GENERATED BY DEFAULT AS IDENTITY (START WITH LIMIT VALUE))`
    或直接 `ALTER SEQUENCE … RESTART START WITH <max+1>`，否则后续插入会主键冲突；
  - **空表**：`max(id)` 为 NULL，重置逻辑要兜底（对齐 PG 的 `coalesce` 坑）。
    *验收*：备份→还原→再插入新行不冲突（含 identity 表）；**空表还原后也不报错**。

### Phase 6 — 收尾与对等

- [ ] **P6-T1 隧道支持。** 复用共享的 `rustgrid-tunnel`（PG 已抽好），在
  `OracleDriver::connect` 里 `Tunnel::start` 拿到本地端口，再把 connect string 指向 `127.0.0.1:<local_port>`。
- [ ] **P6-T2 连接 Options 页对齐（TCPS / 钱包）。**
  - Oracle 的 TLS 是 **TCPS 协议 + 钱包（wallet）**，与 core 的 `TlsMode` 五档不是一一对应：
    映射 `Disabled → tcp://`；其余 → `tcps://`。
  - 钱包路径/口令走 profile 的 `options`（如 `oracle.wallet_location` / `oracle.wallet_password`），
    **无需迁移 `connections.json`**（与 ODBC 存 `odbc.*` 同机制）；接的是 **`PoolConfig`** 的
    `set_wallet_location` / `set_wallet_password`（**不是** `oracledb::Config`）。
  - 注明：驱动**只读 `tnsnames.ora`**，不读 `sqlnet.ora` / `oraaccess.xml`。
    TNS 搜索目录可用 `PoolConfig::set_config_dir(...)` 指定。
- [ ] **P6-T3 只读 / 超时 / init_sql 复核。**
  - 只读会话：`ALTER SESSION SET READ ONLY`？Oracle 无会话级只读开关；可用
    `SET TRANSACTION READ ONLY`（仅事务级）或忽略——**钉死策略**并文档化。
  - 超时：⚠️ **`PoolConfig` 上没有 `set_tcp_connect_timeout`**（只有 `set_ping_interval` /
    `set_ping_timeout`，那是空闲连接保活用的）。落地时先在 P0-T1 探针里确认
    `oracledb` 是否另有连接超时入口；**确认不到就明确记为"不支持"，别留一个假的接线**。
    `query_timeout` 侧可用 `ALTER SESSION SET …` 或语句级 hint 尽力。
  - ⚠️ **`init_sql` 没有 `after_connect` 钩子可用**：`oracledb` 的池没有暴露"新建连接回调"，
    所以 `Settings.init_sql` 无法在每次建连时自动跑。两个可行解，**钉死一个**：
    ① 每次 `acquire()` 后在同一条连接上执行 `init_sql`（正确但多一次往返，建议加 `Once` 之外的
    轻量代价评估）；② 明确不支持并文档化。
    ⚠️ 同一个问题也影响 §3.6 的 `DBMS_METADATA.SET_TRANSFORM_PARAM`（**会话级**）：
    净化 DDL 的 PL/SQL 块必须与 `GET_DDL` 在**同一个 `spawn_blocking` 闭包、同一条连接**上执行，
    不能"设一次管全局"。
- [ ] **P6-T4 导出 `.sql` 方言。** `rustgrid_export::SqlDialect` 现有 `MySql`/`Postgres` 两个变体，
  `app/export.rs:771-777` 只匹配了 `Postgres`。新增 `SqlDialect::Oracle`：
  - `quote_identifier`：`"..."`；
  - `literal`：字符串 `'…'`（`'`→`''`）、**空串→`NULL`**、字节 → `HEXTORAW('…')`、
    时间 → `TO_DATE`/`TO_TIMESTAMP`；
  - `export.rs` 的 match 加 `DriverDialect::Oracle => SqlDialect::Oracle`。
  - `.xlsx`/`.csv`/`.txt` 引擎无关，不用动。
- [ ] **P6-T5 SQL 解析 / 高亮方言。** `app/src/sql.rs` 的 `sqlparser_dialect()` 已按
  `DriverDialect` 分派（`sql.rs:12-19`）。为 Oracle 加 `DriverDialect::Oracle => Box::new(OracleDialect {})`
  （**sqlparser 自带 `OracleDialect`**，见 `sqlparser::dialect`）。这影响：语法高亮、
  `view_select_for`（视图 EXPLAIN）、`infer_single_table_for`（**决定查询结果网格能否就地编辑**）。
  ⚠️ 换 dialect 会**改变这三者的既有行为**，必须单独补单测（Oracle 用例）确认没有回归。
- [ ] **P6-T5a app 层例程/视图标识符适配（这是"必须改"，不是"复核"）。**
  app 层有三处硬编码 `dialect == DriverDialect::Postgres` 的 schema 判定，Oracle 不加就会出错：
  - `sql.rs:201` `routine_identity_for`——schema 限定名的拼接分支；
  - `sql.rs:283` / `sql.rs:292` `view_identity_for`——`CREATE VIEW "S"."V"` 的名字解析。
    → 抽一个 `fn is_schema_qualified(dialect: DriverDialect) -> bool`（`Postgres | Oracle`）替换三个
    `== Postgres`，**别再加第四个 `|| dialect == DriverDialect::Oracle`**。
  - ⚠️ **`routine_call_sql_for`（`routine.rs:604`）的非 PG 分支会拼 `database.name`**：
    ```rust
    let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(name));
    ```
    Oracle 的 `database` 是 **PDB 名**，拼出来是 `XEPDB1."F"`——**必然非法**。
    必须新增一条 Oracle 分支，产出：
    - 函数 → `SELECT "S"."F"(…) FROM dual`（Oracle 的 `SELECT` 不能省 `FROM`）；
    - 过程 → `BEGIN "S"."P"(…); END;`（不是 `CALL`；`CALL` 仅适用于无 OUT 参数的简单过程，
      且不能在匿名块里用——**推荐一律匿名块**）。
  *验收*：Oracle 上 新建函数 / 运行函数 / 运行过程 / 保存例程 / 保存视图 都生成正确方言的 SQL；
  运行过程能真正跑通（不报 `ORA-00900` / `ORA-00933`）。
- [ ] **P6-T5b 语句扫描器支持 `q'[]'` 与 PL/SQL 块。** `sql::current_statement`（`sql.rs:605`）
  与 `in_string_or_comment` 只认 `'` / `"` / 反引号 / `--` / `/* */`，**不认 `q'X…X'`**，
  也**不把 `BEGIN…END` 当块**；Oracle 的 `CREATE PROCEDURE … BEGIN … ; … END;` 会在块内 `;` 处被切错，
  影响函数编辑器补全上下文。`helpers::split_statements` 同理（且要**剥掉行尾分号**）。
  *验收*：`q'[a;b]'` 内的 `;` 不切分；`BEGIN … END;` 整块不被切碎。
- [ ] **P6-T6 i18n 复核。** 新增文案（若有）在 `locales/{en,zh-CN}.yml` **两处都加**；
  优先复用现有 key（Oracle 无专属库对话框字段）。
- [ ] **P6-T7 文档。**
  - 更新 `AGENTS.md`：Repository layout 加 `rustgrid-oracle`；Tech stack 加 `oracledb` 依赖
    与"纯 Rust、无 Instant Client"的说明；Commands 加 live 测试；
    **"Still out of scope" 那句要改写**（`AGENTS.md:454`，原文是
    "engines other than MySQL, MariaDB, SQLite, SQL Server and PostgreSQL"，
    **新版本已含 PostgreSQL**，且没有 Oracle 字样）；`README.md`/`README_zh.md` 引擎表把
    **Oracle 从 "via generic ODBC" 移到 Supported**（ODBC 行保留 DB2/达梦 等）。
  - 新增 `docs/oracle-driver.md`（仿 `odbc-driver.md`）：连接串、TCPS/钱包、限制、已知差异，
    重点写明 **`oracledb` 是 beta**、**同步阻塞 + `spawn_blocking`**、**空串=NULL**、
    **分页需 `ORDER BY`/`ROWID`**、**脚本客户端切分（PL/SQL 块、`q'[]'`、去分号）**、
    **行数是估算值**、**改名用户不支持** 这几条。
- [ ] **P6-T8 全量校验。** `cargo fmt --all` → `cargo build` → `cargo test --workspace` →
  `cargo clippy --workspace --all-targets -- -D warnings` 全绿。

---

## 7. `Connection` trait 实现映射表

> 逐方法对照，作为落地时的检查表。带 ⚠️ 的是有非平凡实现成本的项。

| 方法                                                                                                               | Oracle 实现要点                                                                     |
| ---------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `driver_id`                                                                                                      | `"oracle"`                                                                      |
| `list_databases`                                                                                                 | ⚠️ 返回**当前库/PDB 名**（`SYS_CONTEXT`），Oracle 无多库列表                                       |
| `list_tables`                                                                                                    | ⚠️ `all_tables` ∪ `all_views`（+物化视图），返回 `OWNER.NAME`（**原样大写**）                    |
| `columns`                                                                                                        | ⚠️ `all_tab_columns` + `all_col_comments` + `all_constraints`(P)，**类型串留给绑定用**      |
| `fetch_page`                                                                                                     | ⚠️ `OFFSET … FETCH`（**强制 `ORDER BY`**）；主键 / `ROWID` / 视图退化 `ORDER BY 1`；⚠️ 过滤树 + ⚠️ 类型化绑定 |
| `update_rows`                                                                                                    | 主键优先，`None`→`IS NULL`，`:n` 参数化 + ⚠️ 类型化绑定，事务 + `commit`                             |
| `insert_rows`                                                                                                    | `INSERT … VALUES (:1,…)`；identity 列不写值（可用 `execute_batch`）                       |
| `delete_rows`                                                                                                    | 同 update 的键处理与类型处理                                                               |
| `execute_query`                                                                                                  | ⚠️ 客户端切分（PL/SQL 块 + `q'[]'` + 去分号）+ 单连接顺序执行                                        |
| `execute_query_many`                                                                                             | ⚠️ 同上，按语句边界切结果集；零行 `SELECT` 也要返回列元数据                                             |
| `create_database` / `_sql` / `drop_database`                                                                     | **不支持**（`supports_database_management = false`）；返回明确错误                         |
| `drop_table` / `empty_table` / `truncate_table` / `rename_table`                                                 | `DROP TABLE` / `DELETE FROM` / `TRUNCATE TABLE` / `ALTER TABLE … RENAME TO`（新名去 schema 前缀） |
| `database_options` / `database_owners` / `alter_database_*`                                                      | 空 / 不支持（库管理关闭）                                                                   |
| `server_version`                                                                                                 | `v$version`（无权限退 `product_component_version`）                                    |
| `session_count`                                                                                                  | `v$session`（`type='USER'`；无权限返回 0，文档注明）                                          |
| `table_status` / `table_statuses`                                                                                | ⚠️ `all_tables.num_rows`（**估算**，NULL 归零）+ `all_segments.bytes` + `all_tab_comments` |
| `object_ddl`                                                                                                     | ⚠️ `DBMS_METADATA.GET_DDL`（表/视图）                                                 |
| `character_sets` / `collations`                                                                                  | 单值（`nls_database_parameters`）或空                                                    |
| `list_schemas`                                                                                                   | ⚠️ `all_users`（可选隐藏维护账号）                                                        |
| `create_schema` / `drop_schema`                                                                                  | ⚠️ `CREATE USER … NO AUTHENTICATION` / `DROP USER`（**默认实现是报错，必须覆写**，P1-T11）      |
| `column_types`                                                                                                   | Oracle 类型表（`VARCHAR2`/`NUMBER`/`DATE`/…）                                          |
| `list_routines` / `list_routine_infos`                                                                           | ⚠️ `all_objects`（FUNCTION/PROCEDURE/PACKAGE/TRIGGER/TYPE），名字 `OWNER.NAME`，**两处一致** |
| `list_events`                                                                                                    | 空                                                                               |
| `backup_object_metadata`                                                                                         | ⚠️ `GET_DDL`（表/视图/例程/触发器）                                                       |
| `stream_table_rows`                                                                                              | ⚠️ 流式 SELECT + Oracle 字面量渲染（空串→NULL、`HEXTORAW`、`TO_DATE`）                          |
| `restore_object`                                                                                                 | 回放 DDL+INSERT，⚠️ identity 改写 + 序列重置（空表兜底）                                         |
| `storage_engines`                                                                                                | 空                                                                               |
| `table_schema` / `table_schema_sql`                                                                              | ⚠️ 设计器核心（`all_*` 读 + `CREATE/ALTER TABLE` 生成）                                    |
| `close`                                                                                                          | ⚠️ **不能调 `Pool::close`（需 `&mut`）**；返回 `Ok(())`，靠 `Pool` 的 `Drop` 关闭         |
| `list_users` / `user_details` / `user_edit_sql` / `user_edit_groups` / `save_user` / `drop_user` / `rename_user` | ⚠️ `dba_users`/`all_users` + `CREATE/ALTER/DROP USER`；host 置空；口令转义；**rename 不支持**  |
| `authentication_plugins` / `ssl_types`                                                                           | 空                                                                               |
| `object_privilege_matrix` / `set_object_privileges` / `object_privileges_sql`                                    | ⚠️ `dba_tab_privs` 读全量 + 权限子集映射（§8）                                               |
| `view_details` / `view_sql` / `save_view` / `drop_view`                                                          | `GET_DDL('VIEW')` + `CREATE OR REPLACE VIEW`                                    |

---

## 8. 类型映射表（Oracle → `CellValue`）

| Oracle 类型                                                                  | `CellValue`      | 备注                                              |
| ------------------------------------------------------------------------- | ---------------- | ----------------------------------------------- |
| `NUMBER(p,0)`（整数、p≤18）                                                    | `Int` / `Uint`   | 溢出或精度不足时退 `Text`                                |
| `NUMBER(p,s)`（有小数）                                                        | `Text`           | **精确字符串，不转 f64**（同 SQL Server/PG）               |
| `FLOAT` / `BINARY_FLOAT` / `BINARY_DOUBLE`                                | `Float`          |                                                 |
| `VARCHAR2` / `NVARCHAR2` / `CHAR` / `NCHAR`                               | `Text`           |                                                 |
| `CLOB` / `NCLOB` / `LONG`                                                  | `Text`           | 大对象截断预览策略要定（R5）                                 |
| `BLOB` / `RAW` / `LONG RAW`                                                | `Bytes`          | 十六进制展示                                          |
| `DATE`                                                                    | `Text`           | **含时间部分**（Oracle `DATE` 有 H/M/S）                  |
| `TIMESTAMP` / `TIMESTAMP WITH [LOCAL] TIME ZONE`                          | `Text`           | `OracleTimestamp` 格式化                           |
| `INTERVAL YEAR TO MONTH` / `INTERVAL DAY TO SECOND`                        | `Text`           |                                                 |
| `ROWID` / `UROWID`                                                         | `Text`           |                                                 |
| `XMLTYPE`                                                                  | `Text`           |                                                 |
| `JSON`                                                                     | `Text`           | 23c+ 原生 JSON 类型                                 |
| `VECTOR`                                                                   | `Text`           | 26ai；按文本展示                                       |
| `BOOLEAN`                                                                  | `Bool`           | 23c+ SQL 布尔类型                                   |
| 其它 / 未知                                                                    | `Text` 或 `Bytes` | 文本兜底；取不到文本回落 `Bytes`                             |

> 反向（写入）不走这张表：驱动按列类型做**类型化绑定**（§3.8 方案 A）或 `TO_DATE` 包裹（方案 B）。
> ⚠️ **`NUMBER` 的 `Int`/`Text` 二选一必须钉死**：推荐"整数标度→`Int`，否则→`Text` 精确串"，
> 兼顾可读性与精度；**任何情况下都不用 f64 承载 `NUMBER`**。

---

## 9. 测试计划

### 9.1 纯函数单元测试（放在 `src/**` 的 `#[cfg(test)] mod tests`）

> 仓库惯例：驱动 crate 的单测都在源文件里（`rustgrid-sqlserver/src/helpers.rs:528`、
> `rustgrid-postgresql/src/helpers.rs`），`tests/` 只放 live 集成测试。**不要建 `tests/helpers.rs`**
> ——`tests/` 下每个文件是独立的测试二进制，链接不到 crate 私有的 `helpers` 模块。

- 标识符引用与转义（`a"b`、**全大写/混合大小写**、含点）。
- `resolve_object` 的 `schema.name` / `schema.package.name` / 裸名回退 `default_schema`。
- **语句切分（重点，§3.7）**：
  - 字符串内分号、注释内分号不切；
  - `q'[a;b]'` / `q'{x;y}'` / `q'!a;b!'` 内分号不切；
  - **`BEGIN … INSERT …; COMMIT; END;` 整块不被切碎**；`CREATE OR REPLACE PROCEDURE … END;` 同上；
  - `IF … END IF;` / `CASE … END;` 不误判块边界；
  - 单独一行 `/` 作为分隔符；
  - **行尾分号被剥掉**；不平衡时整段兜底。
- 过滤树翻译：嵌套组、`IN`、`BETWEEN`、`IS NULL`、绑定顺序与 `:n` 对齐，类型化绑定/`TO_DATE` 正确。
- 分页 SQL：有/无排序、有/无主键（`ROWID`）、**视图退化 `ORDER BY 1`** 分支。
- DDL 生成：`CREATE TABLE`、`ALTER TABLE` 增删列/改类型、索引、外键、**`auto_increment` → identity**。
- 字面量渲染：`'` 转义、**空串→NULL**、`HEXTORAW`、`TO_DATE`/`TO_TIMESTAMP`、`NULL`。
- 类型映射：§8 全覆盖；`NUMBER` 大数不丢精度。

### 9.2 真机集成测试（`crates/rustgrid-oracle/tests/live_oracle.rs`，默认 `#[ignore]`）

环境变量：`RUSTGRID_ORACLE_HOST` / `PORT` / `USER` / `PASSWORD` / `SERVICE`
（默认 `localhost` / `1521` / `system` / — / `FREEPDB1`）。
⚠️ 默认值曾写成 `XEPDB1`，与下面 Docker 镜像 `gvenzl/oracle-free` 的服务名不一致，
**已统一为 `FREEPDB1`**；用 Oracle XE 镜像时再显式传 `SERVICE=XEPDB1`。
运行：`cargo test -p rustgrid-oracle -- --ignored`。

覆盖：连接与认证失败映射 → 列库/列 schema/列表/列列 → 分页（含过滤与排序）→
增删改 → 任意 SQL（含**多语句、PL/SQL 块、`q'[]'`、带分号语句**）→ 表 DDL →
表设计器读写 → 视图列表/打开 → 函数/过程/包列表/详情 → 用户/角色列表。

**必须覆盖的类型用例**（§3.8，最容易回归）：对 `NUMBER` / `DATE` / `TIMESTAMP` / `VARCHAR2` 列
分别做 过滤、UPDATE、INSERT、按主键 DELETE。

**必须覆盖的边界用例**：
- **空串语义**（§3.9）：写入 `Some("")` 后读回是 NULL；导出的 `.sql` 里空串是 `NULL`。
- **PL/SQL 脚本**（§3.7）：`CREATE OR REPLACE PROCEDURE … BEGIN …; END;` 整段创建成功；
  带行尾 `;` 的语句不报驱动错误。
- **分页稳定**（§3.5）：无主键表用 `ROWID`；视图分页不报错；
  **IOT / 全局临时表能走 `ROWID` 回退分支**（§3.5 四级回退，不能只测普通堆表）。
- **运行过程**（P6-T5a）：`BEGIN "S"."P"(…); END;` 真的能跑通，而不是拼出 `CALL XEPDB1."P"`。
- **导入向导建表**（P2-T6）：Excel/CSV 导入后表结构与数据都正确。

Docker 起库参考（Oracle Free，需接受许可）：

```bash
docker run -d --name rg-oracle -p 1521:1521 -e ORACLE_PASSWORD=oracle \
  gvenzl/oracle-free:23-slim
# 服务名通常为 FREEPDB1
```

### 9.3 回归

- `cargo test --workspace`：MySQL / SQLite / SQL Server / PostgreSQL / ODBC 现有测试不得回归。
- `cargo clippy --workspace --all-targets -- -D warnings`：零告警。

---

## 10. 风险与未决问题

| #  | 风险 / 问题                                                                     | 影响 | 建议                                                                                |
| -- | --------------------------------------------------------------------------- | -- | --------------------------------------------------------------------------------- |
| **R1** | **`oracledb` 是 beta（26.0.0-beta.4），官方明示 API 会变**                          | **高** | **锁定精确版本**（`=26.0.0-beta.4`）；所有驱动 API 调用收敛在 `connection.rs`/`driver.rs` 一处，便于升级；升级前跑 live 测试 |
| **R2** | **同步阻塞 API 桥接**                                                              | 中  | **已降级**：docs.rs 已确认 `Pool`/`Connection` 均 `Send + Sync`，但**都无 `Clone`** → 池必须 `Arc`（§3.1/§3.2）。残余风险只剩"阻塞线程里 `acquire` 能否跑通"，由 P0-T1 探针验证；不通过才转 §3.2 方案 B |
| **R3** | **PL/SQL 脚本切分**（无 `$$`、块内含分号、`q'[]'`、行尾分号被拒）                                  | **高** | §3.7 客户端切分器 + P6-T5b；`#[cfg(test)]` 用大量用例回归；执行路径与"填 `statement`"共用同一实现           |
| **R4** | core 的 `Privilege` 枚举是 MySQL 形状，Oracle 权限（`ALTER`/`INDEX`/`EXECUTE`…）无法一一对应 | 中  | 驱动内做子集映射 + 字符串兜底；**不轻易改 core**；必要时单列一条 core 变更任务                                 |
| **R5** | **LOB（CLOB/BLOB）与 `LONG`**：大对象全量读会吃内存；备份字面量渲染需分块/`DBMS_LOB`                     | 中→高 | 网格只预览前 N KB；备份对超大 LOB 走 `DBMS_LOB` 分块或占位 + 文档注明；`LONG` 老类型尽力。⚠️ 另见 §3.2.1：`stream_table_rows` 若退化成"闭包里 `collect()`"，大表会整表进内存——**必须走 channel 流式** |
| **R6** | **空串=NULL** 与其它引擎语义不同                                                        | 中  | §3.9 钉死读写语义；live 测试专门覆盖；导出字面量空串→`NULL`                                              |
| **R7** | **`OFFSET/FETCH` 需 12c+ 且强制 `ORDER BY`**                                     | 中  | §3.5；11g 及更早不支持（文档注明）；视图/无 `ROWID` 对象分页不稳定（文档注明）；⚠️ **IOT / 全局临时表也没有 `ROWID`**，需走 §3.5 的四级回退 |
| **R8** | **`DBMS_METADATA` 输出含 STORAGE/TABLESPACE 等 DBA 细节**                          | 低→中 | §3.6 用 `SET_TRANSFORM_PARAM` 净化；净化失败时退原文（仍可读）                                        |
| **R9** | **标识符大小写**：`"users"` ≠ `USERS`                                              | 中  | §3.4 一律按目录原样名字引用，**禁止折叠大小写**；单测覆盖                                                  |
| **R10** | **schema = user**，建/删 schema 实为建/删用户，且 `rename_user` 不支持                        | 中  | §3.10 `NO AUTHENTICATION` / `DROP USER`；`rename_user` 返回明确错误，文档注明                   |
| **R11** | 行数只能给估算值（`all_tables.num_rows`），未收集统计信息时为 NULL                                 | 低  | 文档注明；NULL 归零；可提供"收集统计信息"可选增强                                                       |
| **R12** | 驱动**只读 `tnsnames.ora`**，不读 `sqlnet.ora`/`oraaccess.xml`                       | 低  | P6-T2 文档化；需要时用完整 connect string                                                      |
| **R13** | 依赖体积增大（`oracledb` 约 1MB / 15K SLoC，含 `rustls`/`aes`/`pbkdf2`）                | 低  | 已知代价；`opt-level="z"`+`lto`+`strip` 已就位；TLS 相关已在 lock 中，不新增原生编译                          |
| **R14** | ⚠️ **beta 依赖进默认构建**：`rustgrid-app` 会无条件依赖 `rustgrid-oracle`，于是**每个用户每次 `cargo build` 都拉一个 beta crate**，beta 升级即可能全仓库编译失败 | 中 | 见下方"动手前需拍板"新增的一条——要么与 PostgreSQL 同级（无条件），要么加 `driver-oracle` feature（默认开）。**必须先拍板再动 P0-T2** |

**动手前需拍板**：§2.1 路线（A/B/C）、§3.2 同步桥接（A 还是 B，取决于 P0-T1 的探针）、
**`oracledb` 是否进默认构建（R14，新增）**——仓库已有 `driver-odbc` 的 feature 先例：
无条件依赖则与 PostgreSQL 同级（简单，但 R14 成立）；加 `driver-oracle` feature 则默认开、
可关（多一层 `#[cfg]`，但 beta 期可一键摘除）。
§3.3 schema 隐藏清单、§3.5 分页下限版本、§3.8 绑定类型方案（A 类型化 还是 B `TO_DATE`）、
§8 `NUMBER` 的 `Int`/`Text` 取舍、§3.7 切分器的块识别策略。

---

## 11. 验收标准（Definition of Done）

- [ ] `cargo fmt --all`、`cargo build`、`cargo test --workspace`、
  `cargo clippy --workspace --all-targets -- -D warnings` 全绿，**零新告警**。
- [ ] 不影响 MySQL / MariaDB / SQLite / SQL Server / PostgreSQL / ODBC 现有功能与测试。
- [ ] 连接菜单里 Oracle **可点**（不再灰显）且**只出现一次**，默认端口 1521，能测试连接与保存。
- [ ] **零外部依赖**：无需安装 Oracle Instant Client 即可连接（证明走的是 thin 驱动）。
- [ ] 真机（或 Docker `gvenzl/oracle-free`）跑通：
  列库 → 列 schema → 列表/视图 → 列列 → 分页取数（含过滤/排序）→ 增删改 →
  执行任意 SQL（含**多语句、PL/SQL 块、`q'[]'`、带分号语句**）→ 表设计器读改 →
  视图列表/打开 → 函数/过程/包列表/详情 → 用户/角色列表。
- [ ] **类型回归**：NUMBER / DATE / TIMESTAMP 列的 过滤 + 增删改 全部成功（R6/§3.8）。
- [ ] **空串语义**：写入空串读回为 NULL；导出 `.sql` 里空串是 `NULL`（R6/§3.9）。
- [ ] **脚本回归**：`CREATE OR REPLACE PROCEDURE … END;` 整段创建成功；带行尾 `;` 不报驱动错误（R3/§3.7）。
- [ ] **分页回归**：无主键表用 `ROWID` 稳定翻页；打开视图分页不报错（R7/§3.5）。
- [ ] **schema 管理**：新建 / 删除空 schema 成功；删除非空 schema 报可读错误（R10/§3.10）。
- [ ] 备份 → 还原 → 再插入新行不冲突（序列已重置，含 identity 表、**空表**）。
- [ ] 新增 live 测试默认 `#[ignore]`，靠 `RUSTGRID_ORACLE_*` 启用，并写入 `AGENTS.md`。
- [ ] `AGENTS.md` / `README.md` / `README_zh.md` 已更新（引擎列表：Oracle 从 "via ODBC" 移到 Supported）。
- [ ] `PLANNED_ENGINES` 中 Oracle 占位已删除（并确认菜单无重复条目）。
- [ ] 导出的 `.sql` 在 Oracle 上可执行（P6-T4）；查询编辑器高亮 / EXPLAIN / 结果网格可编辑判定
      对 Oracle 语法正确（P6-T5）；语句扫描器认 `q'[]'` 与 PL/SQL 块（P6-T5b）。

---

## 12. 建议排期（里程碑）

| 里程碑 | 内容      | 可演示结果                         |
| --- | ------- | ----------------------------- |
| M0  | P0-T1   | **可行性验证**：`oracledb` 能编译、能连、`Arc<Pool>` + 阻塞线程 `acquire` 探针通过（**决定整计划成败**） |
| M1  | P0 + P1 | 能连、能看表、能跑查询（只读可用）             |
| M2  | P2      | 网格可编辑、表可建改删                   |
| M3  | P3      | 视图与函数/过程/包设计器可用               |
| M4  | P4 + P5 | 用户/权限 + 备份/还原                  |
| M5  | P6      | 隧道、TCPS/钱包、导出/解析方言、文档、全量校验（可发布） |

> 落地时以 `cargo check -p rustgrid-oracle` 逐步迭代，每完成一个 Phase 跑一次
> `cargo fmt` + `cargo build`；全部完成后跑全量 `test` + `clippy`。
> **建议先把 §3.2（同步桥接）、§3.7（脚本切分）和 §3.8（绑定类型）三条在 P1 就钉死**——
> 它们会反向影响 P2 全部编辑任务与 P6 的 app 层方言改动，返工成本最高。
> 而 **P0-T1 是唯一的 Go/No-Go 闸门**：不过就换路线，别硬做。
> 注意闸门的判据已收窄——"`Pool` 不是 `Clone`"**不算失败**（用 `Arc` 解决），
> 真正要验的是"**编译得过**"和"**阻塞线程里 `acquire` + `query` 跑得通**"。

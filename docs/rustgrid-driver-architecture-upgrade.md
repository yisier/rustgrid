# RustGrid 驱动架构升级方案(借鉴 DBX)

> 目标:把「驱动元信息 + 能力」收敛为**单一事实源**,并把 `DriverSource` 边界真正用起来,
> 使「新增一个引擎」从「改 5 处 `match driver`」变成「加一个 crate + 挂一行」。
>
> 本文做架构升级,**不引入运行时下载 / Agent 进程 / JRE 管理**(与「小体积单二进制」目标冲突,且 native 仅 4 个引擎)。
> ODBC 作为**可选引擎**(独立 crate + feature,默认关闭)在 Phase 6 落地。

---

## 0. 前置决策与假设

本方案基于以下假设设计;已确认项标注 D4,其余 D1–D3 可在开工前调整:

1. **范围**:native 覆盖 MySQL / MariaDB / SQLite / SQL Server 四个引擎,只打磨扩展边界;**额外提供一个可选的 ODBC 引擎**(独立 crate + feature 开关,默认关闭),用于兜底没有纯 Rust 驱动的长尾库。不做运行时下载驱动(ODBC 驱动由用户在系统层自行安装)。
2. **形态**:`DriverDescriptor` 用 Rust 结构体(由 `Driver` 返回),不解析外部 JSON 清单(编译期确定,避免运行时 I/O 与错误路径)。
3. **兼容**:分阶段推进,每个 Phase 结束时 `cargo build` / `test` / `clippy` 必须全绿,不破坏既有行为。
4. **边界预留**:`DriverSource` 做成 `BuiltinDriverSource` 的静态实现,将来要动态加载只是再加一个实现,UI 零改动。

**关键决策点**:
- D1(待确认):是否接受在 core 新增 `DriverDescriptor` / `DriverCapabilities` / `DriverDialect`(会让 `Driver` trait 略有扩展,但保留旧方法做默认实现,不破坏现有 driver crate)。
- D2(待确认):图标/品牌色是否放进 descriptor(会字面出现 `"icons/sqlserver.svg"` 这类字符串在 core 里;好处是 UI 单一来源)。备选:图标留在 app,由 descriptor 的 `icon_style` 枚举驱动。
- D3(待确认):`New Connection` 菜单里目前「灰显的 PostgreSQL / Oracle / MongoDB」是否保留(它们并不在 registry 中)。本方案默认保留为 app 侧常量占位。
- **D4(已确认):保留 native 驱动,ODBC 作为可选引擎**(独立 crate + feature 开关,默认关闭)。详见 §4 Phase 6。

---

## 1. DBX 的可借鉴原则(削减版)

| 原则 | DBX 做法 | RustGrid 对应 |
|---|---|---|
| 元信息单一事实源 | `database-drivers.manifest.json` | 收敛为 `DriverDescriptor` |
| 按能力而非引擎名判断 | 四级能力 + per-engine override | `DriverCapabilities` + 上移的 `DriverCapability` |
| 驱动/呈现分离 | profile(driver) + presentation | descriptor 描述呈现,`connect()` 负责行为 |
| 加载边界可扩展 | Agent manager | `DriverSource` + `BuiltinDriverSource` |
| 操作有超时边界 | Agent 请求 30s 超时 | 统一 `with_timeout` 包装(P3) |
| 生命周期可观测 | runtime monitoring | 连接健康抽象(P5,可选) |

**明确不学**:Agent 独立进程、stdin/stdout JSON-RPC、JRE 管理、运行时下载/升级驱动。

---

## 2. 现状问题清单(证据)

| # | 问题 | 位置 |
|---|---|---|
| P-1 | 引擎图标硬编码 `match driver` | `crates/rustgrid-app/src/app/mod.rs:2911`(`driver_icon`)、`:2925`(`tree_driver_icon`,还含 `mysql\|mariadb` 徽标特判、SQLite/SQLServer 品牌色特判) |
| P-2 | 连接类型硬编码(含不在 registry 的引擎) | `crates/rustgrid-app/src/app/toolbar.rs:11`(`CONNECTION_TYPES`)、`:303`(`render_connect_menu`) |
| P-3 | 能力是零散布尔,app 再手工映射 | `crates/rustgrid-core/src/driver.rs` 的 `supports_*`;`crates/rustgrid-app/src/app/mod.rs:1267`(`DriverCapability` 私有 enum)、`:2197`(`driver_supports`) |
| P-4 | 方言按 id 字符串分派 | `crates/rustgrid-app/src/sql.rs:346`(`functions_for`)、`crates/rustgrid-app/src/app/sql_completion.rs` |
| P-5 | `DriverSource` 是空 trait,注册是手写列表 | `crates/rustgrid-core/src/registry.rs:7`;`crates/rustgrid-app/src/main.rs:36-41` |
| P-6 | 展示名/默认端口在 trait,布局在 `DatabaseEditorSpec`,图标在 app——三处分离 | `driver.rs` 多处 |
| P-7 | 驱动操作无统一超时/取消边界 | `Runtime::spawn` 调用点分散 |

---

## 3. 目标架构(改完后)

```
rustgrid-core
├── driver.rs        Driver trait: id/descriptor()/dialect()/connect()   ← 行为
│                    DriverDescriptor { name, port, icon, icon_style,    ← 元信息(单一事实源)
│                                       capabilities, database_editor, order }
├── capability.rs    DriverCapability + DriverCapabilities               ← 能力
├── dialect.rs       DriverDialect { Conventions..., builtin_functions } ← 方言
└── registry.rs      DriverRegistry + DriverSource
                     BuiltinDriverSource (impl DriverSource)            ← 内置实现

rustgrid-odbc (可选,feature = "driver-odbc",默认关闭)
└── OdbcDriver        Driver/Connection via odbc-api                     ← 长尾库兜底
                     复用 DriverDescriptor / DriverDialect / DriverSource

rustgrid-app
├── main.rs          registry.register_source(&BuiltinDriverSource)
├── app/mod.rs       driver_icon/tree_driver_icon → 读 descriptor
├── app/toolbar.rs   render_connect_menu → 遍历 registry(+ app 侧占位常量)
├── app/form.rs      连接表单按 ConnectionFormSpec 渲染(含 ODBC 字段)
└── sql.rs           functions_for(dialect) 取代 functions_for(driver_id)
```

核心原则:app 层**不再出现具体引擎名字符串**(`"mysql"` / `"sqlserver"` …),只认 descriptor / capability / dialect。

---

## 4. 分阶段升级步骤

### Phase 0 — 引入 descriptor 骨架(零行为变化)

**目的**:先加类型与默认实现,不动任何调用点,保证 `cargo build` 全绿。

**改动**:
1. `crates/rustgrid-core/src/driver.rs`(或新增 `descriptor.rs`):
   ```rust
   #[derive(Debug, Clone)]
   pub struct DriverDescriptor {
       pub id: DriverId,
       pub display_name: String,
       pub default_port: u16,
       pub is_file_based: bool,
       pub icon: &'static str,          // 见 D2
       pub icon_style: DriverIconStyle,
       pub capabilities: DriverCapabilities,
       pub database_editor: DatabaseEditorSpec,
       pub order: u16,                  // 菜单稳定排序
   }

   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum DriverIconStyle {
       Plain,            // 状态色单色图标
       SolidBadge,       // 白字画在状态色徽标上(MySQL/MariaDB)
       Brand(u32),       // 品牌色图标(SQLite/SQL Server)
   }
   ```
2. `Driver` trait 增加**带默认实现**的方法:
   ```rust
   fn descriptor(&self) -> DriverDescriptor {
       DriverDescriptor {
           id: self.id(),
           display_name: self.display_name(),
           default_port: self.default_port(),
           is_file_based: self.is_file_based(),
           icon: "icons/connection.svg",
           icon_style: DriverIconStyle::Plain,
           capabilities: DriverCapabilities::from_flags(
               self.supports_database_management(),
               self.supports_users(),
               self.supports_routines(),
               self.supports_schemas(),
           ),
           database_editor: self.database_editor(),
           order: 1000,
       }
   }
   ```
   > 这样即使不 override,行为与今天完全一致;`supports_*` 继续可用。
3. `crates/rustgrid-core/src/lib.rs` 导出 `DriverDescriptor`、`DriverIconStyle`、`DriverCapability`、`DriverCapabilities`。

**验收**:`cargo build` 无变化,现有测试全过。**无任何调用点改动。**

---

### Phase 1 — 能力与呈现收敛(核心收益)

**目的**:消除 P-1、P-2、P-3、P-6。

**改动**:
1. `crates/rustgrid-core/src/capability.rs`(新增):
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum DriverCapability {
       DatabaseManagement, Users, Routines, Schemas, Events, Views,
   }

   #[derive(Debug, Clone, Default)]
   pub struct DriverCapabilities(Vec<DriverCapability>); // 无新依赖;可换 bitflags

   impl DriverCapabilities {
       pub fn from_flags(db: bool, users: bool, routines: bool, schemas: bool) -> Self { … }
       pub fn none() -> Self { … }
       pub fn with(self, c: DriverCapability) -> Self { … }
       pub fn has(&self, c: DriverCapability) -> bool { … }
   }
   ```
2. **每个驱动 crate 覆写 `descriptor()`**,填入正确元信息:
   - `rustgrid-mysql`:MySQL/MariaDB → `SolidBadge`、`icons/mysql.svg`/`icons/mariadb.svg`、order 10/20。
   - `rustgrid-sqlite`:`Brand(0x0f80cc)`、`icons/sqlite.svg`、无 `DatabaseManagement/Users/Routines/Schemas`。
   - `rustgrid-sqlserver`:`Brand(0xcc2927)`、`icons/sqlserver.svg`、开启 `Schemas` 等。
   - `DatabaseEditorSpec` 直接搬进 descriptor。
3. app 层删除散落 match:
   - `app/mod.rs:2911 driver_icon` → `driver.descriptor().icon`(找不到驱动时回退 `icons/connection.svg`)。
   - `app/mod.rs:2925 tree_driver_icon` → 依据 `descriptor().icon_style` 决定 `Plain / SolidBadge / Brand(color)`。
   - `app/mod.rs:2911`、`2925` 两个函数改为接收 `&DriverDescriptor`(调用点已持有 registry)。
   - `app/mod.rs:1267` 删除私有 `DriverCapability`,改用 core 的类型;`driver_supports`(:2197)改为 `driver.descriptor().capabilities.has(capability)`,并保留「未知连接返回 true」的语义。
4. `app/toolbar.rs:11`:
   - 启用项由 registry 生成:`registry.drivers()` 按 `descriptor().order` 排序,取 `display_name` + `id`。
   - 灰显占位(postgresql/oracle/mongodb)保留为 app 侧常量 `PLANNED_ENGINES`(见 D3)。
   - `DriverRegistry::drivers()` 排序需稳定:按 `(order, display_name)` 排序。`registry.rs` 增加 `drivers_sorted()` 或在 `drivers()` 内排序。
5. `crates/rustgrid-app/src/app/sql_completion.rs`:若此处引用驱动 id,改为用 descriptor/`dialect`(与 Phase 3 合并处理亦可)。

**验收**:
- 四个引擎的图标、品牌色、菜单顺序、能力开关与改动前**逐一对齐**(可截图/人工核对)。
- `grep -rn '"mysql"\|"mariadb"\|"sqlserver"\|"sqlite"' crates/rustgrid-app/src/app` 结果显著减少,只剩占位常量与少数必要处。

---

### Phase 2 — `DriverSource` 落地(扩展边界)

**目的**:消除 P-5,让「运行时安装驱动」将来只加一个实现。

**改动**:
1. `crates/rustgrid-core/src/registry.rs`:
   ```rust
   pub struct BuiltinDriverSource { drivers: Vec<Arc<dyn Driver>> }
   impl BuiltinDriverSource {
       pub fn new() -> Self { … }
       pub fn with(mut self, d: Arc<dyn Driver>) -> Self { self.drivers.push(d); self }
   }
   impl DriverSource for BuiltinDriverSource {
       fn drivers(&self) -> Vec<Arc<dyn Driver>> { self.drivers.clone() }
   }
   ```
   > 之所以放在 core 而非 app:它是「内置驱动列表」的通用实现,任何前端(CLI/MCP 将来)都能复用。
   > 若坚持 app 不依赖具体 driver crate 的原则,`BuiltinDriverSource` 只存 `Vec<Arc<dyn Driver>>`,具体实例仍由 `main.rs` 注入。
2. `crates/rustgrid-app/src/main.rs:36-41`:
   ```rust
   let mut registry = DriverRegistry::new();
   registry.register_source(
       &BuiltinDriverSource::new()
           .with(Arc::new(rustgrid_mysql::MysqlDriver::new()))
           .with(Arc::new(rustgrid_mysql::MariaDbDriver::new()))
           .with(Arc::new(rustgrid_sqlite::SqliteDriver::new()))
           .with(Arc::new(rustgrid_sqlserver::SqlServerDriver::new())),
   );
   ```
   > 或者更彻底:`rustgrid-drivers` 门面 crate 提供 `builtin_source()`,app 只依赖它。视 D1 决定,本方案先不强推新 crate。

**验收**:`main.rs` 不再逐个 `register(...)`;注册点只剩一处可扩展的 source。

---

### Phase 3 — 方言数据化(去掉 id 字符串分派)

**目的**:消除 P-4。

**改动**:
1. `crates/rustgrid-core/src/dialect.rs`(新增):
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum DriverDialect { Mysql, SqlServer, Sqlite, Generic }

   impl DriverDialect {
       pub fn builtin_functions(self) -> &'static [&'static str] { … }
       pub fn keyword_set(self) -> &'static [&'static str] { … }   // 如需要
   }
   ```
2. `Driver` 增加 `fn dialect(&self) -> DriverDialect { DriverDialect::Generic }`;四个驱动覆写。
3. `crates/rustgrid-app/src/sql.rs:346`:`functions_for(driver_id)` → `functions_for(dialect)`,把 `BUILTIN_FUNCTIONS` 迁到 `DriverDialect::Mysql::builtin_functions()`。
4. `sql_completion.rs`:scope 里存 `dialect`(或直接存 `&'static [&'static str]`),不再存 driver id 做 match。
5. `crates/rustgrid-app/src/sql.rs:1024` 的断言改为按 dialect。

**验收**:`sql.rs` / `sql_completion.rs` 不再出现引擎名字符串。

---

### Phase 4 — 统一驱动操作超时边界(P3,可选)

**目的**:缓解 P-7,防止驱动 hang 拖垮 UI。

**改动**:
1. `crates/rustgrid-app/src/runtime.rs` 增加:
   ```rust
   pub fn spawn_with_timeout<T>(&self, secs: u64, fut) -> JoinHandle<Result<T, Error>>
   ```
   或在 core 加 `Error::Timeout`(若尚无)。
2. 对**易 hang** 的调用点包一层:首页连接、`execute_query`、`fetch_page`、备份/导入的长任务。
3. 超时统一映射为 `Error::Query("timed out after Ns")` 或新增 `Error::Timeout(String)`,UI 走既有错误对话框。

> 注意:此阶段不引入进程隔离;只做「未来式超时」。风险中等,放最后。

**验收**:人为让某查询 `WAITFOR DELAY`/`SLEEP` 超过阈值时,UI 能报超时而非永久转圈。

---

### Phase 5 — 连接健康/生命周期抽象(P5,可选,延后)

**目的**:集中 keepalive / 健康检查 / 活跃信息。

**改动**:core 增加 `ConnectionHealth`(默认空实现),app 连接信息面板统一展示。当前只有 `server_version`/`session_count`,收益有限,**建议暂缓**,等有明确 UI 需求再做。

---

### Phase 6 — ODBC 可选引擎(独立 crate + feature 开关)

> **决策 D4(已确认)**:native 为主,ODBC 作为**可选引擎**,默认关闭。
> **前提**:Phase 0–3 完成(descriptor / capability / dialect / DriverSource 就绪)。
> **定位**:只兜底「没有纯 Rust 驱动的长尾库」(Oracle、DB2、达梦…),**不替换** native MySQL/SQLite/SQL Server,也不默认打包。

**配置零迁移**:`ConnectionProfile` / `ConnectionConfig` 已有 `options: BTreeMap<String, String>`(`crates/rustgrid-core/src/model.rs:43/57`),ODBC 直接用它存:
```
odbc.driver              = "ODBC Driver 18 for SQL Server"
odbc.dsn                 = ""                  # DSN 或连接串二选一
odbc.connection_string   = ""
odbc.engine              = "sqlserver"         # 底层库提示,用于方言/能力
```
→ **不需要 bump `connections.json` 版本、不需要写 migration**;密码仍走加密 secrets,不进 profile。

**改动**:
1. 新建 `crates/rustgrid-odbc`,加入 workspace `members` / `[workspace.dependencies]`;依赖 `odbc-api`(安全封装,底层 `odbc-sys`)。
2. `crates/rustgrid-app/Cargo.toml` 增加 feature(默认关闭):
   ```toml
   [features]
   driver-odbc = ["dep:rustgrid-odbc"]
   ```
   `main.rs` 条件注册:
   ```rust
   let mut source = BuiltinDriverSource::new()
       .with(Arc::new(rustgrid_mysql::MysqlDriver::new()))
       /* … native … */;
   #[cfg(feature = "driver-odbc")]
   { source = source.with(Arc::new(rustgrid_odbc::OdbcDriver::new())); }
   registry.register_source(&source);
   ```
   > `odbc-api` 在 **Unix 构建需要 unixODBC/iODBC 开发库**,所以必须 feature 化,避免拖累默认构建与 CI。
3. `OdbcDriver::descriptor()`:`icon_style = Plain`(或新增 `icons/odbc.svg`)、默认端口 0、通用能力;`dialect()` 依据 `odbc.engine` 返回。
4. `DriverDescriptor` 增加 **`ConnectionFormSpec`**(与 `DatabaseEditorSpec` 同路子),`OdbcDriver` 声明 `odbc: true`;`app/form.rs` 据此渲染:
   - **驱动程序**下拉 —— 用 `Environment::drivers()` 枚举系统已装 ODBC 驱动(复刻 Navicat 那个列表);
   - **DSN / 连接串**(二选一);
   - **底层数据库类型**(写入 `odbc.engine`)。
5. `Connection` 实现:`list_databases` / `list_tables` / `columns` / `fetch_page` / `execute_query` / `update_rows` / `insert_rows` / `delete_rows`(基于编目函数 + 方言 SQL)。
6. **阻塞与并发**:ODBC 同步且句柄非 `Sync` → 每个操作包 `Runtime::spawn_blocking`(导入 Excel 已有先例),内部 `Mutex<Connection>` 串行化(与 tiberius 处理类似)。
7. **方言与能力**:`odbc.engine` 映射到 `DriverDialect`(SQL Server / Oracle / PostgreSQL / Generic),SQL 补全函数表、引号风格走 Phase 3 的方言;`supports_*` 按底层类型探测,拿不到的能力返回空。
8. **错误映射**:缺驱动 / 32-64 位不匹配 / 连接失败给出可读提示;「停止查询」用 `SQLCancel`(弱于 native,UI 标注限制)。

**运行环境要求(需在文档中告知用户)**:
- Windows 自带 Driver Manager,但仍需用户安装**具体 ODBC 驱动**;
- Linux / macOS 需先装 unixODBC / iODBC 运行时,再装驱动;
- 构建 `rustgrid-odbc` 时 Unix 需对应开发库。

**验收**:不开启 feature 时,二进制、依赖、默认构建**完全不受影响**;开启后能枚举系统驱动、连接并浏览/查询一个 ODBC 数据源。

---

## 5. Todo List

### Phase 0 — descriptor 骨架
- [ ] 在 `rustgrid-core` 新增 `DriverDescriptor` + `DriverIconStyle`
- [ ] 新增 `DriverCapability` + `DriverCapabilities`(无新依赖)
- [ ] `Driver::descriptor()` 默认实现(组合现有方法)
- [ ] `lib.rs` 导出新类型
- [ ] `cargo build` 全绿,无调用点改动

### Phase 1 — 元信息与能力收敛
- [ ] MySQL/MariaDB 覆写 `descriptor()`(SolidBadge + 图标 + order)
- [ ] SQLite 覆写 `descriptor()`(Brand 蓝 + 无 DB/Users/Routines 能力)
- [ ] SQL Server 覆写 `descriptor()`(Brand 红 + Schemas 等 + database_editor)
- [ ] `app/mod.rs::driver_icon` 改读 descriptor
- [ ] `app/mod.rs::tree_driver_icon` 改按 `icon_style` 渲染,删除 `mysql/mariadb/sqlite/sqlserver` 特判
- [ ] 删除 app 私有 `DriverCapability`,`driver_supports` 改走 descriptor
- [ ] `registry.rs::drivers()` 按 `(order, display_name)` 稳定排序
- [ ] `render_connect_menu` 由 registry 生成启用项;灰显占位改 `PLANNED_ENGINES` 常量
- [ ] 核对四引擎图标/色/菜单顺序/能力开关与改动前一致
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 全绿

### Phase 2 — DriverSource
- [ ] core 增加 `BuiltinDriverSource` + `DriverSource` 实现
- [ ] `main.rs` 改为 `register_source(...)`
- [ ] 评估是否新增 `rustgrid-drivers` 门面 crate(可选)
- [ ] 构建/测试全绿

### Phase 3 — 方言数据化
- [ ] core 新增 `DriverDialect`(含 `builtin_functions`)
- [ ] `Driver::dialect()` + 四驱动覆写
- [ ] `sql.rs::functions_for(dialect)` 替换 id 版本
- [ ] `sql_completion.rs` scope 改存 dialect
- [ ] `sql.rs:1024` 断言更新
- [ ] 构建/测试全绿

### Phase 4 — 超时边界(可选)
- [ ] `Runtime::spawn_with_timeout`
- [ ] 连接 / execute_query / fetch_page 包超时
- [ ] 超时错误映射 + UI 验证
- [ ] 构建/测试全绿

### Phase 5 — 连接健康(延后)
- [ ] 评估需求后再排期

### Phase 6 — ODBC 可选引擎(默认关闭)
- [ ] 新建 `crates/rustgrid-odbc`,加入 workspace members/dependencies
- [ ] `odbc-api` 依赖 + `driver-odbc` feature(gated)
- [ ] `OdbcDriver::descriptor()`(Plain 图标、`odbc: true`、通用能力)
- [ ] `DriverDescriptor` 增加 `ConnectionFormSpec`;`app/form.rs` 渲染 ODBC 字段
- [ ] `Environment::drivers()` 枚举系统驱动填充下拉
- [ ] `Connection` 实现:`list_databases/tables/columns/fetch_page/execute_query/update/insert/delete`
- [ ] 阻塞调用统一 `spawn_blocking` + `Mutex<Connection>`
- [ ] `odbc.engine` → `DriverDialect` / 能力映射
- [ ] 错误映射(缺驱动/位数不符/连接失败)可读化
- [ ] 文档:unixODBC/iODBC 与 ODBC 驱动安装说明
- [ ] 验证 feature 开/关两种构建均 `test` / `clippy` 全绿
- [ ] 确认关闭 feature 时二进制体积与依赖不变

### 收尾
- [ ] 更新 `AGENTS.md`:Repository layout / Extensibility requirements 反映 descriptor + DriverSource
- [ ] 更新 `docs/sqlserver-driver.md` 中「驱动注册」相关描述
- [ ] 全量 `cargo test --workspace` + `cargo clippy` + `cargo build`

---

## 6. 构建与验证命令(本机 Windows GNU 工具链)

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="gcc"
$env:CC="gcc"

cargo fmt --all
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build
```

> 若跑 release,另加 `$env:GPUI_FXC_PATH=(Resolve-Path tools\fxc-shim\fxc.exe).Path`(见 AGENTS.md Gotchas)。

---

## 7. 风险与回滚

| 风险 | 缓解 |
|---|---|
| `Driver` trait 扩展影响 4 个 crate | Phase 0 全默认实现,编译不破坏;逐步 override |
| descriptor 与现有 `supports_*` 双份真相 | Phase 1 末让 `driver_supports` 只读 descriptor;后续可把 `supports_*` 降级为私有/删除 |
| 菜单顺序变化(registry 无序) | `order` 字段 + 稳定排序,人工核对 |
| 图标字符串进 core 引起分层争议(见 D2) | 备选:core 只给 `icon_style` 枚举,路径留 app |
| 超时误杀长查询 | 阈值可配置,仅对交互查询默认生效 |
| ODBC 引入 Unix 构建依赖(unixODBC/iODBC) | 独立 crate + feature 化,默认不开;CI 分「开/关」两种组合构建 |
| ODBC 驱动质量参差、元数据不一致 | 仅定位为长尾兜底;`odbc.engine` 分方言;错误可读化 |
| ODBC 同步阻塞拖慢 UI | 统一 `spawn_blocking` + `Mutex<Connection>` |
| 用户机器无对应 ODBC 驱动 | 连接时枚举驱动并给出明确安装提示 |

**回滚**:每个 Phase 独立提交;Phase 1 起若图标/菜单异常,可先只保留 descriptor 读取、暂不删除旧 match;Phase 6 可整体通过关闭 feature 回退。

---

## 8. 非目标(明确不做)

- ❌ Agent 独立进程 / JSON-RPC / sidecar
- ❌ JRE 管理、运行时下载与升级驱动(ODBC 驱动由用户系统层安装,不在本方案内下载)
- ❌ 外部 JSON 清单解析
- ❌ 新增 **native** 引擎(PostgreSQL/Oracle/MongoDB 仍为灰显占位;其长尾覆盖走 Phase 6 的可选 ODBC 引擎)
- ❌ 用 ODBC **替换** native MySQL/SQLite/SQL Server
- ❌ 默认开启 ODBC(保持单二进制、零额外运行时的默认体验)

**已明确采用(见 D4 / Phase 6)**:native 为主 + ODBC 作为**可选引擎**(默认关闭)。

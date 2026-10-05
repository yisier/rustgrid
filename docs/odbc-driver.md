# ODBC 引擎

RustGrid 用一个**通用 ODBC 驱动**兜底那些没有原生 Rust 驱动的数据库（Oracle、DB2、达梦、
Informix……）：它通过用户系统里已安装的 ODBC 驱动连接，并直接执行该引擎的 SQL。

它**默认开启**（app 特性 `driver-odbc`），并且**不引入任何链接期或强运行时依赖**：ODBC
Driver Manager（`odbc32.dll` / `libodbc.so` / iODBC）是在**首次使用时运行时加载**的
（`crates/rustgrid-odbc/src/api.rs` 用 `libloading`）。因此：

- 构建不需要 `unixODBC` 开发库；
- 没装 ODBC 的机器照常启动，ODBC 引擎只是「没有可用驱动 / 连接会报错」，不会让程序崩溃；
- 想要一个不含 ODBC 的精简构建，用 `--no-default-features`。

这与 Navicat 的做法一致：驱动管理器是外部可选项，不是编进程序的。

## 构建

```powershell
# 默认即包含 ODBC
cargo build -p rustgrid-app

# 不含 ODBC 的精简构建
cargo build -p rustgrid-app --no-default-features
```

## 运行环境要求

| 平台 | 需要安装 |
|---|---|
| Windows | 系统自带 Driver Manager；具体驱动**单独安装**，例如 `ODBC Driver 18/17 for SQL Server`、Oracle Instant Client 的 ODBC 驱动等 |
| Linux | `unixODBC`（`libodbc.so.2`）运行时 + 对应驱动。**只在真正使用 ODBC 时才需要** |
| macOS | iODBC 或 unixODBC + 对应驱动 |

> 构建时不需要任何 ODBC 开发库——这是运行时加载带来的好处。

## 新建连接

开启后，**连接 → ODBC...** 会出现在引擎菜单里。常规页多出四个字段：

| 字段 | 存入 profile | 说明 |
|---|---|---|
| ODBC 驱动程序 | `odbc.driver` | 系统已安装的 ODBC 驱动名（可搜索下拉，来自 `SQLDrivers`） |
| 连接字符串 | `odbc.connection_string` | 完整连接串（**优先级最高**，填了就忽略下面几项） |
| DSN | `odbc.dsn` | 已配置的数据源名 |
| 底层数据库类型 | `odbc.engine` | 方言提示，如 `sqlserver`；用于选择分页语法 |

配置直接存在 `ConnectionProfile::options`（`BTreeMap`）里，**无需迁移 `connections.json`**；密码仍走
加密的 `secrets.json`。三者至少填一个：连接串 / DSN / 驱动名。

示例（SQL Server，走系统 ODBC 驱动）：
- ODBC 驱动程序：`ODBC Driver 17 for SQL Server`
- 网络地址 / 端口：`localhost` / `1433`
- 用户名 / 密码：`sa` / `•••`
- 底层数据库类型：`sqlserver`

> 自签名证书（如 Docker 版 SQL Server）会让 `ODBC Driver 17/18` 的证书校验失败。此时改用
> 「连接字符串」字段并加上 `TrustServerCertificate=yes;`：
> `Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;UID=sa;PWD=...;TrustServerCertificate=yes;`

## 已实现 / 未实现

**已实现**：连接、列出数据库 / 表 / 视图、读取列、分页取数、执行任意 SQL（结果按文本显示）、
行编辑（增 / 删 / 改，参数化 `SQLBindParameter`）、表管理（删表 / 清空 / 截断 / 重命名，按方言）、
建库 / 删库 SQL、驱动枚举、连接失败识别为认证错误（触发密码提示）。

> ODBC 默认**隐藏**数据库管理入口（`supports_database_management = false`）：建库/删库语句可用，
> 但各引擎的库选项对话框差异太大，暂不暴露。

**暂未实现**（返回「not supported by the ODBC driver yet」）：

- 表设计器（读取 / 修改表结构）——注意 UI 本身是**故意留的 stub**，按钮禁用
- 备份 / 还原
- 用户 / 存储例程 / 视图管理

> 这些是**引擎相关**功能；对 SQL Server 这类有原生驱动的引擎，应直接使用原生驱动
> （`rustgrid-sqlserver` 等），它们已完整实现上述能力。ODBC 只做引擎无关的部分。

## 测试

真机集成测试（默认 `#[ignore]`）：

```powershell
$env:RUSTGRID_ODBC_CONNECTION_STRING="Driver={ODBC Driver 17 for SQL Server};Server=localhost,1433;UID=sa;PWD=...;TrustServerCertificate=yes;"
cargo test -p rustgrid-odbc -- --ignored --nocapture
```

覆盖：驱动枚举、列出全部数据库 / 表 / 视图、列、建表、插入（含 NULL）、更新、删除、分页、截断、删表。

## 已知限制

- ODBC 是**同步阻塞** API：每次操作在调用线程上直接完成（连接用一个 `Mutex` 串行化）。
- 所有值以**文本**读取（`SQLGetData` + `SQL_C_WCHAR`），数值/日期不做类型化还原。
- 元数据（数据库/表/列）依赖 ODBC 驱动自身实现质量，不同驱动差异较大。
- 分页按 `odbc.engine` 选择 `LIMIT/OFFSET`（默认）或 `OFFSET … FETCH`（`sqlserver`）。
- 运行时加载只在 `SQL_OV_ODBC3` 环境中工作；极老的驱动可能不兼容。

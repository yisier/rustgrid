# 用户与权限管理

本文说明 RustGrid 的账户（用户/角色）与权限管理：一套引擎无关的模型与 UI，如何通过
`Driver` / `Connection` 抽象适配 MySQL/MariaDB、SQL Server、PostgreSQL、Oracle，以及各引擎
的差异与仍未实现的部分。

对应 AGENTS.md 的 "User management"（scope 第 10 条）。

## 分层

```
rustgrid-core            引擎无关模型 + Driver/Connection 抽象
  user.rs                UserAccount / UserDetails / UserEdit、
                         ObjectGrant / ObjectPrivilegeRow / RoleMembership、
                         PrivilegeId / PrivilegeInfo / PrivilegeGroup /
                         PrivilegePreset / PrivilegeCatalog / PrivilegeScope
  driver.rs              Connection 的账户方法、Driver::user_editor() -> UserEditorSpec

rustgrid-<engine>        每个驱动实现 Connection 的账户方法，并返回自己的
                         PrivilegeCatalog；账户属性/权限名/DDL 全部在驱动内翻译

rustgrid-app             UI（只依赖 core）
  src/app/user.rs            Users 主标签页（账户列表 + 工具栏 + 删除）
  src/app/user_create.rs     新建/编辑用户窗口（UserCreateDialog）
  src/app/privilege_manager.rs  对象权限管理器窗口（PrivilegeManager）
  locales/{en,zh-CN}.yml     全部文案
```

UI 层**不**出现任何引擎判断或原始 SQL：所有差异通过 `UserEditorSpec`（界面布局）与
`PrivilegeCatalog`（权限目录）下发，账户的读写 SQL 由驱动生成。

## 核心模型（`rustgrid-core/src/user.rs`）

- `UserAccount`：账户属性。含引擎无关字段与各引擎专属字段（MySQL 的 `host`/`plugin`/
  `ssl_*`/资源限制、PostgreSQL 的 `password_valid_until`、Oracle 的
  `default_tablespace`/`profile`/`tablespace_quota`）。
- `UserDetails`：账户 + 服务器级 `server_privileges` / `denied_server_privileges` +
  对象级 `grants: Vec<ObjectGrant>` + 角色边 `roles`/`members`。
- `UserEdit`：编辑器保存时下发的编辑（`original`、`account`、`password`、
  `server_privileges`、`denied_server_privileges`、`grants`、`roles`、`members`）。
- `ObjectGrant { database, schema, name, privileges }`：一条对象级授权；`name` 为空表示
  数据库级，`schema` 为空表示无 schema 概念。`object_name()` 给出 `schema.name` 显示键。
- `ObjectPrivilegeRow { user, host, privileges, denied }`：对象权限管理器矩阵中的一行
  （某账号在某对象上的授权与显式拒绝）。
- `RoleMembership`：一条角色边。
- `PrivilegeId(String)`：权限身份，即引擎在 `GRANT` 里用的关键字（`"SELECT"`、
  `"CONTROL SERVER"`、`"CREATE ANY TABLE"`），大小写不敏感比较。
- `PrivilegeScope { Server, Database, Schema, Object }`：权限可授予的作用域。
- `PrivilegeInfo { id, scopes, groups, label_key, label }`：目录中的一条权限；`groups` 是
  `(scope, group id)` 列表，因此同一权限在不同作用域可归入不同分组。构造器：
  `server` / `database` / `schema` / `object` / `both`（server+object）/
  `database_and_schema` / `database_and_object`。
- `PrivilegeGroup`、`PrivilegePreset`（一键预设）。
- `PrivilegeCatalog { groups, privileges, server_presets, object_presets, deny_supported }`：
  驱动返回的完整目录。`deny_supported` 为 `true` 时 UI 为每项提供“拒绝”开关。

## 驱动契约

`Driver::user_editor() -> UserEditorSpec`：描述新建/编辑用户窗口显示哪些字段与分区，例如
`host`、`authentication_plugin`、`password_expiry`、`password_valid_until`、
`default_tablespace`、`tablespace_quota`、`profile`、`server_privileges`、
`object_privileges`（账号编辑器里的对象级分区）、`object_privilege_manager`（对象权限管理器
工具栏项，可独立开启）、`roles`、`list_super_user`。默认 `UserEditorSpec::mysql()`。

`Connection` 的账户方法（均有默认实现，默认报“不支持”）：

| 方法 | 用途 |
|---|---|
| `list_users` | Users 主标签页的账户列表 |
| `privilege_catalog` | 该引擎的权限目录 |
| `user_details` | 载入某账户的完整可编辑状态 |
| `save_user` | 按 `UserEdit` 创建/修改账户并替换其权限/角色 |
| `user_edit_sql` / `user_edit_groups` | SQL 预览与“确认并执行”的分组脚本 |
| `drop_user` / `rename_user` | 删除 / 重命名账户 |
| `authentication_plugins` / `ssl_types` | 编辑器下拉选项 |
| `object_privilege_matrix` | 某对象上所有账号的权限矩阵 |
| `set_object_privileges` / `object_privileges_sql` | 写入矩阵 / 预览其脚本 |

### 对象名的编码约定

对象权限管理器的 `name` 参数同时表达三类对象：

| `name` | 含义 |
|---|---|
| `""` | 整个数据库 |
| `"schema.*"` | 一个 schema |
| `"schema.table"` 或 `"table"` | 表/视图（无 schema 的引擎用裸名） |

驱动的 `object_privilege_matrix` / `set_object_privileges` / `object_privileges_sql`
按此解析。账号编辑器的 `权限` 分区目前只处理数据库级与表级授权。

## UI

### Users 主标签页（`user.rs`）

账户列表（详细列表/平铺网格两种布局，来自共享的 `ui` 列表模板），工具栏：编辑 / 新建 /
删除 / 权限管理器（仅当 `object_privilege_manager` 为真）。列表列由 `UserEditorSpec` 决定
（例如 Oracle 无 `super_user` 列）。

### 新建/编辑用户窗口（`user_create.rs`，独立 OS 窗口）

左导航分区（按 `UserEditorSpec` 显示）：常规 / 服务器权限 / 权限 / 角色 / SQL 预览。
- 常规：身份（`user`、`host` + 快捷芯片、认证插件）、密码、账号状态（过期策略 /
  密码有效期至 / 锁定）、资源限制、存储与配置（Oracle 的默认表空间 / 配额 / PROFILE）。
- 服务器权限：按目录分组的勾选网格 + 一键预设；`deny_supported` 时每项带“拒绝”开关，
  授权与拒绝互斥。
- 权限：数据库列表 + 细化勾选（数据库级 / 指定具体表）。
- 角色：成员属于 / 成员两张表，`集` 列授予 `WITH ADMIN OPTION`。
- SQL 预览：保存将执行的脚本。

保存前经“确认并执行”对话框展示按 `UserEditSection` 分组的变更脚本。

### 对象权限管理器（`privilege_manager.rs`，独立 OS 窗口）

对象在左（数据库 → schema → 表，按作用域显示相应网格），右侧列出账号（勾选即授予）与
选中账号的细粒度勾选；`deny_supported` 时每项带“拒绝”开关。`name` 的三类编码见上。
保存同样先显示变更预览。

### 权限显示

勾选行显示**引擎原始关键字**（如 `CONTROL SERVER`、`USAGE`），悬浮气泡显示本地化的名称
（`PrivilegeTooltip`），避免翻译歧义。分组标题与预设名始终本地化。

## 各引擎映射

| | MySQL/MariaDB | SQL Server | PostgreSQL | Oracle |
|---|---|---|---|---|
| 身份 | `user@host` | 登录名 | 角色名 | 用户名 |
| 服务器级 | `mysql.user` 的 28 项 | `sys.server_permissions`（目录 13 项） | 角色属性（SUPERUSER/CREATEDB/CREATEROLE/REPLICATION/BYPASSRLS） | 系统权限（目录 14 项） |
| 数据库级 | （复用对象级 `db.*`） | `ON DATABASE::db`（class 0） | `ON DATABASE db`（CONNECT/CREATE/TEMPORARY） | — |
| schema 级 | — | — | `ON SCHEMA`（CREATE/USAGE） | — |
| 对象级 | 表/库/例程 | 表/视图（class 1） | 表/视图 | 表/视图 |
| 角色 | `mysql.role_edges` | 服务器角色 | `pg_auth_members` | `dba_role_privs` |
| DENY | — | ✅（对象级 + 服务器级） | — | — |
| 专属属性 | plugin/ssl/资源限制 | — | `VALID UNTIL` | PROFILE / 默认表空间 / 配额 |

### 驱动实现要点

- **MySQL**（`rustgrid-mysql/src/user.rs`）：账户在 `mysql.user`；服务器权限取
  `information_schema.USER_PRIVILEGES`，对象授权取 `SCHEMA_/TABLE_/ROUTINE_PRIVILEGES`；
  写入回放 `CREATE/ALTER USER`、`GRANT/REVOKE`、`RENAME USER` 脚本（文本协议），结尾
  `FLUSH PRIVILEGES`。
- **SQL Server**（`rustgrid-sqlserver/src/user.rs`）：账户是**登录**（无 host）；服务器
  权限取 `sys.server_permissions`（`G`/`W`→授予，`D`→拒绝，sysadmin 显示目录全集）；
  对象权限矩阵取 `sys.database_permissions`，主体名用 `SUSER_SNAME` 对齐登录名；给尚无库
  用户的登录授权时先发一条带守卫的 `CREATE USER … FOR LOGIN …`。带 `DENY` 的三态差量生成
  `REVOKE` + `GRANT`/`DENY`。
- **PostgreSQL**（`rustgrid-postgresql/src/user.rs`）：角色属性以 `ALTER ROLE …` 写入；
  `rolvaliduntil` 经 `VALID UNTIL` 读写；对象/数据库/schema 授权分别读
  `pg_class.relacl` / `pg_database.datacl` / `pg_namespace.nspacl`（`aclexplode`）。
- **Oracle**（`rustgrid-oracle/src/user.rs`）：账户在 `dba_users`/`all_users`；系统权限取
  `dba_sys_privs`，对象授权取 `dba_tab_privs`；`PROFILE` / `DEFAULT TABLESPACE` /
  `QUOTA … ON …` 随 `CREATE/ALTER USER` 写入；配额读 `dba_ts_quotas`。

## 仍未实现（后续工作）

1. **PostgreSQL `ALTER DEFAULT PRIVILEGES`**（未来新建对象的默认权限）：需要 core 模型 +
   账户编辑器新分区（按 schema / 对象类型 / 角色）+ 驱动读写 `pg_default_acl`。
2. **Oracle 多表空间配额列表**：现在只管理默认表空间那一条（`UserAccount::tablespace_quota`）；
   改为“每表空间的配额映射”，并在「存储与配置」分区做动态行 UI（含增删）。
3. **账号编辑器 `权限` 分区里的 PG schema 授权**：目前 schema 授权只在对象权限管理器里。
4. （可选）SQL Server 登录名 → 数据库用户名的映射更完整（自定义用户名、映射账号）。

## 构建与验证

Windows 需用 GNU 工具链（见 AGENTS.md 的 Gotchas）：

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="gcc"
$env:CC="gcc"
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build
```

各引擎的实时集成测试（默认 ignored）：`rustgrid-mysql` / `rustgrid-sqlserver` /
`rustgrid-postgresql` / `rustgrid-oracle`，分别设置 `RUSTGRID_MYSQL_PASSWORD` /
`RUSTGRID_SQLSERVER_PASSWORD` / `RUSTGRID_PG_PASSWORD` / `RUSTGRID_ORACLE_PASSWORD` 后加
`-- --ignored`。

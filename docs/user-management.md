# 用户与权限管理

本文说明 RustGrid 的账户（用户/角色）与权限管理：一套引擎无关的模型与 UI，如何通过
`Driver` / `Connection` 抽象适配 MySQL/MariaDB、SQL Server、PostgreSQL、Oracle，以及各引擎
的差异与仍未实现的部分。

对应 AGENTS.md 的 "User management"（scope 第 10 条）。

## 分层

```
rustgrid-core            引擎无关模型 + Driver/Connection 抽象
  user.rs                UserAccount / UserDetails / UserEdit、
                         UserMapping / ServerSecurableGrant / SecurableClass、
                         ObjectGrant / ObjectPrivilegeRow / RoleMembership、
                         DefaultPrivilege / DefaultObjectType / DefaultPrivilegeInfo、
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
  `default_tablespace`/`profile`/`tablespace_quota`、SQL Server 的
  `login_type`/`default_database`/`default_language`/`check_policy`/`check_expiration`/
  `must_change`/`certificate`/`asymmetric_key`/`credential`）。
- `UserDetails`：账户 + 服务器级 `server_privileges` / `denied_server_privileges` /
  `grant_option_server_privileges`（SQL Server 的 `state = 'W'`，可转授）+ 对象级
  `grants: Vec<ObjectGrant>` + 角色边 `roles`/`members`。
- `UserEdit`：编辑器保存时下发的编辑（`original`、`account`、`password`、`old_password`、
  `server_privileges`、`denied_server_privileges`、`grant_option_server_privileges`、`grants`、
  `roles`、`members`、`mappings`/`original_mappings`、`securables`/`original_securables`）。
- `UserMapping { database, mapped, user_name, default_schema, roles, available_roles }`：
  SQL Server 的“用户映射”一行（登录在某个库里的数据库用户与库角色）。
- `ServerSecurableGrant { class, name, privileges, denied }`：一条服务器级安全对象授权
  （`class` 为 `ENDPOINT`/`LOGIN`，用于 `ON <class>::<name>`）。
- `SecurableClass { id, class, label*, privileges }`：编辑器“终端节点权限/登录权限”分区对应的
  安全对象类别及其可授予权限；`id` 是稳定键（`endpoint`/`login`）。
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
- `PrivilegeCatalog { groups, privileges, server_presets, object_presets, deny_supported,
  grant_option_supported, securable_classes }`：驱动返回的完整目录。`deny_supported` 为真时
  UI 为服务器权限提供“拒绝”列；`grant_option_supported` 为真时提供“含授予选项”列
  （SQL Server 的 `WITH GRANT OPTION`）；`securable_classes` 列出服务器级安全对象类别。

## 驱动契约

`Driver::user_editor() -> UserEditorSpec`：描述新建/编辑用户窗口显示哪些字段与分区，例如
`host`、`authentication_plugin`、`verification_type`（SQL Server 的验证类型）、
`password_expiry`、`password_valid_until`、`default_tablespace`、`tablespace_quota`、`profile`、
`server_privileges`、`object_privileges`（账号编辑器里的对象级分区）、`default_privileges`
（默认权限分区）、`user_mapping`（SQL Server 用户映射分区）、`endpoint_permissions` /
`login_permissions`（SQL Server 服务器安全对象分区）、`object_privilege_manager`（对象权限
管理器工具栏项，可独立开启）、`roles`、`account_enabled`、`list_super_user`。默认
`UserEditorSpec::mysql()`。

`UserDetails` / `UserEdit` 通过 `default_privileges: Vec<DefaultPrivilege>` 承载默认权限规则
（`schema`、`object_type`、`grantee`、`privileges`；`FOR ROLE` 即被编辑的账号本身，不随规则存储）。
`PrivilegeCatalog::default_privileges` 列出每种对象类型可授予的权限。

`Connection` 的账户方法（均有默认实现，默认报“不支持”）：

| 方法 | 用途 |
|---|---|
| `list_users` | Users 主标签页的账户列表 |
| `privilege_catalog` | 该引擎的权限目录 |
| `user_details` | 载入某账户的完整可编辑状态 |
| `save_user` | 按 `UserEdit` 创建/修改账户并替换其权限/角色 |
| `user_edit_sql` / `user_edit_groups` | SQL 预览与“确认并执行”的分组脚本 |
| `drop_user` / `rename_user` | 删除 / 重命名账户 |
| `authentication_plugins` / `ssl_types` / `login_types` | 编辑器下拉选项 |
| `user_mappings` | SQL Server 的用户映射（每个库的数据库用户与库角色） |
| `user_securables` | SQL Server 的服务器安全对象授权（端点 / 登录） |
| `default_privilege_schemas` | 默认权限分区列出的 schema（驱动读写默认权限所在库） |
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

左导航分区（按 `UserEditorSpec` 显示）：常规 / 服务器权限 / 权限 / 默认权限 / 角色 / 用户映射 /
终端节点权限 / 登录权限 / SQL 预览。
- 常规：身份（`user`、`host` + 快捷芯片、认证插件、SQL Server 验证类型）、密码（含“指定旧
  密码”）、账号状态（过期策略 / 密码有效期至 / 锁定或已启用）、资源限制、存储与配置（Oracle 的
  默认表空间 / 配额 / PROFILE）、登录选项（SQL Server 的密码策略 / 过期 / 下次登录改密、默认
  数据库 / 语言、证书 / 非对称密钥 / 凭据）。
- 服务器权限：Navicat 风格的三列表格（`权限 | 授予 | 含授予选项 | 拒绝`）。`grant_option_supported`
  时显示「含授予选项」（勾选即同时授予，可转授），`deny_supported` 时显示「拒绝」；授予/含授予
  选项/拒绝三态互斥，模板一键替换整组授予。
- 权限：数据库列表 + 细化勾选（数据库级 / 指定具体表）。
- 默认权限（PostgreSQL）：左侧 schema 列表（含“所有 Schema”），右侧对象类型（表 / 序列 /
  函数 / 类型 / Schema）+ 授权角色列表 + 细化勾选，编辑 `ALTER DEFAULT PRIVILEGES` 规则。
- 角色：成员属于 / 成员两张表，`集` 列授予 `WITH ADMIN OPTION`。
- 用户映射（SQL Server）：左侧数据库列表（勾选即映射），右侧为该库的数据库用户名 / 默认架构
  与库角色勾选。
- 终端节点权限 / 登录权限（SQL Server）：安全对象 × 权限的矩阵；驱动在 `PrivilegeCatalog` 的
  `securable_classes` 里给出类别与权限，`deny_supported` 时每格带“拒绝”开关。
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
| 用户映射 | — | `sys.database_principals`/`sys.database_role_members` | — | — |
| 安全对象 | — | `ON ENDPOINT::` / `ON LOGIN::`（`sys.server_permissions` class 101/105） | — | — |
| DENY | — | ✅（对象级 + 服务器级 + 安全对象） | — | — |
| 含授予选项 | — | ✅（服务器级，`state = 'W'`） | — | — |
| 默认权限 | — | — | `pg_default_acl`（按 schema / 对象类型 / 角色） | — |
| 专属属性 | plugin/ssl/资源限制 | 验证类型 / 默认库 / 默认语言 / 密码策略 / 证书 / 凭据 | `VALID UNTIL` | PROFILE / 默认表空间 / 配额 |

### 驱动实现要点

- **MySQL**（`rustgrid-mysql/src/user.rs`）：账户在 `mysql.user`；服务器权限取
  `information_schema.USER_PRIVILEGES`，对象授权取 `SCHEMA_/TABLE_/ROUTINE_PRIVILEGES`；
  写入回放 `CREATE/ALTER USER`、`GRANT/REVOKE`、`RENAME USER` 脚本（文本协议），结尾
  `FLUSH PRIVILEGES`。
- **SQL Server**（`rustgrid-sqlserver/src/user.rs`）：账户是**登录**（无 host）；完整属性
  （默认库/语言、密码策略/过期、验证类型、凭据）取自 `sys.server_principals` 连接
  `sys.sql_logins`；服务器权限取 `sys.server_permissions`（`class = 100`，`G`→授予，
  `W`→授予且可转授，`D`→拒绝，sysadmin 显示目录全集）；对象权限矩阵取 `sys.database_permissions`，主体名用
  `SUSER_SNAME` 对齐登录名；给尚无库用户的登录授权时先发一条带守卫的
  `CREATE USER … FOR LOGIN …`。带 `DENY` 的三态差量生成 `REVOKE` + `GRANT`/`DENY`。
  用户映射按 SID 匹配 `sys.database_principals` 并读 `sys.database_role_members`，逐库差量生成
  `USE [db]` 后的 `CREATE USER … FOR LOGIN` / `ALTER USER … DEFAULT_SCHEMA` / `ALTER ROLE … ADD/DROP MEMBER`。
  终端节点权限 / 登录权限读 `sys.server_permissions` 的 class 105（端点）/101（登录），差量生成
  `GRANT/DENY/REVOKE … ON <CLASS>::[name]`。登录创建按验证类型生成
  `CREATE LOGIN … WITH PASSWORD … [MUST_CHANGE]` / `FROM WINDOWS` / `FROM CERTIFICATE` / `FROM ASYMMETRIC KEY`。
- **PostgreSQL**（`rustgrid-postgresql/src/user.rs`）：角色属性以 `ALTER ROLE …` 写入；
  `rolvaliduntil` 经 `VALID UNTIL` 读写；对象/数据库/schema 授权分别读
  `pg_class.relacl` / `pg_database.datacl` / `pg_namespace.nspacl`（`aclexplode`）。默认权限读
  `pg_default_acl`（`aclexplode`，跳过 `PUBLIC`/`grantee = 0`），按 `(schema, 对象类型, 角色)`
  差量生成 `ALTER DEFAULT PRIVILEGES FOR ROLE … [IN SCHEMA …] GRANT/REVOKE … ON … TO/FROM …`；
  schema 列表取自该驱动读写默认权限所在的默认库。
- **Oracle**（`rustgrid-oracle/src/user.rs`）：账户在 `dba_users`/`all_users`；系统权限取
  `dba_sys_privs`，对象授权取 `dba_tab_privs`；`PROFILE` / `DEFAULT TABLESPACE` /
  `QUOTA … ON …` 随 `CREATE/ALTER USER` 写入；配额读 `dba_ts_quotas`。

## 仍未实现（后续工作）

1. **Oracle 多表空间配额列表**：现在只管理默认表空间那一条（`UserAccount::tablespace_quota`）；
   改为“每表空间的配额映射”，并在「存储与配置」分区做动态行 UI（含增删）。
2. **账号编辑器 `权限` 分区里的 PG schema 授权**：目前 schema 授权只在对象权限管理器里。

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

# Oracle 引擎

RustGrid 的 Oracle 支持是一个**原生 Rust 引擎**（`crates/rustgrid-oracle`），不是走 ODBC 兜底。
它建立在 **Oracle 官方的纯 Rust thin 驱动 [`oracledb`](https://crates.io/crates/oracledb)**
（`rust-oracledb`）之上，**无需 OCI / Instant Client**——与其他引擎一样，装好 RustGrid 即可连接。

> 通用 ODBC 驱动（`docs/odbc-driver.md`）仍然保留，用来连接 DB2、达梦等引擎；Oracle 已从
> "via generic ODBC" 提升为同级原生引擎。

## 现状与版本

- 依赖 **`oracledb = "=26.0.0-beta.4"`**，**锁死精确版本**：它是 2026 年的预发布版，官方明示
  API 与功能仍会变动。升级前请先跑一遍 live 测试。
- 依赖**无条件进入默认构建**（与 PostgreSQL 同级），不需要任何 feature 开关，也**不新增原生
  编译负担**：TLS 复用工作区已有的 `rustls 0.23`。
- 所有 `oracledb` 调用都收敛在 `connection.rs` / `driver.rs`，升级时改动面最小。

## 架构要点

- **一个连接池**：Oracle 的连接绑定到某个 service/PDB，而**一条连接就能看到该 PDB 里所有
  schema 的对象**，所以不需要 PostgreSQL 那种"每个数据库一个池"的复杂结构。
- **同步 API 的桥接**：`oracledb` 是阻塞式 API。驱动的每个 `Connection` 方法都在
  `tokio::task::spawn_blocking` 里从池中取一条连接、跑完全部阻塞调用、返回 owned 结果
  （`connection.rs::with_conn`）。因为 `oracledb::Pool` **没有实现 `Clone`**，池用
  `Arc<oracledb::Pool>` 共享。
- **schema 一等公民**：`supports_schemas = true`，树结构为 库（当前 PDB）→ schema（用户） →
  表 / 视图 / 函数 / 查询 / 备份。

## 新建连接

Oracle 的常规连接表单字段：

| 字段 | 存入 profile | 说明 |
|---|---|---|
| 网络地址 / 端口 | `host` / `port` | 默认端口 **1521** |
| 用户名 / 密码 | `username` + 加密的 `secrets.json` | 密码**不写入 profile** |
| 数据库 | `database` | 这里填 **service / PDB 名**（如 `FREEPDB1`）；也可用下面的 `oracle.service_name` |

`ConnectionProfile::options`（`BTreeMap`）里可选的 Oracle 专用项，**无需迁移
`connections.json`**：

| 键 | 说明 |
|---|---|
| `oracle.service_name` | service / PDB 名（优先于 `database` 字段） |
| `oracle.tns_alias` | `tnsnames.ora` 中的 TNS 别名 |
| `oracle.connect_string` | 完整连接串或双引号描述的 connect descriptor（**优先级最高**） |
| `oracle.wallet_location` | 钱包（`ewallet.pem`）目录，用于 TCPS |
| `oracle.wallet_password` | 钱包口令 |
| `oracle.config_dir` | 查找 `tnsnames.ora` 的目录 |

未填连接串 / 别名时，驱动按 **Easy Connect** 语法拼装：

- 明文：`host:port/service`
- TLS：`tcps://host:port/service`

**TLS / 钱包**：core 的 `TlsMode` 与 Oracle 的 "TCPS + 钱包" 不是一一对应——`Disabled` 用明文，
其余档位一律走 **TCPS**。证书 / 私钥路径不适用，身份校验由钱包承担。

**隧道**：SSH / SOCKS5 / HTTP 隧道与其它网络引擎共用 `rustgrid-tunnel`，连接串会指向本地转发端口。

**超时 / init SQL**：`query_timeout` 通过 `Connection::set_call_timeout` 生效；`init_sql` 在每次
取到连接后、执行操作前运行（`oracledb` 的池没有"新建连接回调"，所以只能逐次执行）。
`PoolConfig` **没有** TCP 连接超时入口，因此 `connect_timeout` 在 Oracle 上**不生效**；
`read_only` 也没有会话级开关（Oracle 只有事务级 `SET TRANSACTION READ ONLY`），同样不接线。

## 已实现

- 连接 / 断开、密码提示、认证失败识别（`ORA-01017`、`ORA-28000/28001/28002/28003` → 触发密码框）。
- 浏览：当前 service/PDB、schema 列表（隐藏 `SYS`/`SYSTEM`/`XDB`/`MDSYS` 等维护账号）、表 / 视图、
  列（类型 / 可空 / 主键 / 注释）、`OFFSET … FETCH` 分页、过滤树与排序。
- 编辑：网格内增 / 删 / 改（主键优先，`NULL` 键走 `IS NULL`）、任意 SQL（含多语句、PL/SQL 块、
  `q'[...]'`、带行尾分号的语句）。
- 表操作：Drop / Empty / Truncate / Rename；表设计器（列 / 索引 / 外键 / 注释）读写。
- 视图：列表 / 详情（`DBMS_METADATA.GET_DDL`）/ 保存（`CREATE OR REPLACE VIEW`）/ 删除。
- 例程：函数 / 过程 / 包 / 触发器列表（`ALL_OBJECTS`）、详情（`GET_DDL`）、保存、删除。
- 用户：用户 / 角色列表（`dba_users` 退 `all_users`）、详情、保存（`CREATE/ALTER USER` +
  `GRANT`/`REVOKE`）、删除（`DROP USER … CASCADE`）、对象权限矩阵（`dba_tab_privs`）。
- 备份 / 还原：`.rgbak`（表 + 视图 + 例程），行以 Oracle 字面量渲染，还原后重置 identity 序列。

## 已知差异与限制

- **空串 = NULL**：Oracle 的 `''` 就是 `NULL`。驱动读 `NULL` 为 `CellValue::Null`；写入空串会被
  存成 `NULL`（不可逆）；导出的 `.sql` 里空串渲染成 `NULL`。
- **脚本必须客户端切分**：`oracledb` 一次只跑一条语句，且拒绝行尾分号。驱动内置切分器（保留
  PL/SQL 块、`q'X...X'`、双引号标识符与注释），逐条剥离行尾 `;` 后在**同一条池连接**上顺序执行。
- **分页需要排序**：Oracle 的 `OFFSET … FETCH`（**12c+ 才支持**）强制要求 `ORDER BY`。驱动优先用
  主键；无主键的表用 `ROWID`（先探测，索引组织表 / 全局临时表等取不到时退化为 `ORDER BY 1`，
  此时跨页顺序不保证）；视图没有 `ROWID`，同样退化。
- **行数是估算值**：表列表的"行"来自 `ALL_TABLES.NUM_ROWS`（统计信息，未 `ANALYZE` 时为 `NULL`，
  驱动归零），**不是精确计数**。
- **DDL 一律走 `DBMS_METADATA`**：Oracle 没有 `SHOW CREATE`。取 DDL 前会在同一条连接上设置
  `SESSION_TRANSFORM`（去掉 `STORAGE`/`TABLESPACE`/`SEGMENT_ATTRIBUTES`），得到干净的脚本。
- **标识符大小写**：Oracle 把未加引号的标识符折叠为大写，驱动**一律按目录返回的原样名字**加双引号
  引用，**不做小写化**。
- **绑定参数**：core 传来的值都是文本；DATE / TIMESTAMP 列用 `TO_DATE` / `TO_TIMESTAMP` 包裹
  （不依赖会话的 `NLS_DATE_FORMAT`），`None` 渲染成 SQL `NULL` 而非空串。
- **schema = 用户**：`CREATE USER … NO AUTHENTICATION` 建纯 schema，`DROP USER`（不带 `CASCADE`，
  删非空 schema 报 `ORA-01922`）删 schema；Oracle **不支持重命名用户**，驱动的改用户名返回明确错误。
- **库管理关闭**：Oracle 建库是 DBA 级重量操作，无法映射到连接级对话框，
  `supports_database_management = false`，库管理入口隐藏；`character_sets` / `collations` /
  `storage_engines` / 事件调度器均为空。
- **`oracledb` 只读 `tnsnames.ora`**，不读 `sqlnet.ora` / `oraaccess.xml`。

## 真机集成测试

默认 `#[ignore]`，靠 `RUSTGRID_ORACLE_*` 环境变量启用：

```bash
# 起库（服务名通常为 FREEPDB1）
docker run -d --name rg-oracle -p 1521:1521 -e ORACLE_PASSWORD=oracle gvenzl/oracle-free:23-slim

export RUSTGRID_ORACLE_PASSWORD=oracle      # 可选 HOST/PORT/USER/SERVICE
cargo test -p rustgrid-oracle -- --ignored
```

测试覆盖：连接与认证失败映射、列库 / 列 schema / 列表 / 列列、分页、增删改、任意 SQL
（含多语句、PL/SQL 块、带分号语句）。

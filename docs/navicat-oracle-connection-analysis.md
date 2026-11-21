# Navicat Premium Lite 18 — Oracle 新建连接分析

> 本文档是一次逆向分析 + RustGrid 改造说明，**不提交到仓库**。
>
> 分析对象：`D:\Program Files\Navicat\Navicat Premium Lite 18`（文档里的路径按本机安装位置写）。
>
> 分析时间：2026-11-17。
>
> **状态说明（2026-11-17）**：第 6 节的界面改造实现后，因实际布局效果不理想已回退；第 6 节保留为下一次重设计的提案，第 2–5 节的分析结论仍然有效。

## 1. 结论摘要

Navicat 的 Oracle 连接表单之所以简单，是因为它把 Oracle 的连接方式收成了 **两个模式**：

- **Basic（基本）**：主机 + 端口 + 服务名（或 SID）+ 用户名 + 密码。
- **TNS**：TNS 别名（Net Service Name）+ 用户名 + 密码。

它默认只显示 Basic；TNS 和其它高级选项都在「连接类型」或高级设置里，不占用默认页面。底层依旧是 OCI + 连接描述符，但用户看不到描述符。

RustGrid 原来的 Oracle 表单把 4 个模式（服务名 / SID / TNS 别名 / 连接串）平铺在同一层，而且连接串是单行输入，导致：

- 默认页面比 Navicat 长；
- 用户很容易选中「连接串 / 描述符」，粘贴一段带空 `( )` 的文本，然后收到 `oracledb` 的原始 parse error；
- 选错模式后旧的字段仍然可见/生效。

本次改造把 RustGrid 的 Oracle 表单改成 **基本 / TNS / 高级** 三个模式，模式选择放在最上面，按模式切换字段，并把连接描述符改成「高级」tab 里的多行输入。

## 2. 分析方法与证据

### 2.1 REA 能做的部分

对 `navicat.exe` 使用了 REA 的：

- `inspect_with_objdump`（`file_headers` / `section_headers` / `symbols`）
- PE 结构由 GNU objdump 读取；`inspect_pe_resources`、`inspect_pe_imports` 因文件超过 64 MiB 的资源读取上限未能完整解析
- Ghidra 在本机未配置（`GHIDRA_INSTALL_DIR` 未设置），Hopper 不支持 Windows，Rizin 未安装，因此没有做函数级反编译

对 `libcc.dll` 和 `navicat.exe` 使用 GNU `strings` 读取可读字符串（UTF-16LE + ASCII）。

### 2.2 文件事实

| 文件 | 事实 |
| --- | --- |
| `navicat.exe` | 99,096,136 字节，PE32+ x86-64，无符号表，`.text` ~45 MB，`.rsrc` ~46 MB |
| `navicat.exe` 版本 | 18.0.5.0（Navicat Premium Lite 18） |
| `libcc.dll` | 134,816,840 字节，连接层（OCI / MySQL / PostgreSQL / SQL Server / MongoDB / Redis / SQLite） |
| `instantclient_23_0/oci.dll` | Oracle Call Interface 23.26.1.0 |
| `instantclient_23_0/oraociicus.dll` | 108,724,736 字节，Oracle 客户端库 |

`navicat.exe` 是 Delphi/C++Builder 系（字符串里可见 `System.Classes.TBasicAction`、`Vcl.StdCtrls` 等），不是 Qt。

### 2.3 Oracle 连接模式

`libcc.dll` 中的 schema 明确定义了：

```text
_REUSE_TYPE_CMConnMode_ORA: [ "Basic", "TNS" ]
_REUSE_TYPE_CMServiceNameType_ORA: [ "ServiceName", "SID" ]
_REUSE_TYPE_CMConnRole_ORA: [ "Default", "SYSDBA", "SYSOPER" ]
```

对应的 Oracle 服务器设置模板：

```text
ServiceProvider, Host, Port, UserName, SavePassword,
ConnectionMode, ServiceNameType, ServiceName, TNS,
AutoConnect, UseKeepAlive, KeepAliveInterval, SessionLimit,
ConnectionRole, UseOSAuthentication, ClientChartsetID, ClientEncoding,
CustomDatabaseList, UseSSH, SSH
```

由此可知 Navicat 的默认表单只覆盖了其中一小部分；其余字段在高级设置/SSH 等入口里。

### 2.4 Basic 模式的连接串

`libcc.dll` 里能找到以下连接描述符片段：

```text
(DESCRIPTION=(ADDRESS=(PROTOCOL=tcp)(HOST=...
)) (CONNECT_DATA=(
SERVICE_NAME
```

以及 OCI 调用：

```text
OCIEnvNlsCreate
OCIServerAttach
OCISessionBegin
AS SYSDBA
AS SYSOPER
LoadOCILibraries
```

说明 Basic 模式在内部构造的是：

```text
(DESCRIPTION=(ADDRESS=(PROTOCOL=tcp)(HOST=<host>)(PORT=<port>))
  (CONNECT_DATA=(SERVICE_NAME=<service>)))
```

SID 模式则把 `SERVICE_NAME` 换成 `SID`。TNS 模式把 `Conn.TNS` 别名直接交给 OCI，由 OCI 按照 `tnsnames.ora` 解析。

`libcc.dll` 里还能看到：

```text
cc_lib_oci:_load_library:unset NLS_LANG:
network\admin\tnsnames.ora
net80\admin\tnsnames.ora
```

即 Navicat 在加载 OCI 前会处理 `NLS_LANG`，并在 Oracle 客户端目录下查找 `tnsnames.ora`。

### 2.5 UI 字符串

`navicat.exe` 的 UTF-16 字符串表里能找到这些 Oracle 表单标签：

```text
Connection Name
Connection Type
Host
Port
Service Name
User Name
Password
Save password
Basic
Advanced
Role
OS Authentication
Client Character Set
Service Name/SID
Net Service Name
```

结合 `libcc.dll` 的绑定键：

```text
Conn.Host
Conn.Port
Conn.Username
Conn.Password
Conn.ConnectionMode
Conn.ServiceNameType
Conn.ServiceName
Conn.TNS
Conn.ConnectionRole
Conn.UseOSAuthentication
Conn.ClientCharsetID
Conn.ClientEncoding
Conn.UseKeepAlive
Conn.KeepAliveInterval
Conn.SessionLimit
Conn.UseSSH
```

可以还原出 Oracle 表单的字段集合。

## 3. Navicat 的 Basic 表单

从用户提供的截图和字符串可以确定的布局：

```text
连接名称:      [...........................]

连接类型:      [Basic ▼]        // 可选 Basic / TNS

主机:          [...........................]
端口:          [1521......................]
服务名称:      [ORCL......................]
               (●) 服务名称   ( ) SID

用户名:        [...........................]
密码:          [...........................]

[x] 保存密码
```

交互特征：

1. **连接类型在最上面**，默认 Basic。
2. Basic 模式下才显示主机、端口、服务名称和「服务名称/SID」单选。
3. 端口默认 `1521`。
4. SID 不是独立的「连接类型」，只是服务名称字段的解释方式。
5. 勾选「保存密码」才持久化密码。
6. 不显示原始连接描述符。
7. TNS 模式下，主机/端口/服务名称整块消失，换成 TNS 别名（Net Service Name）。
8. 高级字段（Role、OS Authentication、Keepalive、Session Limit、字符集/编码、自定义库列表、SSH）不占用默认页面。

## 4. Navicat 的 TNS 模式（由字符串/绑定推断）

```text
连接名称:      [...........................]

连接类型:      [TNS ▼]

TNS 别名:      [ORCL......................]   // Net Service Name
用户名:        [...........................]
密码:          [...........................]

[x] 保存密码
```

TNS 别名由 OCI 解析，`tnsnames.ora` 的位置来自：

- 显式 TNS 目录（如果 UI 提供）；
- `TNS_ADMIN`；
- `ORACLE_HOME\network\admin`；
- `ORACLE_HOME\net80\admin`。

## 5. 为什么 RustGrid 原来显得复杂

1. **模式平铺**：`服务名 / SID / TNS 别名 / 连接串` 是四个平级选项，用户不知道哪个是默认路径。
2. **连接类型位置靠后**：连接类型在用户名/密码之后，用户先填了一堆字段才看到模式。
3. **字段不随模式收敛**：主机/端口在所有模式下都显示且必填；TNS/描述符模式其实不用它们。
4. **原始描述符暴露**：连接串/描述符是普通单行 `TextInput`，粘贴多行描述符时换行被压成空格；`oracledb` 的 parse error 直接透传。
5. **旧字段抢占**：`connect_string` / `tns_alias` 的优先级不检查当前连接类型，切换模式后旧值仍然生效。
6. **TNS 配置目录藏在高级**：用户不知道 TNS 别名需要 `tnsnames.ora`。

## 6. RustGrid 的新设计（提案，已回退）

### 6.1 常规页

```text
连接名称 *

连接类型:      [基本 ▼]        // 基本 / TNS 别名 / 高级（连接描述符）

--- 基本 ---
主机 *         [...........................]
端口 *         [1521......................]
服务名 / SID * [...........................]
               类型: (●) 服务名   ( ) SID

--- TNS ---
TNS 别名 *     [...........................]
配置目录       [...........................]  [浏览]
               留空时使用 TNS_ADMIN 或 ORACLE_HOME\network\admin

用户名 *       [...........................]
密码           [...........................]
[ ] 记住密码
```

- 「连接类型」在表单最上面。
- 基本模式下才显示主机/端口/服务名/SID；TNS 模式下整块换成 TNS 别名 + 配置目录。
- 高级模式的连接描述符在「高级」tab，是多行编辑器；常规页只保留模式选择和凭据。
- 主机/端口在 TNS/高级模式下隐藏，也不再是必填。
- 服务名/SID 用一个字段 + 单选，和 Navicat 一致。
- 必填校验按模式执行：TNS 别名或连接描述符为空时，在对应字段上显示「此项为必填」。
- 旧 profile 的 `service` / `sid` / `tns` / `connect_string` 值在打开表单和连接时自动迁移到新词汇。

### 6.2 高级页

```text
连接描述符（仅高级模式）  [多行文本，保留换行]
钱包目录 (TCPS)          [...........................]  [浏览]
钱包口令                 [...........................]
查询超时 / 初始化 SQL     原有字段
```

- 连接描述符从单行改为 `ui::TextArea`，多行粘贴不再被压平。
- 描述符字段的 hint 明确指向「高级」页。
- 解析错误仍然来自 `oracledb`，但多行输入让 `parse error ... in text <line>` 能对应到真实行；后续可以在表单层加中文校验提示。

### 6.3 兼容性

- 旧 profile 的 `oracle.connect_type = service | sid | tns | connect_string` 会迁移为：
  - `service` → `basic` + `service_name_type = service`
  - `sid` → `basic` + `service_name_type = sid`
  - `tns` → `tns`
  - `connect_string` → `advanced`
- 旧 profile 在未打开表单直接连接时，驱动仍按旧优先级读取 `connect_string` / `tns_alias`，不会因为迁移丢失连接。
- `oracle.config_dir` 在旧 profile 里位于高级页，新表单把它移到 TNS 模式的常规页；旧值仍然有效。
- 旧字段 `oracle.sid` 仍被驱动作为 SID 的备选值。

## 7. 代码落点

| 文件 | 改动 |
| --- | --- |
| `crates/rustgrid-core/src/descriptor.rs` | 新增 `ConnectionFieldPlacement`，`ConnectionFieldSpec` 增加 `placement` / `required`，`ConnectionFieldLabel` 增加 `required`，`ConnectionFieldKind` 增加 `Radio` / `Multiline` |
| `crates/rustgrid-core/src/driver.rs` | 新增 `Driver::normalize_connection_options` 默认钩子 |
| `crates/rustgrid-oracle/src/driver.rs` | 新表单 spec、模式迁移、`connect_string` 按模式分流、SID/服务名单选、测试 |
| `crates/rustgrid-app/src/app/connection_form.rs` | 常规页按 Leading/Target/Trailing 分区渲染、单选、必填提示、多行描述符 |
| `crates/rustgrid-app/src/app/form.rs` | 必填校验按可见性/模式执行，`form_option_errors` |
| `crates/rustgrid-app/src/form.rs` | `option_visible` 默认选项语义、`missing_required` 跳过隐藏字段、Oracle 表单测试 |
| `crates/rustgrid-app/src/app/tree.rs` | 打开表单前调用驱动的选项迁移 |
| `crates/rustgrid-app/locales/{en,zh-CN,ru}.yml` | 新增/更新 Oracle 模式、服务名/SID、TNS 目录提示 |

## 8. 分析限制

- 本机没有配置 Ghidra，Rizin 未安装，Hopper 不支持 Windows，因此没有函数级反编译；`navicat.exe` 无符号表，UI 布局主要来自字符串、绑定键和截图。
- `inspect_pe_resources` / `inspect_pe_imports` 因 `navicat.exe` 超过 64 MiB 上限未能解析完整资源树；`.rsrc` 的具体 UI 资源未展开。
- TNS 模式和高级设置的精确布局是**推断**：模式字段、绑定键、连接描述符碎片和 `tnsnames.ora` 搜索路径都有直接证据，但未在 Navicat 的 TNS 页面上逐项截图验证。
- Navicat 版本 18.0.5，Oracle 客户端 23.26.1；Oracle 19c 的用户端行为可能略有差异。

## 9. 验证

- `cargo check -p rustgrid-app` 通过。
- `cargo test -p rustgrid-oracle`：33 个单元测试通过（含新表单 spec 和旧模式迁移）。
- `cargo test -p rustgrid-app`：247 个测试通过（含新增的 Oracle 模式切换测试）。
- `cargo test -p rustgrid-core -p rustgrid-sqlserver -p rustgrid-sqlite -p rustgrid-redis -p rustgrid-mongodb -p rustgrid-mysql` 通过。
- `cargo test --workspace` 通过（SQLCipher 测试会打印 `sqlcipher_codec_ctx_set_error` 日志，但测试结果为成功）。
- `cargo build` 通过（Windows GNU 工具链；链接器会打印一条既有的 `.rsrc merge failure: multiple non-default manifests` 警告，不影响产物）。

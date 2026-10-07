[English](README.md) | 简体中文

# RustGrid

> 用 Rust 打造的 Navicat 风格数据库管理工具 —— 原生、轻量、零学习成本。

RustGrid 是一款使用 **Rust + GPUI** 构建的跨平台（Windows / macOS / Linux）数据库管理工具。
它的界面布局和交互习惯对齐 Navicat，连接树、对象列表、工具栏和弹窗都与你熟悉的操作一致，
迁移毫无成本。

## 亮点

- **原生，而非 WebView** —— GPUI 元素由 GPU 直接渲染，不依赖浏览器引擎，单个二进制。
- **兼容 Navicat** —— 相同的连接树、对象面板、工具栏与弹窗。
- **可扩展的驱动层** —— 驱动与引擎解耦，新增数据库无需改动界面。
- **Apache-2.0** —— 允许免费商用。

## 支持的数据库

| 引擎                       | 状态           |
| -------------------------- | -------------- |
| MySQL / MariaDB            | 已支持         |
| SQLite                     | 已支持         |
| SQL Server                 | 已支持         |
| PostgreSQL                 | 已支持         |
| Oracle                     | 已支持         |
| DB2、达梦 等               | 通过通用 ODBC  |

## 功能

- **连接管理** —— 新建 / 编辑 / 删除、连接、密码提示；密码在磁盘上加密存储
  （XChaCha20-Poly1305），配置文件中不保存明文。
- **数据库** —— 枚举数据库，新建 / 编辑 / 删除（字符集 + 排序规则）。
- **表与视图** —— 浏览表与视图，分页网格并支持就地编辑，以及删除 / 清空 / 截断 / 重命名。
- **SQL 编辑器** —— 语法高亮、关键字 / 表名补全、美化、解释，并可保存命名查询。
- **导入导出向导** —— Excel、CSV、TXT。
- **用户与例程** —— 账户与权限管理，以及存储例程、视图设计器。
- **备份与还原** —— RustGrid 自有的 `.rgbak` 容器。
- **主题与语言** —— 浅色 / 深色 / 跟随系统，英文 / 简体中文。

## 从源码构建

前置要求：Rust stable（edition 2024，工具链已在 `rust-toolchain.toml` 中固定）。

```bash
git clone https://github.com/yisier/rustgrid.git
cd rustgrid
cargo run -p rustgrid-app
```

生成的二进制文件名为 `RustGrid`。

在 Windows 上需要链接器和 C 编译器（sqlx 的 `ring` 依赖）。安装了 MSVC Build Tools 时
默认工具链即可，否则请使用 GNU 工具链：

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="gcc"
$env:CC="gcc"
cargo run -p rustgrid-app
```

## 项目结构（Cargo workspace）

- `crates/rustgrid-core` —— 与引擎无关的领域层：驱动 / 连接 trait 与模型。
- `crates/rustgrid-mysql` —— MySQL 与 MariaDB 驱动。
- `crates/rustgrid-sqlite` —— SQLite 驱动。
- `crates/rustgrid-sqlserver` —— SQL Server 驱动。
- `crates/rustgrid-postgresql` —— PostgreSQL 驱动。
- `crates/rustgrid-oracle` —— Oracle 驱动（Oracle 官方纯 Rust `oracledb` thin 驱动，无需 Instant Client）。
- `crates/rustgrid-tunnel` —— 共享的 SSH / SOCKS5 / HTTP 隧道，供网络驱动复用。
- `crates/rustgrid-odbc` —— 通用 ODBC 驱动（DB2、达梦 等）。
- `crates/rustgrid-backup` —— `.rgbak` 备份容器。
- `crates/rustgrid-export` —— 表数据导出器（`.xlsx` / `.csv` / `.sql` / `.txt`）。
- `crates/rustgrid-import` —— Excel / CSV / TXT 源文件读取。
- `crates/rustgrid-config` —— 带版本号的设置 / 连接配置，以及加密的密钥存储。
- `crates/rustgrid-app` —— GPUI 桌面应用（二进制 `RustGrid`）。

## 开源协议

采用 **Apache License 2.0** 协议，允许商用。完整协议文本见 [`LICENSE`](LICENSE)。

## 状态

项目处于早期阶段，迭代很快 —— 功能、界面与磁盘存储格式都可能发生变化，恕不另行通知。

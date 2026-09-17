[English](README.md) | 简体中文

# RustGrid

> 兼容 Navicat 的数据库管理工具 —— 零成本迁移，无学习成本。

RustGrid 是一款使用 **Rust + GPUI** 构建的跨平台（Windows / macOS / Linux）数据库管理工具。
它在界面布局和交互习惯上对齐 Navicat，连接树、对象列表、工具栏和弹窗都与你熟悉的操作一致，
可以从 Navicat 平滑迁移，无需重新学习。

## 为什么选择 RustGrid

- **从 Navicat 零成本迁移** —— 熟悉的连接树、对象列表、工具栏与弹窗。
- **相同的使用习惯，无学习成本** —— 交互方式沿用 Navicat 的工作流。
- **友好的开源协议** —— 采用 Apache License 2.0 协议，可免费商用。
- **原生、轻量** —— 单个 Rust 二进制，界面由 GPUI 的 GPU 渲染，无需捆绑运行时。

## 不是 WebView

RustGrid 是使用 GPUI 直接在 GPU 上渲染的原生应用，**不是** WebView / Electron 这类应用。
很多数据库客户端会内嵌浏览器引擎（Chromium、Electron、WebView2 或类似的 Web 运行时），
用 HTML/CSS/JS 来绘制界面；RustGrid 不是这样做的。

|                | WebView / Electron 类工具                     | RustGrid                                    |
| -------------- | --------------------------------------------- | ------------------------------------------ |
| 界面技术       | 内嵌浏览器中的 HTML/CSS/JS                    | 原生 GPUI 元素，GPU 渲染                   |
| 运行时         | 捆绑浏览器引擎，或依赖系统 WebView            | 不依赖浏览器引擎，单个原生二进制           |
| 启动与内存     | 需要先初始化浏览器引擎                        | 更轻量，启动更接近原生                     |
| 外观与交互     | Web 控件                                      | 原生桌面外观（经典窗口 / 弹窗样式）        |
| 系统集成       | 运行在 Web 沙箱中                             | 直接与操作系统窗口集成                     |

因为不需要捆绑或加载浏览器引擎，RustGrid 在 Windows 上不依赖 WebView2，在 Linux 上不依赖
WebKitGTK，也不需要任何其他系统 Web 运行时，并且在各平台上渲染效果一致。最终体验更像原生
桌面软件，而不是“窗口里的网页”。

## 支持的数据库（路线图）

RustGrid 基于与引擎无关的驱动层设计，新增一种数据库只需增加一个驱动，界面保持不变。

| 引擎         | 状态                         |
| ------------ | ---------------------------- |
| MySQL        | 开发中（第一个里程碑）       |
| PostgreSQL   | 计划中                       |
| SQL Server   | 计划中                       |
| Oracle       | 计划中                       |
| SQLite       | 计划中                       |

> RustGrid 正在快速迭代中，**目前 MySQL 支持正在开发中**。当前版本已经可以连接数据库、
> 浏览数据库 / 表 / 视图、管理数据库（新建 / 编辑 / 删除，支持字符集与排序规则），
> 并以分页方式预览表数据。

## 功能（当前）

- 连接管理：新建 / 编辑 / 删除，连接 / 断开，密码提示。
- 密码在磁盘上加密存储（XChaCha20-Poly1305），配置文件中不保存明文。
- 枚举数据库；新建、编辑、删除数据库（字符集 + 排序规则）。
- 按数据库列出表和视图。
- 分页、只读的数据表格预览。
- 浅色 / 深色 / 跟随系统主题，以及英文 / 简体中文界面。

## 从源码构建

前置要求：Rust stable（edition 2024，工具链已在 `rust-toolchain.toml` 中固定）。

```bash
git clone https://github.com/yisier/rustgrid.git
cd rustgrid
cargo run -p rustgrid-app
```

生成的二进制文件名为 `rustgrid`。

在 Windows 上需要链接器和 C 编译器（sqlx 的 `ring` 依赖）。安装了 MSVC Build Tools 时，
默认的 MSVC 工具链即可。否则请使用 GNU 工具链：

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="gcc"
$env:CC="gcc"
cargo run -p rustgrid-app
```

## 项目结构（Cargo workspace）

- `crates/rustgrid-core` —— 与引擎无关的领域层：驱动 / 连接 trait 与模型。
- `crates/rustgrid-mysql` —— MySQL 驱动实现。
- `crates/rustgrid-config` —— 带版本号的设置 / 连接配置，以及加密的密钥存储。
- `crates/rustgrid-app` —— GPUI 桌面应用（二进制 `rustgrid`）。

## 开源协议

采用 **Apache License 2.0** 协议，允许商用。完整协议文本见 [`LICENSE`](LICENSE)。

## 状态

项目处于早期阶段，迭代很快 —— 功能、界面与磁盘存储格式都可能发生变化，恕不另行通知。

# 快速开始

从 [Releases](https://github.com/yisier/rustgrid/releases/latest) 下载对应平台的压缩包，解压后直接运行 `RustGrid`。没有安装程序，也不依赖运行时；Windows 版只有一个 `RustGrid.exe`。

![连接树](./assets/zh/connections.png)

## 新建连接

1. 点击 **新建连接**，选择数据库类型。
2. 填写主机、端口、用户名和密码。SQLite 填写文件路径；ODBC 填写驱动或连接串。
3. 点击 **测试连接**，然后 **保存并连接**。

只有勾选 **记住密码** 时才会保存密码，而且密码存入加密凭据文件，不写进连接配置。没有保存密码时，使用连接会弹出输入框。

## 第一步

- 在树中打开数据库，选择表并双击，即可分页浏览数据。
- 点击 **新建查询**，对选中的连接和数据库执行 SQL。
- 做大改动前，先在 **备份** 标签页生成一个 `.rgbak`。
- 用 **同步** 比较两个数据库，并在写入前查看差异。

## 支持的数据库

| 类型 | 数据库 |
| --- | --- |
| SQL | MySQL、MariaDB、PostgreSQL、SQL Server、Oracle、SQLite、ClickHouse |
| NoSQL | Redis、MongoDB |
| 通用 | ODBC（DB2、达梦及其他 ODBC 驱动） |

详细能力和限制见 [支持的数据库](./engines.html)。

## 平台

Windows 10+、macOS 13+（Intel 与 Apple Silicon）、Linux x64 / arm64。

- **macOS**：安装包未签名，首次打开请右键选择 **打开**。应用可以自更新；被隔离的副本会先要求移出隔离区。
- **Linux**：压缩包内就是可执行文件，除桌面环境常规的 GTK 依赖外无需额外系统库。
- **更新**：**设置 → 关于** 会检查最新版本并显示该版本的更新日志，并按当前平台下载对应文件。

卸载时删除可执行文件或 `.app` 即可。设置、连接和备份保存在系统配置目录中，可以单独清理。

English | [简体中文](README_zh.md)

# RustGrid

> A Navicat-style database GUI built in Rust — native, lightweight, zero learning curve.

RustGrid is a cross-platform (Windows / macOS / Linux) database management tool built with
**Rust + GPUI**. Its layout and interactions follow Navicat, so switching costs nothing:
the connection tree, object lists, toolbars and dialogs all behave the way you already expect.

## Highlights

- **Native, not a WebView** — GPU-rendered GPUI elements, no browser engine, a single binary.
- **Navicat-compatible** — the same connection tree, object panes, toolbars and dialogs.
- **Extensible drivers** — an engine-agnostic driver layer; new engines never touch the UI.
- **Apache-2.0** — free for commercial use.

## Supported databases

| Engine                     | Status           |
| -------------------------- | ---------------- |
| MySQL / MariaDB            | Supported        |
| SQLite                     | Supported        |
| SQL Server                 | Supported        |
| PostgreSQL                 | Supported        |
| Oracle                     | Supported        |
| DB2, Dameng, ...           | via generic ODBC |

## Features

- **Connections** — create / edit / delete, connect, password prompt; passwords are encrypted
  on disk (XChaCha20-Poly1305) and never stored in a profile.
- **Databases** — enumerate, create / edit / delete with charset and collation.
- **Tables & views** — browse tables and views, a paginated grid with in-place editing, and
  drop / empty / truncate / rename operations.
- **SQL editor** — syntax highlighting, keyword/table completion, beautify, explain, and saved
  named queries.
- **Import & export wizards** — Excel, CSV and TXT.
- **Users & routines** — account and privilege management, plus stored-routine and view designers.
- **Backup & restore** — RustGrid's own `.rgbak` container.
- **Themes & languages** — light / dark / system, English / 简体中文.

## Build from source

Prerequisite: Rust stable (edition 2024; the toolchain is pinned in `rust-toolchain.toml`).

```bash
git clone https://github.com/yisier/rustgrid.git
cd rustgrid
cargo run -p rustgrid-app
```

The produced binary is named `RustGrid`.

On Windows a linker and C compiler are required (for sqlx's `ring`). With MSVC Build Tools
installed the default toolchain works; otherwise use the GNU toolchain:

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="gcc"
$env:CC="gcc"
cargo run -p rustgrid-app
```

## Project layout (Cargo workspace)

- `crates/rustgrid-core` — engine-agnostic domain: driver/connection traits and models.
- `crates/rustgrid-mysql` — MySQL and MariaDB drivers.
- `crates/rustgrid-sqlite` — SQLite driver.
- `crates/rustgrid-sqlserver` — SQL Server driver.
- `crates/rustgrid-postgresql` — PostgreSQL driver.
- `crates/rustgrid-oracle` — Oracle driver (Oracle's pure-Rust `oracledb` thin driver, no Instant Client).
- `crates/rustgrid-tunnel` — shared SSH / SOCKS5 / HTTP tunnel used by the network drivers.
- `crates/rustgrid-odbc` — generic ODBC driver (DB2, Dameng, ...).
- `crates/rustgrid-backup` — the `.rgbak` backup container.
- `crates/rustgrid-export` — table-data exporters (`.xlsx` / `.csv` / `.sql` / `.txt`).
- `crates/rustgrid-import` — Excel / CSV / TXT source readers.
- `crates/rustgrid-config` — versioned settings/profiles and encrypted secret storage.
- `crates/rustgrid-app` — the GPUI desktop application (binary `RustGrid`).

## License

Licensed under the **Apache License, Version 2.0**. Commercial use is permitted. See the
[`LICENSE`](LICENSE) file for the full text.

## Status

Early and evolving quickly — features, UI and on-disk formats may change without notice.

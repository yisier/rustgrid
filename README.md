English | [简体中文](README_zh.md)

# RustGrid

> A Navicat-compatible database GUI — migrate with zero cost and zero learning curve.

RustGrid is a cross-platform (Windows / macOS / Linux) database management tool built with
**Rust + GPUI**. It follows Navicat's layout and interaction conventions, so you can switch
from Navicat without re-learning anything: the connection tree, object panes, toolbars and
dialogs all behave the way you already expect.

## Why RustGrid

- **Zero-cost migration from Navicat** — familiar connection tree, object lists, toolbars and dialogs.
- **Same habits, no learning curve** — interactions mirror Navicat's workflow.
- **Friendly open-source license** — licensed under the Apache License 2.0, free for commercial use.
- **Native and lightweight** — a single Rust binary with a GPU-rendered GPUI interface, no bundled runtime.

## Not a WebView

RustGrid is a native application rendered directly on the GPU with GPUI — it is **not** a
WebView/Electron-style app. Many database GUI tools embed a browser engine (Chromium,
Electron, WebView2, or a similar web runtime) and draw their interface with HTML/CSS/JS.
RustGrid does not.

|                          | WebView / Electron-based tools                     | RustGrid                                              |
| ------------------------ | -------------------------------------------------- | ---------------------------------------------------- |
| UI stack                 | HTML/CSS/JS inside an embedded browser             | Native GPUI elements rendered on the GPU             |
| Runtime                  | Bundles a browser engine or relies on a system WebView | No browser engine; a single native binary        |
| Startup and memory       | The browser engine has to initialize first         | Lighter, native startup                              |
| Look and feel            | Web widgets                                        | Native desktop look (classic window / dialog style)  |
| System integration       | Runs inside a web sandbox                          | Direct integration with native OS windows            |

Because there is no browser engine to ship or load, RustGrid does not require WebView2 on
Windows, WebKitGTK on Linux, or any other system web runtime, and it renders consistently
across platforms. The result feels like a native desktop application rather than a web page
inside a window.

## Supported databases (roadmap)

RustGrid is designed around an engine-agnostic driver layer, so adding a new database only
means adding a driver — the UI stays the same.

| Engine      | Status                                    |
| ----------- | ----------------------------------------- |
| MySQL       | In development (first milestone)          |
| PostgreSQL  | Planned                                   |
| SQL Server  | Planned                                   |
| Oracle      | Planned                                   |
| SQLite      | Planned                                   |

> RustGrid is iterating rapidly. **MySQL support is currently under active development.** The
> current build can already connect, browse databases/tables/views, manage databases
> (create/edit/delete with charset and collation) and preview table data with pagination.

## Features (current)

- Connection management: create / edit / delete, connect / disconnect, password prompt.
- Passwords are stored encrypted on disk (XChaCha20-Poly1305); profiles never contain plaintext.
- Enumerate databases; create, edit and delete a database (charset + collation).
- Per-database listing of tables and views.
- Paginated, read-only data grid for table preview.
- Light / dark / system themes and English / 简体中文 UI.

## Build from source

Prerequisite: Rust stable (edition 2024; the toolchain is pinned in `rust-toolchain.toml`).

```bash
git clone https://github.com/yisier/rustgrid.git
cd rustgrid
cargo run -p rustgrid-app
```

The produced binary is named `RustGrid`.

On Windows a linker and C compiler are required (for sqlx's `ring`). The default MSVC
toolchain works when MSVC Build Tools are installed. Otherwise use the GNU toolchain:

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="gcc"
$env:CC="gcc"
cargo run -p rustgrid-app
```

## Project layout (Cargo workspace)

- `crates/rustgrid-core` — engine-agnostic domain: driver/connection traits and models.
- `crates/rustgrid-mysql` — MySQL driver implementation.
- `crates/rustgrid-config` — versioned settings/profiles and encrypted secret storage.
- `crates/rustgrid-app` — the GPUI desktop application (binary `RustGrid`).

## License

Licensed under the **Apache License, Version 2.0**. Commercial use is permitted. See the
[`LICENSE`](LICENSE) file for the full text.

## Status

Early and evolving quickly — features, UI and on-disk formats may change without notice.

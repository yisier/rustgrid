# AGENTS.md

A Navicat-like database management tool built with **Rust + GPUI**, targeting
**cross-platform** (Windows, macOS, Linux). **Extensibility is a first-class requirement**,
not a later refactor.

## Repository layout (Cargo workspace)

- `crates/navidog-core` — engine-agnostic domain: `Driver`/`Connection` traits, models
  (`CellValue`, `ConnectionProfile`, `TablePage`, ...), `Error`, `DriverRegistry`.
  **No sqlx / GPUI / OS dependencies here.**
- `crates/navidog-mysql` — the only compiled-in driver; implements the core traits with sqlx.
- `crates/navidog-config` — versioned profile storage in the OS config dir (`connections.json`).
- `crates/navidog-app` — GPUI binary `navidog` (`src/main.rs`), UI in `src/app.rs`, locale
  files in `locales/`.
- The root `Cargo.toml` owns all versions under `[workspace.dependencies]`; member crates use
  `<dep>.workspace = true`. Add new dependencies there, not inline in a member.

## Tech stack (decided)

- Rust **stable**, **edition 2024** (`rust-toolchain.toml` pins `stable`; workspace
  `rust-version = "1.94"`).
- UI: **`gpui = "0.2"`** from crates.io (published from `zed-industries/zed`). Use the
  published crate — do **not** switch to a git dependency on the Zed monorepo.
- MySQL: **sqlx 0.9**, `default-features = false`, only features
  `runtime-tokio`, `mysql`, `tls-rustls-ring`, `chrono`.
- i18n: **rust-i18n 4**. Config dir: **directories 6**.

## Commands

- `cargo check --workspace` / `cargo build`
- `cargo run -p navidog-app` (produced binary is `navidog`)
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all`
- No `DATABASE_URL` or sqlx offline cache: queries use the **runtime** API (`sqlx::query`,
  not `sqlx::query!`). Do not add the `macros` feature.
- `Cargo.lock` is committed (this is an application, not a library). No CI yet.

## Extensibility requirements (design for these from the start)

Keep engine-specific code out of the UI/app layers. Adding a second engine must not require
touching UI code.

- **Engine abstraction.** UI depends only on `navidog_core::{Driver, Connection}` and core
  models — never on `sqlx::MySql*` types or raw SQL strings.
- **Driver loading.** `DriverRegistry` plus the `DriverSource` trait are the loader boundary.
  Future runtime driver installation adds a `DriverSource`; keep each driver in its own crate
  so it can be built/distributed independently. Do not assume a single bundled driver.
- **Internationalization.** Route every user-facing string through `t!`. Keys live in
  `crates/navidog-app/locales/{en,zh-CN}.yml` and must be added to **all** locale files.
  `t!` returns `Cow<'_, str>`; call `.to_string()` before handing it to a gpui element.
- **Config storage.** Use `navidog_config::ConfigStore`; bump `CURRENT_VERSION` and extend
  `migrate()` when the on-disk schema changes.

## Dependency policy

- **Do not reinvent the wheel**: use mature, stable third-party crates for non-trivial
  functionality (connection helpers, UI widgets, config storage) instead of hand-rolling.
- Verify a crate's maturity before adopting it — prefer widely used, actively maintained crates.
- Keep the dependency tree small (see Gotchas).

## Scope — phase 1 (do not exceed)

Strictly follow Navicat's UI layout. Only these features:

1. Database connection management (create connection, connect/disconnect)
2. List all databases of the connected MySQL server
3. Per database, list all tables and views
4. Show a selected table's data in a grid/table view with **pagination** — display only, **no editing** yet

Do not implement other database engines yet or edit-in-grid functionality. The abstractions
above are what make them cheap later — do not build the features early.

## Gotchas

- `navidog-mysql::decode_cell` uses sqlx's **checked** `try_get` (which enforces
  `Type::compatible`) for every known type, then falls back to `try_get_unchecked::<Vec<u8>>`
  + UTF-8 for text-encoded types (DECIMAL, JSON) and raw bytes otherwise. Do not reorder to
  put `String` first or use `try_get_unchecked` for numeric/binary types: checked decoding is
  what prevents raw bytes being misread as the wrong type.
- sqlx 0.9's `sqlx::query` only accepts `&'static str` (the `SqlSafeStr` bound). A
  dynamically built query must be wrapped: `sqlx::query(sqlx::AssertSqlSafe(sql))`. Keep
  identifiers escaped (`quote_identifier`) and use bind parameters for all values.
- **Windows needs a linker and a C compiler** (sqlx's `ring`). Install MSVC Build Tools for
  the default `x86_64-pc-windows-msvc` toolchain, or use the GNU toolchain with MinGW-w64 on
  `PATH` (`RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`,
  `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=gcc`, `CC=gcc`).
- Keep gpui usage close to the verified shape in `crates/navidog-app/src/app.rs`
  (`Application::new().run`, `cx.open_window`, `impl Render`).
- **Cross-platform** (Windows/macOS/Linux). Avoid OS-only APIs; gate platform-specific code
  behind `#[cfg(target_os = ...)]`; watch per-OS native deps (e.g. Linux system libraries).
- **Small install size is a hard requirement.** The release profile already sets `lto`,
  `strip`, `opt-level = "z"`, `codegen-units = 1`. Prefer small crates; enable only the sqlx
  features actually used.
- UI must mirror Navicat's layout: connection tree on the left, content/details pane on the right.

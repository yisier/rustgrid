# AGENTS.md

Greenfield repository (no commits yet). A Navicat-like database management tool built with **Rust + GPUI**, targeting **cross-platform** (Windows, macOS, Linux).

## Tech stack (decided)

- Rust: **stable** toolchain, **edition 2024** (`edition = "2024"` in `Cargo.toml`)
- UI: **GPUI community fork (gpui.rs)** — not Zed's official `zed-industries/gpui`. Prefer the fork's docs/examples over Zed's monorepo.
- MySQL access: **sqlx** (async, pool-based)

## Dependency policy

- **Do not reinvent the wheel**: use mature, stable third-party crates for non-trivial functionality (e.g., connection dialogs, UI widgets, config storage) instead of hand-rolling it.
- Verify a crate's maturity before adopting it — prefer widely used, actively maintained crates.

## Scope — phase 1 (do not exceed)

Strictly follow Navicat's UI layout. Only these features:

1. Database connection management (create connection, connect/disconnect)
2. List all databases of the connected MySQL server
3. Per database, list all tables and views
4. Show a selected table's data in a grid/table view with **pagination** — display only, **no editing** yet

Do not implement other database engines (MySQL only for now) or edit-in-grid functionality.

## Development notes

- **Cross-platform** (Windows/macOS/Linux). Test with `cargo run` on the current OS, but keep all three platforms in mind: avoid OS-only APIs, gate platform-specific code behind `#[cfg(target_os = ...)]`, and be aware of per-OS native deps (e.g., Linux system libraries).
- **Small install size is a hard requirement.** The binary itself (GPUI renders via its own backend, not a bundled browser/WebView runtime) is the only payload — keep it that way:
  - Use the `release` profile (opt-level, LTO, strip) to shrink the final binary.
  - Prefer crates that don't drag in large unrelated dependency trees; for `sqlx`, enable only the features actually used (e.g., `runtime-tokio`, `mysql`, `macros` — not the full default set).
  - Prefer smaller installers (e.g., plain archive / inno-setup style) over bundling a full runtime.
- UI must mirror Navicat's layout: connection tree on the left, content/details pane on the right.

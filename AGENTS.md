# AGENTS.md

A Navicat-like database management tool built with **Rust + GPUI**, targeting
**cross-platform** (Windows, macOS, Linux). **Extensibility is a first-class requirement**,
not a later refactor.

## Repository layout (Cargo workspace)

- `crates/navidog-core` — engine-agnostic domain: `Driver`/`Connection` traits, models
  (`CellValue`, `ConnectionProfile`, `TablePage`, ...), `Error`, `DriverRegistry`.
  **No sqlx / GPUI / OS dependencies here.**
- `crates/navidog-mysql` — the only compiled-in driver; implements the core traits with sqlx.
- `crates/navidog-config` — versioned settings/profiles plus encrypted secret storage in the
  OS config dir (`connections.json`, `settings.json`, `secrets.json`).
- `crates/navidog-app` — GPUI binary `navidog`: `src/app/` is the view/render layer, split
  by feature (`mod.rs` holds `AppView`, its state, the `Render` entry, free helpers and
  tests; the rest are `impl AppView` submodules: `tree`, `database`, `db_dialog`, `objects`,
  `sidebar`, `tabs`, `toolbar`, `query`, `query_view`, `query_editor`, `grid`, `grid_input`,
  `grid_commit`, `grid_view`, `grid_cell`, `grid_scroll`, `grid_toolbar`, `dialogs`,
  `widgets`, `form`). New view code goes in the matching submodule, **not** `mod.rs`.
  `ui/` is the internal design system (buttons, dialogs, scrollbars, text fields, ...): put
  shared chrome there, never one-off `div`s in feature code. Several subtrees are child
  `Entity` views wired through `WeakEntity<AppView>` + `notify_*` invalidation: the Tables/Views
  object list (`ObjectPane`), the tab strip (`TabBar`), the connection tree (`TreePane`), and
  each open grid (`GridView`, one entity per grid, owning its `GridState` and all grid
  interaction state). Follow that pattern when a subtree gets large. App-level overlays that
  must cover the whole window (delete confirm, error dialog) stay on `AppView` and are requested
  by child views through the weak handle.
  Other files: `src/session.rs` (UI state), `src/form.rs` (connection form),
  `src/theme.rs` (light/dark palettes), `src/assets.rs` (embedded asset loader for
  `assets/`), `src/runtime.rs` (tokio bridge), locales in `locales/`.
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
- Stored secrets: **chacha20poly1305 0.11** + **base64 0.22** (XChaCha20-Poly1305).

## Commands

- **After every code change, ALWAYS build automatically before finishing** (do not wait to be
  asked): run `cargo fmt --all` then `cargo build` (use the GNU toolchain env vars in Gotchas on
  Windows). A faster `cargo check -p navidog-app` (or `cargo check --workspace`) may be used while
  iterating; finish with `cargo build`. Debug builds do not need the shader toolchain below.
- `cargo check --workspace` / `cargo build`
- `cargo run -p navidog-app` (produced binary is `navidog`)
- Release build (`cargo build --release -p navidog-app`) additionally needs the fxc shim (see
  Gotchas): build it once with
  `gcc -O2 -o fxc.exe tools/fxc-shim/fxc.c -lkernel32`, then run cargo with
  `GPUI_FXC_PATH=<abs path to fxc.exe>` (plus the GNU toolchain env vars below).
- `cargo test --workspace`
- Live MySQL integration test (ignored by default): set `NAVIDOG_MYSQL_PASSWORD` (and
  optionally `NAVIDOG_MYSQL_HOST`/`PORT`/`USER`/`DATABASE`), then
  `cargo test -p navidog-mysql -- --ignored`. It exercises connect, catalog listing, paging,
  and the auth-failure mapping against a real server.
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
- **Config storage.** Use `navidog_config::ConfigStore`. It owns three files under the OS
  config dir: `connections.json` (`CURRENT_VERSION`), `settings.json` (`SETTINGS_VERSION`),
  and encrypted `secrets.json` + `secret.key`. Bump the matching version and extend `migrate()`
  when a schema changes. **Passwords never live in a profile** — they are keyed by profile
  `id` and stored encrypted (see Gotchas).

## UI conventions

- **Windows classic desktop look.** The frame is drawn entirely by the app in the style of a
  classic Win32/Navicat window: a custom 32px titlebar (`render_titlebar` / `titlebar_button`),
  a menu bar, a command toolbar, then the connection tree + content pane. The native titlebar
  is suppressed (`TitlebarOptions { appears_transparent: true }` in `main.rs`); dragging uses
  `WindowControlArea::Drag`, and min/max/close call `window.minimize_window()`,
  `window.zoom_window()`, `window.remove_window()`. Keep metrics square and compact — do not
  introduce rounded/modern widgets.
- **Colors come from `Theme`** (`src/theme.rs`), resolved from `ThemeSetting` +
  `window.appearance()` on every `render`. Add new colors to **both** `Theme::light()` and
  `Theme::dark()` and reference them as `rgb(theme.field)`; don't hardcode palette values in
  `app.rs` (the one-off dialog-close hover reds are legacy exceptions).
- **Icons are embedded assets.** Register every new SVG in `Assets::load` (`src/assets.rs`) and
  load it with `svg().path("icons/foo.svg")`; bitmaps use
  `img(ImageSource::Resource(Resource::Embedded("logo.png".into())))`. An unregistered path
  fails to load silently.
- **Buttons are unified.** Every button must go through `AppView::win_button` (or
  `AppView::dialog_button`) with a `ButtonKind` (`Normal`, `Default`, `Selected`, `Disabled`)
  to keep the Windows/Navicat look: square corners, `button_bg` fill, a 1px border
  (`button_border`, or `button_default_border` for the default/selected button), `theme.text`,
  and a hover of `button_hover_bg` + accent border. Do **not** hand-roll one-off button `div`s,
  rounded corners, or solid `primary` fills. Confirm buttons use `ButtonKind::Default`; toggles
  such as theme/language use `ButtonKind::Selected`.
- **Dialogs are app-drawn overlays, not native windows.** A modal is an `absolute inset-0`
  overlay (`bg(rgba(theme.overlay))`) wrapping a centered frame: a 30px titlebar with an icon +
  title + `dialog_close_button`, a `dialog_face` body, and a right-aligned button row. Tabs are
  plain square buttons (active: `dialog_face` bg + top/left/right border only, so it merges with
  the page; inactive: `button_bg` + full border). Inputs/combos stay `input_bg`.
- **Toolbar/tab items are flat, not push buttons.** The main toolbar
  (`AppView::render_main_tab`) and the object toolbar (`AppView::toolbar_item`) are borderless
  icon+label items with a hover highlight and thin `toolbar_separator`s between them — do
  **not** style them with `win_button` or borders. `win_button` is for dialog push buttons only.
- **The object list (Tables/Views) follows Navicat's layout.** It is column-major: items are
  chunked into columns of `object_rows_per_column()` (derived from the scroll viewport height,
  `OBJECT_ROW_HEIGHT` per row) so a column fills top-to-bottom and then wraps to the next column
  to the right, and it scrolls **horizontally** (`overflow_x_scroll`), not vertically. The
  number of rows per column therefore adapts to the window height. Do not switch it back to
  `flex_row()`/`overflow_scroll()`. gpui 0.2 does **not** paint scrollbars for `overflow_*`
  (it only scrolls and reserves space), so the list ships its own horizontal scrollbar
  (`render_object_hscrollbar`, driven by `object_scroll`); keep it in sync when touching the
  object list.
- Keep dialog controls compact: 12px text, ~24px-high buttons, square text fields.
- Embed bitmap images with `img(ImageSource::Resource(Resource::Embedded("name".into())))`.
  Calling `img("name")` treats the bare filename as a **URI** and fails with
  `Failed to load asset ... loading image asset from "name"`.

## Dependency policy

- **Do not reinvent the wheel**: use mature, stable third-party crates for non-trivial
  functionality (connection helpers, UI widgets, config storage) instead of hand-rolling.
- Verify a crate's maturity before adopting it — prefer widely used, actively maintained crates.
- SQL language work uses `sqlparser` (lexer/parser/keyword list) and `sqlformat` (beautifier);
  do not add a bespoke SQL lexer/formatter. Syntax highlighting maps `sqlparser` tokens to colors
  (`sql.rs`), completion combines `sqlparser` keywords with loaded table names, and `Beautify SQL`
  calls `sqlformat`.
- Keep the dependency tree small (see Gotchas).
- **UI widget reference — `longbridge/gpui-kit`** (https://github.com/longbridge/gpui-kit,
  formerly `gpui-component`, **Apache-2.0**). When a widget is non-trivial (date picker, text
  input, virtualized table, dropdown, toast, tooltip, ...) **port the implementation from this
  repo** instead of inventing it: restyle to the classic Win32/Navicat look and keep it under
  `src/app/ui/`. Keep the upstream file's Apache-2.0 attribution/license notice on anything
  copied verbatim.
  Do **not** add it as a dependency: the current `gpui-kit` / `gpui-component 0.6` is built on
  `gpui-pre` (a republished snapshot of Zed's GPUI), **not** the crates.io `gpui = "0.2"` this
  workspace pins — adopting it would force a whole-app framework migration and break the
  "use the published gpui 0.2 crate" rule. It also drags in heavy deps (`tree-sitter`, `reqwest`,
  `resvg`, `ropey`, `markdown`, ...) and its modern shadcn look fights the required square
  styling. (The older `gpui-component 0.5.x` line targets crates.io `gpui 0.2.2`, but is already
  superseded and still heavy — use it only as a source to port from.)

## Scope (do not exceed)

Strictly follow Navicat's UI layout. Implemented today:

1. Connection management: create/edit/delete, connect/disconnect, password prompt
2. Enumerate databases; create/edit/delete a database (charset + collation) and edit defaults
3. Per database, list tables and views
4. Show a selected table's data in a grid with **pagination** — display only, **no editing** yet
5. SQL query editor tabs (`New Query` button / `Queries` main tab): multi-line editor with SQL
   syntax highlighting and keyword/table completion, `Beautify SQL`, run arbitrary SQL against a
   chosen connection + database, `Explain`, and a result grid that reuses the table grid
   (controls, scrollbars, status, and in-place editing when a single table can be inferred).

Still out of scope: a second database engine, and the disabled placeholder UI (the
`Functions`/`Users`/`Backups` main tabs, the `Design/New/Delete Table`, `Import/Export` toolbar
buttons, and the query editor's `Save`/`Query Builder`/`Snippets` items are deliberate stubs —
leave them disabled unless asked). The abstractions above are what make more engines cheap later —
do not build those features early.

## Gotchas

- The grid filter builder is a **tree**, not a flat list: `GridState::filters` / `filter_draft` are
  `Vec<FilterNode>` where a node is either a `FilterCondition` or a `FilterGroup` (nested children,
  parenthesized). Each node carries its own `FilterConjunction` (how it joins its previous sibling),
  so a group may mix `AND`/`OR`; the root is an implicit list. UI rows are addressed by a node
  **path** (`Vec<usize>`, indices from the root) — keep that in sync when adding node operations.
  `navidog-mysql::filter_clause` and `session.rs::filter_display_clause` recurse over the tree,
  skip disabled/incomplete nodes, drop groups left empty, and must keep bind order identical to the
  rendered `?` placeholders.

- `navidog-mysql::map_connect_error` flags authentication failures as
  `navidog_core::Error::Authentication` by checking `MySqlDatabaseError::number()`
  (1044/1045/1698) — **not** `DatabaseError::code()`, which returns the SQLSTATE (e.g.
  `28000`). The app keys its password prompt off this variant.
- `navidog-mysql::decode_cell` uses sqlx's **checked** `try_get` (which enforces
  `Type::compatible`) for every known type, then falls back to `try_get_unchecked::<Vec<u8>>`
  + UTF-8 for text-encoded types (DECIMAL, JSON) and raw bytes otherwise. Do not reorder to
  put `String` first or use `try_get_unchecked` for numeric/binary types: checked decoding is
  what prevents raw bytes being misread as the wrong type.
- sqlx 0.9's `sqlx::query` only accepts `&'static str` (the `SqlSafeStr` bound). A
  dynamically built query must be wrapped: `sqlx::query(sqlx::AssertSqlSafe(sql))`. Keep
  identifiers escaped (`quote_identifier`) and use bind parameters for all values.
- MySQL DDL (`CREATE`/`DROP`/`ALTER DATABASE`) **must** run through the text protocol:
  `sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(&pool)`. Using `sqlx::query` prepares the
  statement and MySQL rejects it with `1295 ... not supported in the prepared statement
  protocol yet`. Reserve `sqlx::query` for parameterized DML/`SELECT`.
- The query editor runs arbitrary SQL through `Connection::execute_query`
  (`navidog-mysql`), which uses the text protocol on a **pinned pooled connection** (so `USE db`
  and the statement share a session). Since MySQL cannot tell the caller whether a statement
  returns rows ahead of time, `returns_result_set` classifies it by the leading keyword: that path
  uses `raw_sql(..).fetch_all` (falling back to `prepare` for column metadata on a 0-row result),
  everything else uses `raw_sql(..).execute` for `rows_affected`. Keep the keyword list in sync
  when adding statements that produce result sets.
- Query results are stored as ordinary `GridState` entries in `AppView::grids`, flagged by
  `GridState::sql` being `Some` and `show_toolbar == false`; `QueryTab::grid_id` links back to
  one. This is what lets the editor reuse `render_grid` (scrollbars, controls, status, editing).
  Paging is client-side (`page_size = rows.len()`), and `load_page` re-runs the stored SQL via
  `reload_query_grid`. Editing is enabled only when `sqlparser` infers a single-table `FROM`
  (`sql::infer_single_table`).
- DB work is tokio-based but gpui's executor is not tokio. Inside `cx.spawn`, run sqlx
  futures through `Runtime::spawn` and `.await` the returned `JoinHandle` (see
  `app.rs`). Awaiting sqlx directly in a gpui task panics with "there is no reactor running".
- Stored passwords are encrypted with XChaCha20-Poly1305 (`navidog-config/src/secrets.rs`);
  `secrets.json` holds ciphertext keyed by profile `id` and `secret.key` holds the key. The
  profile JSON must stay password-free (a test asserts the plaintext never hits disk).
- **Windows needs a linker and a C compiler** (sqlx's `ring`). On this machine the MSVC
  `link.exe` is **not** installed, so always build with the GNU toolchain by prepending
  `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`,
  `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=gcc`, `CC=gcc` (MinGW-w64 `gcc` is on `PATH`).
  A plain `cargo build` fails with `linker link.exe not found`. With the GNU toolchain the
  host *is* the GNU target, so the binary lands at `target/debug/navidog.exe` (not under a
  triple-named subdirectory).
- Keep gpui usage close to the verified shape in `crates/navidog-app/src/app.rs` /
  `main.rs` (`Application::new().with_assets(Assets).run`, `cx.open_window`, `impl Render`).
- **Window-edge resizing on Windows is app-owned.** gpui hides the OS titlebar, so its
  `WM_NCCALCSIZE` leaves only a 1px frame and the titlebar's `WindowControlArea::Drag` claims
  the top edge, making the window effectively unresizable. `src/win_resize.rs` installs a
  `SetWindowSubclass` hook that answers `WM_NCHITTEST` with the real frame-metric resize codes
  before gpui sees the message (`install(window)` is called from `open_window`). Do **not** edit
  the crates.io copy of gpui under `~/.cargo/registry` to fix this — it is not reproducible.
- gpui's build script only compiles HLSL when `debug_assertions` is **off** (release). It needs
  the Windows SDK `fxc.exe`, which is **not** installed on this machine, so a plain release
  build panics in `gpui/build.rs` with `Failed to find fxc.exe`. Debug builds compile shaders at
  runtime via `D3DCompileFromFile` and are unaffected. Use `tools/fxc-shim` (speaks the fxc
  subset gpui invokes; drives the system `d3dcompiler_47.dll`) via `GPUI_FXC_PATH` for release.
- **Cross-platform** (Windows/macOS/Linux). Avoid OS-only APIs; gate platform-specific code
  behind `#[cfg(target_os = ...)]`; watch per-OS native deps (e.g. Linux system libraries).
- **Small install size is a hard requirement.** The release profile already sets `lto`,
  `strip`, `opt-level = "z"`, `codegen-units = 1`. Prefer small crates; enable only the sqlx
  features actually used.
- UI must mirror Navicat's layout: connection tree on the left, content/details pane on the right.

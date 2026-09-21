# AGENTS.md

A Navicat-like database management tool built with **Rust + gpui-kit**, targeting
**cross-platform** (Windows, macOS, Linux). **Extensibility is a first-class requirement**,
not a later refactor.

## Repository layout (Cargo workspace)

- `crates/rustgrid-core` — engine-agnostic domain: `Driver`/`Connection` traits, models
  (`CellValue`, `ConnectionProfile`, `TablePage`, ...), `Error`, `DriverRegistry`.
  **No sqlx / GPUI / OS dependencies here.**
- `crates/rustgrid-backup` — RustGrid's own `.rgbak` backup container (zstd chunks + JSON
  manifest), used by the Backup main tab.
- `crates/rustgrid-export` — table-data export writers: `.xlsx` (rust_xlsxwriter), `.csv`, `.sql`
  and `.txt`. Engine-agnostic formatting fed by decoded `CellValue` rows.
- `crates/rustgrid-import` — source-file readers for the Import Wizard: Excel workbooks
  (`.xlsx`/`.xls`/`.xlsm`/`.xlsb`/`.ods`) via `calamine`, and `.csv`/`.txt` with an RFC 4180
  parser (`encoding_rs` GB18030 fallback; TXT auto-detects its field delimiter among
  tab/comma/semicolon/pipe), plus the source-column type inference used when the wizard creates a
  table.
- `crates/rustgrid-mysql` — the only compiled-in driver; implements the core traits with sqlx.
- `crates/rustgrid-config` — versioned settings/profiles plus encrypted secret storage in the
  OS config dir (`connections.json`, `settings.json`, `secrets.json`).
- `crates/rustgrid-app` — GPUI binary `RustGrid`: `src/app/` is the view/render layer, split
  by feature (`mod.rs` holds `AppView`, its state, the `Render` entry, free helpers and
  tests; the rest are `impl AppView` submodules: `tree`, `database`, `db_dialog`, `objects`,
  `sidebar`, `tabs`, `toolbar`, `query`, `query_view`, `query_editor`, `grid`, `grid_input`,
  `grid_commit`, `grid_view`, `grid_cell`, `grid_scroll`, `grid_toolbar`, `dialogs`,
  `export`, `import`, `widgets`, `form`). New view code goes in the matching submodule, **not** `mod.rs`.
  `ui/` is the internal design system (buttons, scrollbars, text fields, dropdowns, ...) and the
  wrapper layer over gpui-kit: put shared chrome there, never one-off `div`s in feature code. Several subtrees are child
  `Entity` views wired through `WeakEntity<AppView>` + `notify_*` invalidation: the Tables/Views
  object list (`ObjectPane`), the tab strip (`TabBar`), the connection tree (`TreePane`), and
  each open grid (`GridView`, one entity per grid, owning its `GridState` and all grid
  interaction state). Follow that pattern when a subtree gets large. Dialogs are **not** drawn by
  `AppView`: it holds the dialog state (`form`, `db_dialog`, `password_prompt`, `error_dialog`,
  `delete_confirm`, `options_open`) and `sync_dialog` hands it to `Root` via
  `window.open_dialog` — see "UI conventions".
  Other files: `src/session.rs` (UI state), `src/form.rs` (connection form),
  `src/theme.rs` (light/dark palettes), `src/assets.rs` (embedded asset loader for
  `assets/`), `src/runtime.rs` (tokio bridge), locales in `locales/`.
- The root `Cargo.toml` owns all versions under `[workspace.dependencies]`; member crates use
  `<dep>.workspace = true`. Add new dependencies there, not inline in a member.

## Tech stack (decided)

- Rust **stable**, **edition 2024** (`rust-toolchain.toml` pins `stable`; workspace
  `rust-version = "1.94"`).
- UI: **`gpui-kit = "0.6"`** (crates.io). It is the facade over `gpui-pre` (a published
  snapshot of Zed's GPUI) plus `gpui-base` and the shadcn-styled `gpui-component`; it also
  re-exports GPUI itself. The workspace additionally depends on the same engine under the
  name the app imports: `gpui = { package = "gpui-pre", version = "0.3" }`, so the ~40 view
  files keep `use gpui::…`. **These two must move in lockstep** — see "Upgrading gpui-kit".
  Do **not** switch either to a git dependency on the Zed monorepo.
- MySQL: **sqlx 0.9**, `default-features = false`, only features
  `runtime-tokio`, `mysql`, `tls-rustls-ring`, `chrono`.
- i18n: **rust-i18n 4**. Config dir: **directories 6**.
- Stored secrets: **chacha20poly1305 0.11** + **base64 0.22** (XChaCha20-Poly1305).
- Export: **rust_xlsxwriter 0.99** writes `.xlsx`; **rfd 0.17** (XDG-portal backend on Linux) drives
  the native folder/file dialogs. CSV/SQL/TXT need no dependencies.
- Import: **calamine 0.36** (`dates`) reads `.xlsx`/`.xls`/`.xlsm`/`.xlsb`/`.ods`; `.csv` is
  comma-separated and `.txt` auto-detects tab/comma/semicolon/pipe, both via a built-in RFC 4180
  parser with `encoding_rs` 0.8 for a GB18030 fallback. The same **rfd 0.17** drives the
  source-file dialog. Imported date/time values are normalised with **dtparse 2** (a Python
  `dateutil` port, day-first) to MySQL's canonical form before insert.

## Commands

- **After every code change, ALWAYS build automatically before finishing** (do not wait to be
  asked): run `cargo fmt --all` then `cargo build` (use the GNU toolchain env vars in Gotchas on
  Windows). A faster `cargo check -p rustgrid-app` (or `cargo check --workspace`) may be used while
  iterating; finish with `cargo build`. Debug builds do not need the shader toolchain below.
- `cargo check --workspace` / `cargo build`
- `cargo run -p rustgrid-app` (produced binary is `RustGrid`)
- Release build (`cargo build --release -p rustgrid-app`) normally needs no shader toolchain
  (gpui-kit enables `runtime_shaders`). If that ever changes, see Gotchas: build the fallback
  shim once with `gcc -O2 -o fxc.exe tools/fxc-shim/fxc.c -lkernel32`, then run cargo with
  `GPUI_FXC_PATH=<abs path to fxc.exe>` (plus the GNU toolchain env vars below).
- `cargo test --workspace`
- Live MySQL integration test (ignored by default): set `RUSTGRID_MYSQL_PASSWORD` (and
  optionally `RUSTGRID_MYSQL_HOST`/`PORT`/`USER`/`DATABASE`), then
  `cargo test -p rustgrid-mysql -- --ignored`. It exercises connect, catalog listing, paging,
  and the auth-failure mapping against a real server.
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all`
- No `DATABASE_URL` or sqlx offline cache: queries use the **runtime** API (`sqlx::query`,
  not `sqlx::query!`). Do not add the `macros` feature.
- `Cargo.lock` is committed (this is an application, not a library). No CI yet.

## Extensibility requirements (design for these from the start)

Keep engine-specific code out of the UI/app layers. Adding a second engine must not require
touching UI code.

- **Engine abstraction.** UI depends only on `rustgrid_core::{Driver, Connection}` and core
  models — never on `sqlx::MySql*` types or raw SQL strings.
- **Driver loading.** `DriverRegistry` plus the `DriverSource` trait are the loader boundary.
  Future runtime driver installation adds a `DriverSource`; keep each driver in its own crate
  so it can be built/distributed independently. Do not assume a single bundled driver.
- **Internationalization.** Route every user-facing string through `t!`. Keys live in
  `crates/rustgrid-app/locales/{en,zh-CN}.yml` and must be added to **all** locale files.
  `t!` returns `Cow<'_, str>`; call `.to_string()` before handing it to a gpui element.
- **Config storage.** Use `rustgrid_config::ConfigStore`. It owns three files under the OS
  config dir: `connections.json` (`CURRENT_VERSION`), `settings.json` (`SETTINGS_VERSION`),
  and encrypted `secrets.json` + `secret.key`. Bump the matching version and extend `migrate()`
  when a schema changes. **Passwords never live in a profile** — they are keyed by profile
  `id` and stored encrypted (see Gotchas).

## UI conventions

- **Look: shadcn/ui on Navicat's layout.** The structural layout is Navicat's — a custom 32px
  titlebar (`render_titlebar` / `titlebar_button`), menu bar, command toolbar, then the
  connection tree + content pane — but the chrome is the shadcn look delivered by gpui-kit:
  rounded corners, muted borders, solid `primary` fills, ghost/outline button variants. Do not
  re-introduce bespoke square Win32 chrome. The native titlebar is suppressed
  (`TitlebarOptions { appears_transparent: true }` in `main.rs`); dragging uses
  `WindowControlArea::Drag`, and min/max/close call `window.minimize_window()`,
  `window.zoom_window()`, `window.remove_window()`.
- **Use gpui-kit components first.** Reach for `gpui_kit::component::*` before hand-drawing a
  control: `button::Button` (via `AppView::win_button` / `dialog_button` / `toolbar_item`),
  `checkbox`, `Dialog`, `Input`, `Combobox`, `Scrollbar`, `Tab`, `Table`, `Theme`. Anything
  shared by more than one view belongs in `src/app/ui/`, never inline in feature code.
- **The `ui/` wrappers own the app-facing API.** `ui::TextInput` wraps gpui-kit's
  `InputState`/`Input` and `ui::ComboBox` wraps `ComboboxState`/`Combobox`, exposing the app's
  original signatures (`TextInputOptions`, `on_change`/`on_submit`/`on_cancel`/`on_tab`,
  `set_text`, `set_options`, `set_selected`, ...). Because those gpui-kit states require a
  `&mut Window` to construct, the inner state is created on the **first render** and pending
  mutations/callbacks are flushed there. Do not use `InputState`/`ComboboxState` directly in
  feature code — extend the wrapper instead.
- **Dialogs are `Root`-managed, not app-drawn.** `AppView` holds the dialog state (`form`,
  `db_dialog`, `password_prompt`, `error_dialog`, `delete_confirm`, `options_open`);
  `dialogs.rs::sync_dialog` maps it to a `DialogKind` and opens/closes
  `window.open_dialog(...)` accordingly. Dialog bodies are rebuilt every render through
  `Entity<AppView>::update`, so status (e.g. "testing connection") stays live. Dialogs are
  centered and **not** draggable. There is no `overlay`/`dialog_frame`/`confirm_dialog`
  machinery any more — do not add one back.
- **Colors come from `Theme`** (`src/theme.rs`), resolved from `ThemeSetting` +
  `window.appearance()` on every `render`. The palettes follow shadcn's "zinc" tokens (keep
  `Theme::light()`/`Theme::dark()` in sync) and the same `render` calls
  `gpui_kit::component::Theme::change(...)` when the resolved mode flips, so gpui-kit
  components match. Reference colors as `rgb(theme.field)`; don't hardcode palette values.
- **Icons are embedded assets, with a gpui-kit fallback.** Register every new SVG in
  `Assets::load` (`src/assets.rs`) and load it with `svg().path("icons/foo.svg")` (or
  `Icon::default().path(...)` for gpui-kit components); `Assets::load` delegates unknown paths
  to `gpui_kit::assets::Assets`, which serves the bundled Lucide set. Bitmaps use
  `img(ImageSource::Resource(Resource::Embedded("logo.png".into())))`. An unregistered path
  that is not Lucide fails to load silently.
- **Toolbar/tab items are flat, not push buttons.** The main toolbar
  (`AppView::render_main_tab`) and the object toolbar (`AppView::toolbar_item`) are borderless
  icon+label items (ghost buttons) with thin `toolbar_separator`s between them — do **not**
  style them with borders or filled buttons. `win_button`/`dialog_button` are for push buttons
  inside dialogs. `ui::toolbar_item` supplies the content as explicit children (16px icon, 12px
  label) because gpui-kit's default button typography (16px) is too large for the app's chrome.
- **The object list (Tables/Views) follows Navicat's layout.** It is column-major: items are
  chunked into columns of `object_rows_per_column()` (derived from the scroll viewport height,
  `OBJECT_ROW_HEIGHT` per row) so a column fills top-to-bottom and then wraps to the next column
  to the right, and it scrolls **horizontally** (`overflow_x_scroll`), not vertically. The
  number of rows per column therefore adapts to the window height. Do not switch it back to
  `flex_row()`/`overflow_scroll()`. gpui-pre does **not** paint scrollbars for `overflow_*`
  (it only scrolls and reserves space), so the list ships its own horizontal scrollbar
  (`render_object_hscrollbar`, driven by `object_scroll`); keep it in sync when touching the
  object list.
- **Floating popups must be built with `ui::popup_panel`.** gpui dispatches `ScrollWheelEvent`
  to *every* scrollable hitbox under the cursor — each `overflow_*` container handles it and none
  stops propagation — so a popup whose list overlays a scrollable pane scrolls **both** it and the
  pane behind it. The only fix is hitbox behavior: a `BlockMouse` hitbox (`.occlude()`) makes all
  hitboxes behind it report `should_handle_scroll() == false`. `ui::popup_panel(theme)` is the
  single shared builder for app-drawn floating surfaces (dropdown lists, menus, pickers): it sets
  `absolute` + `.occlude()` + the dialog face/border/shadow, so scrolling inside a popup never
  scrolls the view underneath. Never hand-roll a `div().absolute()` popup; build it on
  `popup_panel`. gpui-kit's own `Combobox`/`Select`/`Popover` already occlude, so they are fine.
- **Dropdowns share one style: search on top, a check mark on the selected row.** gpui-kit's
  `Combobox` (via `ui::ComboBox`) already renders a searchable list and a right-aligned check that
  is invisible when unselected (so rows never shift). `ui::ComboBox` is pinned to the kit's
  `Size::XSmall` (20px, 12px text) so every dropdown in the project is the same compact size; do
  not override it per call site. The table designer's type dropdown
  (`design_view.rs::render_type_combo`) is hand-built and must mirror this: a 20px `ui::TextInput`
  search (`TextInputOptions { size: Some(Size::XSmall), .. }`) above 20px rows, and an
  `icons/check.svg` on the right of the selected row, `opacity(0)` on the others.
- Keep dialog controls compact: 12px text, ~24px-high buttons, compact text fields. gpui-kit's
  sizing defaults are larger than the app's metrics, so `ui::TextInput` renders the kit input at
  `Size::Small`, disables the kit's focus ring (the "shadow" that otherwise floats over the field)
  and pins the text to `DEFAULT_TEXT_SIZE` (12.5px); in-place cell editors (grid/table designer)
  additionally pass `TextInputOptions { bare: true, text_size: Some(..) }`, which drops the kit's
  background, border and inner padding so the field blends into its cell. The grid renders its
  editor **inside** the editing cell (`editing_cell` in `grid_view.rs`) instead of as an absolute
  overlay, and skips that cell's own text — so a transparent `bare` editor never ghosts, and
  scrolling the list moves the editor with its row instead of leaving it floating at a stale
  offset. A selected grid cell paints `Theme::grid_selection_bg` (blue) with
  `grid_selection_text`.
- Grid selection is **Excel-like and multi-range**: `CellSelection` holds `Vec<CellRange>` plus an
  `active` index, and a cell is painted selected when any range contains it. Plain click starts a
  new single-cell range (and opens the editor on release), **Shift+click/drag** moves the active
  range's `cursor`, and **Ctrl/Cmd+click** toggles the clicked cell's range in/out
  (`add_or_remove_range`). Anything that reads a "selection" — status bar, delete, `set_selection_null`,
  multi-cell edit, header highlight — must go through `contains`/`cells`/`row_indices`/`col_indices`
  rather than assuming a single rectangle.
- Embed bitmap images with `img(ImageSource::Resource(Resource::Embedded("name".into())))`.
  Calling `img("name")` treats the bare filename as a **URI** and fails with
  `Failed to load asset ... loading image asset from "name"`.

## Upgrading gpui-kit

`gpui-kit` and the workspace's `gpui` (= `gpui-pre`) alias are the same engine and **must be
bumped together** in the root `Cargo.toml`; a mismatch gives two GPUI copies and incompatible
element types. Two kinds of upgrade:

- **`0.6.x` → `0.6.y` (no code):** `cargo update -p gpui-kit -p gpui-pre`. The `"0.6"`/`"0.3"`
  ranges in the root `Cargo.toml` keep this in-family.
- **A breaking bump (e.g. `gpui-kit 0.7`, a new `gpui-pre` snapshot):** edit both lines in the
  root `Cargo.toml`, then fix the fallout. It is localized by design:
  - `ui/text_input.rs`, `ui/combo.rs` (the wrappers) and the dialog builders
    (`dialogs.rs`, `db_dialog.rs`, `options.rs`) absorb gpui-kit component API changes; the
    40+ call sites behind them do not move.
  - The rest is plain GPUI API churn (the `gpui 0.2` → `gpui-pre 0.3` jump was ~41 mechanical
    errors across ~17 files: `focus(&h, cx)`, `ScrollHandle::max_offset()` returning `Point`,
    `ShapedLine::paint` taking `TextAlign` + `Option<Pixels>`, `BoxShadow { inset }`,
    `track_scroll(&h)`, `Entity::update` returning `R`). Budget one pass like that per
    breaking engine bump.
  - `Cargo.lock` is committed, so pin exactly what built.

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
  formerly `gpui-component`, **Apache-2.0**). It **is** a dependency of this workspace (the
  `gpui-kit` facade, see Tech stack): use its components rather than hand-drawing controls, and
  when a widget is still missing (toast, tooltip, virtualized table, ...) prefer
  wiring the kit's component over porting an implementation. Only fall back to porting upstream
  code when the kit genuinely lacks the widget; keep ports under `src/app/ui/` and retain the
  upstream Apache-2.0 attribution. Upgrade procedure and blast radius: "Upgrading gpui-kit".
- **The grid's in-place date/time picker reuses the kit's `Calendar`.** `DatePicker` holds an
  `Entity<CalendarState>` (`gpui_kit::component::calendar`) as the source of truth for the date
  and subscribes to `CalendarEvent::Selected`; `grid_cell.rs::render_date_picker` renders it and
  only app-draws the time-of-day spinner row and the today/OK/Cancel footer. Do not hand-roll the
  calendar again. The kit's styled `Calendar` facade is sized for a full popup (28px cells), so
  `ui/calendar.rs::compact_calendar` wraps the **unstyled** `gpui_base::Calendar` and overrides
  the cell metrics through its `Calendar::item` hook (the styled facade exposes no item hook);
  the localized labels come from the app's own `calendar.week.*` / `calendar.month.*` entries in
  `locales/{en,zh-CN}.yml`, because the kit's `Calendar.*` translations live in the kit's
  `rust-i18n` backend and are not reachable from the app's `t!`. The kit's `DatePicker`/`Date`
  model is date-only (`NaiveDate`), so it cannot represent `datetime`/`timestamp` values by
  itself — that is why time stays app-drawn. Keep that popup from stealing focus: gpui focuses
  any `track_focus` element on mouse-down (the calendar root and the kit buttons are focusable),
  which would blur the cell editor and auto-commit, so the popup calls
  `capture_any_mouse_down(|_, window, _| window.prevent_default())`.

## Scope (do not exceed)

Keep Navicat's layout (connection tree on the left, content/details pane on the right) with the
shadcn chrome described above. Implemented today:

1. Connection management: create/edit/delete, connect/disconnect, password prompt
2. Enumerate databases; create/edit/delete a database (charset + collation) and edit defaults
3. Per database, list tables and views
4. Show a selected table's data in a grid with **pagination** — display only, **no editing** yet
5. SQL query editor tabs (`New Query` button / `Queries` main tab): multi-line editor with SQL
   syntax highlighting and keyword/table completion, `Beautify SQL`, run arbitrary SQL against a
   chosen connection + database, `Explain`, and a result grid that reuses the table grid
   (controls, scrollbars, status, and in-place editing when a single table can be inferred).
6. Right-clicking a table (in the object list or the connection tree) offers Open/Design plus Drop
   Table, Empty Table (`DELETE FROM`), Truncate Table and Rename; Drop/Empty/Truncate go through
   the shared confirm dialog, while Rename (or `F2` on the selection) edits the name **in place**
   in the row that started it. These run through `Connection`
   (`drop_table`/`empty_table`/`truncate_table`/`rename_table`), never as raw SQL written in the UI.
7. Saving named queries: `Save` (or `Ctrl+S`) in the query editor opens a dialog for the query
   name and save location (connection + database). Saved queries are listed under the `Queries`
   main tab in a **sortable three-column list** (query name / connection / database; click a header
   to sort). The list is scoped by the connection tree's selection — a selected database shows only
   that database's queries, a selected connection shows all of its databases, and any other
   selection shows every saved query. Rows open with a double-click and are deleted through the
   shared confirm dialog. They persist in the config dir's `queries.json` (`SavedQuery`, keyed by
   connection profile id + database name).
8. Table export wizard: with a table selected, **Export Wizard** (object toolbar / table context
   menu) opens a separate OS window walking four pages — P1 format (`.xlsx`/`.csv`/`.sql`/`.txt`),
   P2 tables + output paths (native folder/file dialogs via `rfd`, defaulting to the desktop),
   P3 exported columns per selected table (source-table dropdown when several are chosen, all
   fields by default), and P4 options + run (include column titles, continue on error) with a
   live log/progress view. Writes go through `rustgrid-export`; the wizard streams table pages via
   `Connection::fetch_page` (ordered by the primary key when the table has one).
9. Import wizard: **Import Wizard** (object toolbar / table context menu, when a database is
   open) opens a separate OS window walking four pages — P1 import type (`Excel File`, `CSV File`,
   `Text File`), P2 the merged source page (a native file dialog picks the workbook or delimited
   file; each worksheet — or the single file-named table for CSV/TXT — becomes a row with an
   editable destination table and a **Create Table** check box), P3 field mapping (a destination
   dropdown per source column, auto-matched by normalised header name), and P4 run/log. A source
   table whose name matches an existing table targets it; otherwise the name becomes the table name
   and **Create Table** is ticked automatically (`database_table_names` + `find_existing`).
   Field matching is case/separator-insensitive (`normalize_key`). A created table's columns and
   types are inferred from the data by `rustgrid-import::infer_columns` (integer → `bigint`,
   decimal → `decimal(20,6)`, dates → `date`/`datetime`, booleans → `tinyint(1)`, text → `varchar`/
   `longtext`), and the `id` column becomes the primary key (forced `NOT NULL`). Rows are inserted
   in `IMPORT_BATCH_ROWS` batches through `Connection::insert_rows`; unmatched source columns are
   simply not imported. Date/time values are parsed by `dtparse` (day-first, so `26/6/2025` is
   26 June) and rewritten to MySQL's canonical `YYYY-MM-DD[ HH:MM:SS]`, so text sources with local
   date conventions import into `date`/`datetime` columns.
10. User management: the `Users` main tab lists the server's accounts (toolbar: 编辑 / 新建 /
   删除 / 权限管理器, plus a search field). **New User** and **Edit User** are the *same* separate
    OS window (`user_create.rs`, `UserCreateWindow` observing `AppView`), with two views over one
    shared `UserEditorState`: 快速视图 is Navicat's guided form (普通用户/管理用户 presets +
    per-database privilege matrix) and 完整视图 is the full account designer (attributes, server
    privileges, individual object grants, roles). Switching views is lossless and Save is one
    path, so the two flows cannot drift apart. Saving a new account switches the window to 完整视图
    in place instead of closing and reopening a tab. Passwords are prompted on connect, never
    stored in a profile.

Still out of scope: a second database engine, and the disabled placeholder UI (the `Functions`
main tab, the `Design/New/Delete Table` toolbar buttons, and the query editor's
`Query Builder`/`Snippets` items are deliberate stubs — leave them disabled unless asked). The
abstractions above are what make more engines cheap later — do not build those features early.

## Gotchas

- The grid filter builder is a **tree**, not a flat list: `GridState::filters` / `filter_draft` are
  `Vec<FilterNode>` where a node is either a `FilterCondition` or a `FilterGroup` (nested children,
  parenthesized). Each node carries its own `FilterConjunction` (how it joins its previous sibling),
  so a group may mix `AND`/`OR`; the root is an implicit list. UI rows are addressed by a node
  **path** (`Vec<usize>`, indices from the root) — keep that in sync when adding node operations.
  `rustgrid-mysql::filter_clause` and `session.rs::filter_display_clause` recurse over the tree,
  skip disabled/incomplete nodes, drop groups left empty, and must keep bind order identical to the
  rendered `?` placeholders. Row layout: the first condition of a group is flush left (no
  conjunction gutter); later conditions show the `并且/或者` toggle in that gutter. A group's
  boundary row reuses the group's indent, centres its controls under the operator (`=`) column, and
  reveals the `+`/`−` group actions only on hover (`group_hover`).

- `rustgrid-mysql::map_connect_error` flags authentication failures as
  `rustgrid_core::Error::Authentication` by checking `MySqlDatabaseError::number()`
  (1044/1045/1698) — **not** `DatabaseError::code()`, which returns the SQLSTATE (e.g.
  `28000`). The app keys its password prompt off this variant.
- `rustgrid-mysql::decode_cell` uses sqlx's **checked** `try_get` (which enforces
  `Type::compatible`) for every known type, then falls back to `try_get_unchecked::<Vec<u8>>`
  + UTF-8 for text-encoded types (DECIMAL, JSON) and raw bytes otherwise. Do not reorder to
  put `String` first or use `try_get_unchecked` for numeric/binary types: checked decoding is
  what prevents raw bytes being misread as the wrong type.
- sqlx 0.9's `sqlx::query` only accepts `&'static str` (the `SqlSafeStr` bound). A
  dynamically built query must be wrapped: `sqlx::query(sqlx::AssertSqlSafe(sql))`. Keep
  identifiers escaped (`quote_identifier`) and use bind parameters for all values.
- Grid writes identify a row by its primary key, falling back to **every column** when the table
  has no primary key (`GridState` column `primary_key` flags). Key values are
  `Option<String>` (`CellValue::as_edit_value`): `None` is SQL `NULL` and must render `IS NULL`,
  never be bound as an empty string — binding `''` for a numeric `NULL` column fails with
  `1292 Truncated incorrect INTEGER value: ''`. The same applies to `delete_rows` keys.
  `Connection::execute_query` only returns result-set metadata, so editable query grids re-attach
  the catalog's `primary_key`/`data_type` flags both on first run (`run_query`) **and on every
  reload** (`GridView::reload_query`); missing the reload path silently degrades edits to the
  every-column key and reintroduces the `1292` above.
- MySQL DDL (`CREATE`/`DROP`/`ALTER DATABASE`) **must** run through the text protocol:
  `sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(&pool)`. Using `sqlx::query` prepares the
  statement and MySQL rejects it with `1295 ... not supported in the prepared statement
  protocol yet`. Reserve `sqlx::query` for parameterized DML/`SELECT`.
- The query editor runs arbitrary SQL through `Connection::execute_query`
  (`rustgrid-mysql`), which uses the text protocol on a **pinned pooled connection** (so `USE db`
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
- The export wizard is a separate OS window (like Backup), not a `Root` dialog, because the
  `rfd` folder/file pickers must not run on gpui's event thread: the **synchronous** `rfd` API
  shows a nested Windows message loop inside event handling and crashes the app. The pickers use
  `rfd::AsyncFileDialog` awaited from a gpui task, which runs the dialog on rfd's own thread.
  The wizard reads table rows through
  `Connection::fetch_page` in `EXPORT_PAGE_SIZE` pages (ordered by the primary key when there is
  one, for stable offsets) and feeds them to `rustgrid_export::TableWriter`, which streams CSV/SQL/
  TXT and buffers `.xlsx`. Field selection is applied by projecting each page's columns down to the
  chosen ones, so the writer's header and the row values stay aligned. A table with no primary key
  exports in the engine's natural order.
- The import wizard is likewise a real window (`ImportWindow` observing `AppView`). It reads the
  workbook on tokio's blocking pool through `Runtime::spawn_blocking` — `calamine` is synchronous
  and must not run on gpui's event thread. It opens the workbook once per read (`sheet_names`, then
  `read_sheet` per sheet), and the whole sheet (header + rows) is held in `ImportSheetFields` so a
  freshly created table can infer its column types. Keep new source formats inside
  `rustgrid-import`; the app only sees `SheetData`/`infer_columns`.
- DB work is tokio-based but gpui's executor is not tokio. Inside `cx.spawn`, run sqlx
  futures through `Runtime::spawn` and `.await` the returned `JoinHandle` (see
  `app.rs`). Awaiting sqlx directly in a gpui task panics with "there is no reactor running".
- Stored passwords are encrypted with XChaCha20-Poly1305 (`rustgrid-config/src/secrets.rs`);
  `secrets.json` holds ciphertext keyed by profile `id` and `secret.key` holds the key. The
  profile JSON must stay password-free (a test asserts the plaintext never hits disk).
- **Windows needs a linker and a C compiler** (sqlx's `ring`). On this machine the MSVC
  `link.exe` is **not** installed, so always build with the GNU toolchain by prepending
  `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`,
  `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=gcc`, `CC=gcc` (MinGW-w64 `gcc` is on `PATH`).
  A plain `cargo build` fails with `linker link.exe not found`. With the GNU toolchain the
  host *is* the GNU target, so the binary lands at `target/debug/RustGrid.exe` (not under a
  triple-named subdirectory).
- Keep GPUI usage close to the shapes verified in `main.rs` and the `ui/` wrappers.
  `main.rs` opens the window as `gpui_kit::application().with_assets(Assets).run(|cx| { gpui_kit::init(cx); … })`
  and wraps the app view in `Root::new(view, window, cx)` — `Root` must be the window root or
  `window.open_dialog` panics.
- **gpui-pre API deltas vs the old `gpui 0.2`** (recurring sources of compile errors):
  `Window::focus(&handle, cx)` now takes `cx`; `ScrollHandle::max_offset()` returns
  `Point<Pixels>` (`.x`/`.y`, not `.width`/`.height`); `ShapedLine::paint` takes
  `TextAlign` + `Option<Pixels>`; `BoxShadow` gained an `inset` field; `uniform_list`'s
  `track_scroll` takes `&handle`; `Entity::update` returns `R` directly while
  `WeakEntity::update` returns `Result<R>`.
- **gpui-kit stateful widgets need a `Window` at construction** (`InputState::new`,
  `ComboboxState::new`, `TableState::new` all take `&mut Window`). The app builds its entities
  while there is no window yet, so `ui/text_input.rs` and `ui/combo.rs` create the inner state
  on the first `render` and queue setters/callbacks to that frame. Keep new gpui-kit-backed
  state behind the same lazy wrapper pattern.
- **Window-edge resizing on Windows is app-owned.** gpui hides the OS titlebar, so its
  `WM_NCCALCSIZE` leaves only a 1px frame and the titlebar's `WindowControlArea::Drag` claims
  the top edge, making the window effectively unresizable. `src/win_resize.rs` installs a
  `SetWindowSubclass` hook that answers `WM_NCHITTEST` with the real frame-metric resize codes
  before gpui sees the message (`install(window)` is called from `open_window`). Do **not** edit
  the crates.io copy of gpui under `~/.cargo/registry` to fix this — it is not reproducible.
- gpui's build script only compiles HLSL when `debug_assertions` is **off** (release). With
  gpui-kit this is normally avoided: `gpui-kit` enables `runtime_shaders` on
  `gpui-pre-platform`, so shaders compile at runtime and the Windows SDK `fxc.exe` is not
  needed. If a future dependency change drops that feature, a plain release build panics in
  `gpui/build.rs` with `Failed to find fxc.exe`; the fallback is `tools/fxc-shim` (speaks the
  fxc subset gpui invokes; drives the system `d3dcompiler_47.dll`) via
  `GPUI_FXC_PATH=<abs path to fxc.exe>`. Debug builds are unaffected either way.
- **Cross-platform** (Windows/macOS/Linux). Avoid OS-only APIs; gate platform-specific code
  behind `#[cfg(target_os = ...)]`; watch per-OS native deps (e.g. Linux system libraries).
- **Small install size is a hard requirement.** The release profile already sets `lto`,
  `strip`, `opt-level = "z"`, `codegen-units = 1`. Prefer small crates; enable only the sqlx
  features actually used.
- UI must mirror Navicat's layout: connection tree on the left, content/details pane on the right.

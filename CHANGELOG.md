# Changelog

English | [简体中文](CHANGELOG_zh.md) | [Русский](CHANGELOG_ru.md)

All notable changes to RustGrid are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

UI labels are quoted as the app shows them, so this file quotes the English strings;
[`CHANGELOG_zh.md`](CHANGELOG_zh.md) and [`CHANGELOG_ru.md`](CHANGELOG_ru.md) quote the 简体中文 and
Russian ones.

## [0.1.5] - 2026-11-02

### Added

- The Redis key list has a Group column (the key's prefix through its first colon) and defaults to
  sorting by it.
- The query editor highlights and completes Redis commands and keys on a Redis connection.
- Right-clicking a Redis namespace in the tree offers New Key and Refresh.
- The JSON view on a key page now has syntax colours.

### Changed

- A Redis database's menu offers New Key instead of Run SQL File and Dump SQL File, and Redis query
  tabs hide Format SQL.
- New Key starts with the selected namespace's prefix.
- Paging, sorting and filtering keep a grid's horizontal scroll position, and sorting keeps its
  column widths.
- The connection tree's scrollbar appears only while scrolling and no longer overlaps the divider.
- Editors without line numbers no longer reserve a blank fold gutter.

### Fixed

- Clicking a Redis namespace in the tree no longer closes the app.
- Saving a string key reads the editor's current text and confirms with a Saved toast.
- Only the current Redis namespace stays highlighted, so a previously filtered one is cleared.

## [0.1.4] - 2026-11-01

### Added

- Redis connections (Standalone, Sentinel and Cluster): keys browsed by namespace, each with its
  type, TTL, memory and value preview and its own page (RAW/JSON for strings, editable tables for
  collections), plus New Key, Set TTL, Delete and Rename.
- Sync picks the source and target schema on SQL Server and PostgreSQL.

### Changed

- General turns TLS/SSL and tunnels on with switches, and their pages appear only then.
- The grid's field-layout button is named Fields (it was Filter).
- The mouse wheel pans the icon-grid object lists horizontally.
- The pager fields accept typing, paste and IME, and Up/Down steps the record limit by 100.
- Import continues past a failed row, and Backups in the tree expands to its files.
- The changelog in About, the sync log and error dialogs can be selected and copied.

### Fixed

- Right-clicking a saved query or a backup in the tree no longer closes the app.
- Closing a database closes its tabs, and the object list follows the tree's selection.
- A cell edit saved on leaving the field confirms it.
- The filter panel and the connection tree keep in step with the active view.
- Ctrl+clicking a cell out of Select All keeps the rest of the selection.
- Long backup names show in full in the icon grid.

## [0.1.3] - 2026-10-28

### Added

- MySQL connections gain an optional StarRocks compatibility mode (Advanced → Compatibility mode):
  browsing, paging and editing work, and unsupported features (Users, Functions, table designer,
  Backup/Restore) are hidden while it is on.
- Export Results in the query toolbar exports one or several result sets; several results become
  worksheets in one `.xlsx` workbook.
- Exporting a table grid offers all records or the current page's filtered, sorted rows.

### Changed

- With Restore on startup off, connections, databases and groups start collapsed.
- The backup dialog enables Save only after the name or the selected objects change.

### Fixed

- The filter panel and the applied WHERE clause stay in sync, so the column shown is the one used.
- MySQL TIMESTAMP columns show the server's time instead of raw bytes.

## [0.1.2] - 2026-10-26

### Added

- The table designer selects fields like the data grid: drag or Shift extends a range, Ctrl toggles
  cells, Ctrl+A selects all, a click on the row header selects whole rows, and Ctrl+C copies the
  selection.

### Changed

- The data grid's right-click menu no longer offers "Insert Row".
- Delete, Move Up and Move Down in the table designer act on every selected field row.

### Fixed

- Reordering a table's fields in the designer is saved instead of reported as no changes.
- Clicking a main tab clears that list's search, so the list shows every object again; "Keep the
  object search" now only covers database and connection switches.
- Backup: Delete, Restore and Extract act on the selected row in the current database, so a
  same-named file in another database is never targeted.
- Backup: the elapsed time keeps counting during a long run.
- Backup: a finished run selects the file it just wrote, and pasting a file keeps the list
  selection.
- Pasting copied records keeps their `NULL` values, so numeric columns save instead of failing with
  an incorrect-value error.
- Saving a table's edits confirms success, and Save / Ctrl+S reports when there is nothing to save.
- Refreshing a table confirms that it finished.
- Deleting records from the data grid's right-click menu no longer closes the app.

## [0.1.1] - 2026-10-22

### Added

- The Russian interface, with its own changelog
  ([#22](https://github.com/yisier/rustgrid/pull/22)).
- Options → General: "Single-click mode" opens connections, databases and tables with one click
  ([#14](https://github.com/yisier/rustgrid/issues/14)).
- Options → About reads a new version's notes in the interface language.

### Changed

- Every window uses the same title bar with the platform's own window buttons
  ([#21](https://github.com/yisier/rustgrid/issues/21)).

### Fixed

- MySQL 5.7 and other servers without `GENERATION_EXPRESSION` no longer break the table designer and
  structure sync with error 1054 ([#20](https://github.com/yisier/rustgrid/issues/20)).
- Clicking the Tables or Views category no longer makes the object list jump and re-lay-out
  ([#23](https://github.com/yisier/rustgrid/issues/23)).
- Long Russian labels no longer clip the sync controls.
- The Chinese connection tree shows its category names in Chinese again instead of English.
- Closing a window with unsaved work always asks first, including Alt+F4.

## [0.1.0] - 2026-10-21

### Added

- The Sync main tab (after Backup): compare a source database with a target — across engines too —
  and apply structure, data, both or a full copy, with per-table status, the row-level diff, a SQL
  preview, a run log and saved configurations.
- The table designer adds PostgreSQL's exclusion constraints and rewrite rules, triggers, and
  MySQL/MariaDB partitioning, and edits PostgreSQL identity columns (generation, start, increment).
- The designer suggests the common defaults that suit the selected column's type, and its field grid
  moves like the table grid (arrows, Tab, Enter/F2).
- Empty and Truncate act on every selected table at once.

### Fixed

- PostgreSQL: query results show their result sets again
  ([#12](https://github.com/yisier/rustgrid/issues/12)) and the table designer completes
  ([#15](https://github.com/yisier/rustgrid/issues/15)).
- PostgreSQL: ORDER BY keeps the column's native type instead of sorting numbers and dates as text.
- Search and filter placeholders follow a UI language change
  ([#19](https://github.com/yisier/rustgrid/issues/19)).
- Loading a grid page no longer pushes the rows down and back up.
- A menu opened from the pointer appears under it, the connection tree's group rename field accepts
  typing again, and the object list repaints as soon as the theme changes.
- Long labels wrap in the Options window and the sync options popup.

## [0.0.15] - 2026-10-19

### Added

- The detail lists sort: click a column header to order by it, click again to reverse — on Tables,
  Views, Functions, Users, Backup and the saved queries.
- Options → Editor: "Ctrl + mouse wheel resizes the font", on by default — hold Ctrl and scroll over
  a query editor to zoom it in or out.
- Options → General: "Restore on startup", on by default — the query tabs, connections, open
  databases and table tabs of the last run come back.
- The query result tabs page with ← / → when the result sets do not all fit.

### Changed

- Every detail list is more compact (28px rows), so more rows fit in the pane.
- Options → General: the object-search switch is now "Keep the object search", on by default, and
  each list keeps its own text — a table search no longer filters views, functions or queries.
- The Editor settings (font, size, Show Line Numbers, Word Wrap) reach the editors that are already
  open, instead of only the tabs opened afterwards.

## [0.0.14] - 2026-10-18

### Added

- Options → Shortcuts: every shortcut is a command you can rebind — search the list, click a field and
  press the new keys, ↺ restores its default, ✕ removes it, and a conflict is flagged on both rows.
- The connection tree and the object list work from the keyboard: arrows, Page Up / Page Down,
  Home / End, ← / → to collapse and expand, Enter, F2, and typing a name jumps to it.
- Tabs: Ctrl+Q opens a query, Ctrl+W closes the tab, Ctrl+Tab / Ctrl+Shift+Tab walk them in the tab
  bar's order, and the tab menu shows each action's shortcut.
- The table grid moves like a spreadsheet (arrows, Page Up / Page Down, Home / End, Ctrl+Home /
  Ctrl+End, Tab, Shift extends), F5 reloads in place, and F2 opens a value editor under the grid.
- The query editor: Ctrl+K or Ctrl+/ toggles a comment, Ctrl+D duplicates, Ctrl+T swaps lines,
  Alt+↑ / Alt+↓ move them, and Format SQL is undoable.
- Queries run statement by statement on one connection: each keeps its own result, error and time,
  and the Info tab logs them with the failing statement marked.
- A MySQL dump with `DELIMITER` and `CREATE PROCEDURE` bodies runs as it does in the mysql client.
- The next start reopens the query tabs (with unsaved text), the connections, the open databases and
  the table tabs that were open.
- Options → General: the statement limit per run, stopping at the first error, and keeping the object
  search across tabs; right-click a setting to copy its name or path.

### Changed

- Comments follow Navicat: `--`, `//`, `#` (MySQL) and an unclosed `/*` comment out the rest of the
  line with or without a space, and what looks commented out is never sent to the server.
- Completion offers the statement's own columns and aliases first where a column is expected.
- Clicking a cell that is already selected edits it; ← / → inside a cell stay there.
- Close Other Tabs and Close All are on Ctrl+Alt+W and Ctrl+Shift+W, rebindable like every command.

### Fixed

- A `DELIMITER GO` no longer ends a statement inside a word such as `CATEGORY`.
- `END$$` ends a statement, so a routine written without a space before its delimiter splits.

## [0.0.13] - 2026-10-16

### Added

- Each engine's designer shows its own shape: SQL Server gets Collation / Sparse, Size / Scale / Computed
  and Unique Keys / Checks tabs; SQLite, PostgreSQL and Oracle drop the pages they lack.
- SQL Server's designer edits unique constraints and check constraints as well.
- The index field editor is Navicat's field list: tick the fields, drag them into order, set Sort / Prefix.
- The designer's fields grid navigates with Tab / Shift+Tab, and its Type drop-down works from the keyboard.
- SQL Preview panes are selectable and copyable — the designer's, the user window's and Object Privileges'.

### Changed

- An index column carries its own options (`DESC`, MySQL's prefix) and round-trips them.
- The Fields / Referenced Fields cells are drop-downs: click the cell, no "..." button.
- The designer's field details, field list and Options page are no longer MySQL-shaped on every engine.
- The light palette reads like Navicat's: white chrome and visible separators instead of grey fills.
- The Indexes grid lists secondary indexes only; the primary key is managed from the Fields tab.
- The strip in front of the first field is narrower, and the designer's labels share one left edge.
- Index Type shows the engine's own type text; Index Method appears only where the engine has more than one.
- The designer's Triggers tab is gone — it could only ever show an empty grid.

### Fixed

- The designer's grids scroll with the mouse wheel.
- Auto Increment appears only on the types an engine accepts, never as a dead control.
- Saving a table's structure reports success and closes the open cell editor; a save with nothing to change
  says so.
- A changed index survives Save on every engine.
- Adding a field, unique key or check starts unnamed, scrolled into view and ready to type.
- The index field editor no longer opens off-screen, and its Index Type column is no longer clipped.
- A new table's SQL Preview works before the table has a name.

## [0.0.12] - 2026-10-15

### Added

- **Changelog** in Options → About: once a check has found a newer release, the panel shows that
  version's notes — what installing would bring, in the UI's own language — and stays off the page
  entirely while this build already is the newest release. The repository keeps its changelog in
  English and Chinese, and the release job publishes each version's section as the release notes
  (it fails when a version's section is missing).
- SQL completion matches loosely: a candidate is found by its exact name, a prefix, anything inside
  it or a subsequence of it, with the closest matches leading; the tables lead right after `FROM` /
  `JOIN` / `INTO` / `UPDATE` / `TABLE`, and the everyday keywords (in their most-typed order) come
  before the rest of the language. `Tab` accepts the highlighted candidate, and still indents when
  no popup is open.
- The query editor's run keystrokes are configurable in Options → **Shortcuts**: click a field, press
  the combination, and OK applies it (Default restores Navicat's Ctrl+R / Ctrl+Shift+R). Ctrl+R runs
  the whole editor, Ctrl+Shift+R only the selection, and Ctrl+Enter stays an alias for running.

### Fixed

- Grid cells and headers have 6px of horizontal padding, so a value no longer sits flush against the
  previous column's divider and its text (issue #5).
- The MySQL account editor lists a database the account only holds an entry for
  (`GRANT USAGE ON db.* WITH GRANT OPTION`), which Navicat and `SHOW DATABASES` report as well.
- **Close Connection** closes that connection's account window and Object Privileges window with it
  and drops the Users list, so no window is left editing an account over a dead connection.

## [0.0.11] - 2026-10-15

### Added

- Grid **Field Filter** popup: hide, reorder (drag handle) and search fields, with Reset /
  Select All / Invert; edits land on a draft and only OK applies them, and the layout is remembered
  per connection / database / table.
- Field comments and data types in the grid header, controlled by the new **Data** page of the
  Options window (Show field comments / Show field types, both off by default).

### Fixed

- Field Filter now applies the draft only on OK (Cancel / click-away discards), uses one state-aware
  icon, opens a taller popup, and Invert never hides the last visible field.
- The tree's **Refresh** re-fetches the open database's tables and routines, so external changes
  show up.
- Run SQL File releases its pinned session inside the Tokio runtime instead of crashing when a
  script finishes.
- Read-only errors and SQL text are selectable/copyable; a submenu flyout closes when the pointer
  leaves it; filter and tree interaction polish; an empty grid renders as a full empty table.

## [0.0.10] - 2026-10-14

### Added

- Connection tree multi-select, drag-and-drop into groups, and **Import Connections**
  (RustGrid / Navicat / DBX) plus **Export Connections** in the connection menus.
- macOS ships a `.app` bundle with a Dock icon, and the updater swaps the bundle as one.

### Fixed

- The grid keeps its selection while the scrollbar thumb is dragged.
- Search placeholders retranslate when the language changes.
- Docs: added the README screenshot grid.

## [0.0.9] - 2026-10-13

### Fixed

- Grid cursor-wheel scrolling, single-line cells, backup pause, and multi-row delete.

## [0.0.8] - 2026-10-13

### Added

- Per-connection environment colour (Red / Yellow / Green) that paints the connection node and its whole
  subtree in the tree, plus every tab of that connection.

### Fixed

- The account editor requires a new password when the authentication plugin changes.

## [0.0.7] - 2026-10-12

### Changed

- Upgraded gpui-kit to 0.7.3 and adopted its new components.

### Fixed

- The custom titlebar stays clear of the macOS window buttons.

## [0.0.6] - 2026-10-12

### Added

- Per-engine connection pages and engine-specific options (General / TLS / Tunnel/Proxy / Advanced), driven by the
  driver descriptor.
- Dump SQL file and Run SQL File, with one pinned session per script and a live log box.
- Connection groups (nested, rename, delete, drag-reorder) and multi-row table delete.
- Database dump/run SQL file, schema double-click, and dialog / log fixes.

### Fixed

- Connection type stays directly above the field it controls; the dump repeats the INSERT header on
  every batch; Copy Log placement; record counts in the run log.

## [0.0.5] - 2026-10-10

### Fixed

- Keep the tree selection when switching main tabs, gate schema objects, and widen the filter dropdown.
- PostgreSQL: decode `"char"` catalog columns as text.

## [0.0.4] - 2026-10-10

### Added

- Live download progress for the self-updater.

### Fixed

- Schema gating, cleaner chrome, and arrow-key cell navigation.
- Grid paste into new rows, backup dialog polish, and the search input.

## [0.0.3] - 2026-10-09

### Added

- MySQL **Advanced** section for SSL / REQUIRE in the account editor.

### Changed

- UX overhaul and a new app icon; table-designer edit fix.

### Fixed

- Decode the unsigned `password_lifetime` column; backup profile rename; only show the applicable
  schema action.

## [0.0.2] - 2026-10-09

### Added

- Engine-aware account and privilege management: SQL Server user mapping, securables and full login
  attributes, and `WITH GRANT OPTION` for server privileges.
- Open / close a schema in the tree and highlight the open schema.

### Fixed

- Guarded `CREATE USER` and server-scope grants run in `master`; server-securables matrix alignment
  and grant/deny cycling; column overlap in the server privileges grid.

## [0.0.1] - 2026-10-08

First release.

### Added

- Cargo workspace scaffold: engine-agnostic `rustgrid-core`, a MySQL driver, the config store and
  the GPUI application; licensed under Apache-2.0.
- Connection management (create / edit / delete, connect, password prompt, encrypted secrets) and
  database management (charset / collation).
- Tables and views with a paginated, editable grid — inserts, multi-range Excel-like selection,
  sorting, a filter builder, resizable columns and a sum of the selected numeric cells.
- SQL editor with syntax highlighting, keyword/table completion, Beautify SQL, Explain, saved named
  queries, multi-result tabs, and an Extract SQL window.
- Backup / restore with RustGrid's own `.rgbak` container, and the import / export wizards
  (Excel / CSV / TXT / `.xlsx` / `.sql`).
- User and privilege management, stored-routine and view designers, and a unified Explorer-style
  list interaction shared by every list page.
- Drivers: MySQL / MariaDB, SQLite, SQL Server, PostgreSQL, Oracle, and a runtime-loaded generic
  ODBC engine (DB2, Dameng, ...).
- gpui-kit / shadcn chrome with light, dark and system themes, in English and 简体中文.
- Portable GitHub Releases builds with a self-updater.

[0.0.11]: https://github.com/yisier/rustgrid/releases/tag/v0.0.11
[0.0.10]: https://github.com/yisier/rustgrid/releases/tag/v0.0.10
[0.0.9]: https://github.com/yisier/rustgrid/releases/tag/v0.0.9
[0.0.8]: https://github.com/yisier/rustgrid/releases/tag/v0.0.8
[0.0.7]: https://github.com/yisier/rustgrid/releases/tag/v0.0.7
[0.0.6]: https://github.com/yisier/rustgrid/releases/tag/v0.0.6
[0.0.5]: https://github.com/yisier/rustgrid/releases/tag/v0.0.5
[0.0.4]: https://github.com/yisier/rustgrid/releases/tag/v0.0.4
[0.0.3]: https://github.com/yisier/rustgrid/releases/tag/v0.0.3
[0.0.2]: https://github.com/yisier/rustgrid/releases/tag/v0.0.2
[0.0.1]: https://github.com/yisier/rustgrid/releases/tag/v0.0.1

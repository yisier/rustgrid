use super::*;

/// The connection status shown by a tree row, reduced to what rendering needs. A failed connection
/// keeps only its state: the error itself is surfaced in a dialog when it happens.
enum TreeStatus {
    Connected,
    Connecting,
    Failed,
    Disconnected,
}

/// A lightweight, owned snapshot of one connection for rendering the tree. `AppView` keeps the
/// real `ConnectionNode`; the snapshot only clones names and flags, never passwords.
struct TreeConnection {
    index: usize,
    /// The connection profile's id, used to place the row in the persisted tree layout.
    id: String,
    name: String,
    /// The engine's icon asset path and how it is drawn, from the driver's descriptor.
    icon: &'static str,
    icon_style: DriverIconStyle,
    /// Whether the engine has stored routines; when false the Functions category is hidden.
    supports_routines: bool,
    /// Whether the engine has schemas; when false the flat category tree is used and the
    /// schema create/drop actions are not offered.
    supports_schemas: bool,
    /// Whether the connection supports `.rgbak` backup/restore; when false the Backups category is
    /// hidden (the MySQL driver's StarRocks compatibility mode turns this off).
    supports_backups: bool,
    /// Whether the engine is a key-value store (Redis): objects are keys with namespaces, so the
    /// tree shows namespace folders instead of table/view/function categories.
    supports_key_value: bool,
    status: TreeStatus,
    /// The error from the last failed connect, shown as a tooltip on the node.
    failed_error: Option<String>,
    expanded: bool,
    /// The connection's environment colour, tinting this connection's whole subtree (参考 Navicat).
    color: Option<ConnectionColor>,
    databases: Loadable<Vec<TreeDatabase>>,
    /// Saved queries belonging to this connection, across all its databases.
    saved_queries: Vec<TreeSavedQuery>,
    /// Backup files and saved backup configurations belonging to this connection.
    backups: Vec<TreeBackup>,
}

struct TreeDatabase {
    index: usize,
    name: String,
    opened: bool,
    expanded: bool,
    categories: CategoryExpansion,
    tables: Loadable<Vec<TreeTable>>,
    routines: Loadable<Vec<RoutineInfo>>,
    /// Schema names derived from the object names (`dbo`, `sales`, ...). Empty for engines whose
    /// objects are not schema-qualified (MySQL/SQLite), which then render categories directly.
    schemas: Vec<String>,
    expanded_schemas: BTreeSet<String>,
    opened_schemas: BTreeSet<String>,
    /// The key-value namespaces (Redis key prefixes) under this database.
    namespaces: Loadable<Vec<rustgrid_core::ObjectNamespace>>,
}

#[derive(Clone)]
struct TreeTable {
    name: String,
    /// The leaf label shown in the tree: the bare object name when `name` is
    /// schema-qualified (`users` for `dbo.users`), so it is not redundant under a schema node.
    display: String,
    is_view: bool,
    /// The schema prefix of `name` (`dbo` in `dbo.users`), when present.
    schema: Option<String>,
}

/// Split a schema-qualified object name (`dbo.users`) into `(schema, object)`.
fn split_schema(name: &str) -> Option<(String, &str)> {
    name.split_once('.')
        .filter(|(schema, object)| !schema.is_empty() && !object.is_empty())
        .map(|(schema, object)| (schema.to_string(), object))
}

/// The distinct schema prefixes among the loaded tables/routines. Empty when no object is
/// schema-qualified, so schema-less engines keep the flat database → category tree.
fn build_schema_nodes(
    tables: &Loadable<Vec<TreeTable>>,
    routines: &Loadable<Vec<RoutineInfo>>,
) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    if let Loadable::Loaded(items) = tables {
        for table in items {
            if let Some(schema) = &table.schema {
                names.insert(schema.clone());
            }
        }
    }
    if let Loadable::Loaded(items) = routines {
        for routine in items {
            if let Some((schema, _)) = split_schema(&routine.name) {
                names.insert(schema);
            }
        }
    }
    names.into_iter().collect()
}

/// One saved-query leaf shown under a database's Queries category in the connection tree.
struct TreeSavedQuery {
    /// The database folder the query belongs to, matching a `TreeDatabase`'s name.
    database: String,
    /// The schema folder the query was filed under, when it has one.
    schema: Option<String>,
    /// The query's display name.
    name: String,
    /// The index into `AppView::query_files`, used to open it.
    index: usize,
    /// The query list's selection key (`q:<path>`).
    key: String,
}

/// Which kind of backup entry a tree leaf stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeBackupKind {
    File,
    Config,
}

/// One backup file or saved configuration leaf under a database's Backups category.
struct TreeBackup {
    /// The database folder the backup belongs to, matching a `TreeDatabase`'s name.
    database: String,
    /// The entry's display name.
    name: String,
    /// Whether it is a `.rgbak` file or a saved configuration.
    kind: TreeBackupKind,
    /// The index into `AppView::backup_files` / `AppView::backup_configs`.
    index: usize,
    /// The backup list's selection key (`file:<name>` / `config:<name>`).
    key: String,
}

/// The tree row id of a backup entry. The kind letter keeps file and configuration indices apart
/// (both start at zero), and the database coordinates scope it like a query leaf.
fn backup_tree_id(connection_index: usize, database_index: usize, backup: &TreeBackup) -> String {
    let kind = match backup.kind {
        TreeBackupKind::File => 'f',
        TreeBackupKind::Config => 'c',
    };
    format!(
        "bak-{connection_index}-{database_index}-{kind}-{}",
        backup.index
    )
}

fn snapshot_connections(app: &AppView) -> Vec<TreeConnection> {
    app.connections
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let status = match &node.status {
                ConnectionStatus::Connected(_) => TreeStatus::Connected,
                ConnectionStatus::Connecting => TreeStatus::Connecting,
                ConnectionStatus::Failed(_) => TreeStatus::Failed,
                ConnectionStatus::Disconnected => TreeStatus::Disconnected,
            };
            let failed_error = match &node.status {
                ConnectionStatus::Failed(message) => Some(message.clone()),
                _ => None,
            };
            let databases = match &node.databases {
                Loadable::Idle => Loadable::Idle,
                Loadable::Loading => Loadable::Loading,
                Loadable::Failed(error) => Loadable::Failed(error.clone()),
                Loadable::Loaded(databases) => Loadable::Loaded(
                    databases
                        .iter()
                        .enumerate()
                        .map(|(index, database)| {
                            // Leaf tables are only read when the database is expanded.
                            let tables = if database.expanded {
                                match &database.tables {
                                    Loadable::Idle => Loadable::Idle,
                                    Loadable::Loading => Loadable::Loading,
                                    Loadable::Failed(error) => Loadable::Failed(error.clone()),
                                    Loadable::Loaded(tables) => Loadable::Loaded(
                                        tables
                                            .iter()
                                            .map(|table| {
                                                let (schema, object) =
                                                    match split_schema(&table.name) {
                                                        Some((schema, object)) => {
                                                            (Some(schema), object.to_string())
                                                        }
                                                        None => (None, table.name.clone()),
                                                    };
                                                TreeTable {
                                                    name: table.name.clone(),
                                                    display: object,
                                                    is_view: matches!(
                                                        table.kind,
                                                        rustgrid_core::ObjectKind::View
                                                    ),
                                                    schema,
                                                }
                                            })
                                            .collect(),
                                    ),
                                }
                            } else {
                                Loadable::Idle
                            };
                            // Routine leaves are only read when the Functions category is expanded.
                            let routines = if database.expanded {
                                match &database.routines {
                                    Loadable::Idle => Loadable::Idle,
                                    Loadable::Loading => Loadable::Loading,
                                    Loadable::Failed(error) => Loadable::Failed(error.clone()),
                                    Loadable::Loaded(routines) => {
                                        Loadable::Loaded(routines.clone())
                                    }
                                }
                            } else {
                                Loadable::Idle
                            };
                            // Merge the schemas derived from object names with the database's own
                            // schema list (loaded on open), so an empty/newly created schema
                            // still appears in the tree.
                            let mut schemas = build_schema_nodes(&tables, &routines);
                            if let Some(Loadable::Loaded(loaded)) = &database.schemas {
                                for name in loaded {
                                    if !schemas.contains(name) {
                                        schemas.push(name.clone());
                                    }
                                }
                                schemas.sort();
                            }
                            TreeDatabase {
                                index,
                                name: database.name.clone(),
                                opened: database.opened,
                                expanded: database.expanded,
                                categories: database.categories,
                                tables,
                                routines,
                                schemas,
                                expanded_schemas: database.expanded_schemas.clone(),
                                opened_schemas: database.opened_schemas.clone(),
                                namespaces: match &database.namespaces {
                                    Loadable::Idle => Loadable::Idle,
                                    Loadable::Loading => Loadable::Loading,
                                    Loadable::Failed(error) => Loadable::Failed(error.clone()),
                                    Loadable::Loaded(namespaces) => {
                                        Loadable::Loaded(namespaces.clone())
                                    }
                                },
                            }
                        })
                        .collect(),
                ),
            };
            let descriptor = app
                .registry
                .get(&node.profile.driver)
                .map(|driver| driver.descriptor());
            TreeConnection {
                index,
                id: node.profile.id.clone(),
                name: node.profile.name.clone(),
                icon: descriptor
                    .as_ref()
                    .map_or("icons/connection.svg", |descriptor| descriptor.icon),
                icon_style: descriptor
                    .as_ref()
                    .map_or(DriverIconStyle::Plain, |descriptor| descriptor.icon_style),
                supports_routines: app.driver_supports(index, DriverCapability::Routines),
                supports_schemas: app.driver_supports(index, DriverCapability::Schemas),
                supports_backups: app.driver_supports_backup(index),
                supports_key_value: app.driver_supports(index, DriverCapability::KeyValue),
                status,
                failed_error,
                expanded: node.expanded,
                color: app.connection_color(index),
                databases,
                saved_queries: app
                    .query_files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| file.connection_id == node.profile.id)
                    .map(|(index, file)| TreeSavedQuery {
                        database: file.database.clone(),
                        schema: file.schema.clone(),
                        name: file.name.clone(),
                        index,
                        key: query::query_file_key(file),
                    })
                    .collect(),
                backups: app
                    .backup_files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| file.connection_id == node.profile.id)
                    .map(|(index, file)| TreeBackup {
                        database: file.database.clone(),
                        name: file.name.clone(),
                        kind: TreeBackupKind::File,
                        index,
                        key: backup::backup_file_key(&file.name),
                    })
                    .chain(
                        app.backup_configs
                            .iter()
                            .enumerate()
                            .filter(|(_, config)| config.connection_id == node.profile.id)
                            .map(|(index, config)| TreeBackup {
                                database: config.database.clone(),
                                name: config.name.clone(),
                                kind: TreeBackupKind::Config,
                                index,
                                key: backup::backup_config_key(&config.name),
                            }),
                    )
                    .collect(),
            }
        })
        .collect()
}

/// What a tree row does when the keyboard expands, collapses or opens it.
type NavAction = Rc<dyn Fn(&mut TreePane, &mut Window, &mut Context<'_, TreePane>)>;

/// One drawn tree row, recorded by its renderer in draw order so keyboard navigation walks exactly
/// what is on screen (groups, connections, databases, schemas, categories and their objects).
#[derive(Clone)]
pub(super) struct NavRow {
    id: String,
    /// The visible name, for typing a name to jump to the row.
    label: String,
    /// Nesting level; a row's parent is the nearest row above it with a smaller depth.
    depth: usize,
    /// `Some(expanded)` for a row with children, `None` for a leaf.
    expanded: Option<bool>,
    /// Expand / collapse this row (Right / Left).
    toggle: Option<NavAction>,
    /// Open this row (Enter): its double-click action.
    open: Option<NavAction>,
    /// The table / view a table row stands for, so F2 knows what to rename.
    table: Option<(usize, usize, String, bool)>,
}

impl NavRow {
    fn new(id: String, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            depth: 0,
            expanded: None,
            toggle: None,
            open: None,
            table: None,
        }
    }

    /// A row with children: Right / Left expand and collapse it, and Enter does the same.
    fn branch(mut self, expanded: bool, toggle: NavAction) -> Self {
        self.expanded = Some(expanded);
        self.open = Some(toggle.clone());
        self.toggle = Some(toggle);
        self
    }

    fn opens(mut self, open: NavAction) -> Self {
        self.open = Some(open);
        self
    }
}

/// How long a pause ends a typed name: the next key starts a new one (Explorer-like). Shared with
/// the object list.
pub(super) const TYPEAHEAD_RESET: std::time::Duration = std::time::Duration::from_millis(1000);

/// The index of the parent of row `index`: the nearest row above it with a smaller depth.
fn nav_parent(depths: &[usize], index: usize) -> Option<usize> {
    let depth = *depths.get(index)?;
    (0..index)
        .rev()
        .find(|&candidate| depths[candidate] < depth)
}

/// The row a typed `prefix` jumps to: the first row after `current` (wrapping) whose label starts
/// with it, ignoring case. A one-letter prefix starts after the current row, so repeating a letter
/// cycles through the rows that begin with it; a longer prefix may stay on the current row.
pub(super) fn typeahead_target(
    labels: &[&str],
    current: Option<usize>,
    prefix: &str,
) -> Option<usize> {
    if labels.is_empty() || prefix.is_empty() {
        return None;
    }
    let prefix = prefix.to_lowercase();
    let start = match current {
        Some(index) if prefix.chars().count() == 1 => index + 1,
        Some(index) => index,
        None => 0,
    };
    (0..labels.len())
        .map(|offset| (start + offset) % labels.len())
        .find(|&index| labels[index].to_lowercase().starts_with(&prefix))
}

impl TreePane {
    pub(super) fn new(app: WeakEntity<AppView>, cx: &mut Context<'_, Self>) -> Self {
        Self {
            app,
            scroll: ScrollHandle::new(),
            selected: None,
            selected_table: None,
            rename_row: None,
            group_renaming: false,
            focus: cx.focus_handle(),
            theme: Theme::dark(),
            visible_ids: Vec::new(),
            selection: BTreeSet::new(),
            nav_rows: Vec::new(),
            nav_build: std::cell::RefCell::new(Vec::new()),
            nav_depth: std::cell::Cell::new(0),
            row_bounds: Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            typeahead: String::new(),
            typeahead_at: None,
            nav_generation: 0,
        }
    }

    /// Record `row` (drawn at the current depth) for keyboard navigation and return it wrapped so
    /// its window bounds are captured each frame. Call it before drawing the row's children.
    fn nav_row(&self, row: impl IntoElement, mut nav: NavRow) -> AnyElement {
        nav.depth = self.nav_depth.get();
        let id = nav.id.clone();
        self.nav_build.borrow_mut().push(nav);
        let bounds = self.row_bounds.clone();
        div()
            .w_full()
            .child(row)
            .on_children_prepainted(move |children, _window, _cx| {
                if let Some(row) = children.first() {
                    bounds.borrow_mut().insert(id.clone(), *row);
                }
            })
            .into_any_element()
    }

    /// Draw a row's children one level deeper.
    fn nested<R>(&self, draw: impl FnOnce() -> R) -> R {
        self.nav_depth.set(self.nav_depth.get() + 1);
        let result = draw();
        self.nav_depth.set(self.nav_depth.get() - 1);
        result
    }

    /// The index of the selected row in the last frame's rows.
    fn nav_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.nav_rows.iter().position(|row| &row.id == selected)
    }

    /// Select row `index` from the keyboard: highlight it, keep the multi-selection in step and
    /// scroll it into view.
    fn nav_select(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(row) = self.nav_rows.get(index) else {
            return;
        };
        let id = row.id.clone();
        self.selected = Some(id.clone());
        self.selected_table = row.table.clone();
        // A selectable row (connection / group) becomes the single selection; any other row clears
        // the multi-selection. Only the tree redraws now; the rest of the app catches up once the
        // keys settle, so holding an arrow key stays smooth.
        if let Some(app) = self.app.upgrade() {
            let selectable = list_ops::is_tree_selectable(&id);
            app.update(cx, |app, _| {
                if selectable {
                    app.tree_selection.select_one(id.clone());
                } else {
                    app.tree_selection.clear();
                }
            });
        }
        self.reveal(&id);
        self.nav_generation += 1;
        let generation = self.nav_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NAV_SETTLE).await;
            let _ = this.update(cx, |tree, cx| {
                if tree.nav_generation == generation
                    && let Some(app) = tree.app.upgrade()
                {
                    app.update(cx, |_, cx| cx.notify());
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Move the selection by `delta` rows (clamped), or to the first row when nothing is selected.
    fn nav_move(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        if self.nav_rows.is_empty() {
            return;
        }
        let last = self.nav_rows.len() - 1;
        let next = match self.nav_index() {
            Some(index) => index.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.nav_select(next, cx);
    }

    /// How many rows one Page Up / Page Down moves: a screenful, less one row of context.
    fn nav_page(&self) -> isize {
        let height = f32::from(self.scroll.bounds().size.height);
        ((height / 22.0).floor() as isize - 1).max(1)
    }

    /// Scroll the row `id` into view if the last frame drew it outside the visible area.
    fn reveal(&self, id: &str) {
        if let Some(row) = self.row_bounds.borrow().get(id).copied() {
            ui::reveal_vertically(&self.scroll, row);
        }
    }

    /// Put the tree's cursor on a database row and scroll it into view. Used when a table/view tab
    /// is brought to the front: the tree must show the database the active tab belongs to, even
    /// when the user opened tables from several databases.
    pub(super) fn select_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let id = format!("db-{connection_index}-{database_index}");
        let changed = self.selected.as_deref() != Some(id.as_str());
        if changed {
            self.selected = Some(id.clone());
            self.selected_table = None;
            // The row may first be drawn (the connection was just expanded) by this frame, so
            // reveal again once the frame has been painted.
            let reveal_id = id.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(NAV_SETTLE).await;
                let _ = this.update(cx, |tree, cx| {
                    tree.reveal(&reveal_id);
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
        }
        self.reveal(&id);
    }

    /// Right: expand a collapsed row, or step into an expanded row's first child.
    fn nav_expand(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(index) = self.nav_index() else {
            return self.nav_move(0, cx);
        };
        let row = self.nav_rows[index].clone();
        match row.expanded {
            Some(false) => {
                if let Some(toggle) = row.toggle {
                    toggle(self, window, cx);
                }
            }
            Some(true)
                if self
                    .nav_rows
                    .get(index + 1)
                    .is_some_and(|next| next.depth > row.depth) =>
            {
                self.nav_select(index + 1, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Left: collapse an expanded row, or move to the parent of any other row.
    fn nav_collapse(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(index) = self.nav_index() else {
            return self.nav_move(0, cx);
        };
        let row = self.nav_rows[index].clone();
        if row.expanded == Some(true)
            && let Some(toggle) = row.toggle
        {
            toggle(self, window, cx);
        } else {
            let depths: Vec<usize> = self.nav_rows.iter().map(|row| row.depth).collect();
            if let Some(parent) = nav_parent(&depths, index) {
                self.nav_select(parent, cx);
            }
        }
        cx.notify();
    }

    /// Enter: open the selected row (a table's grid, a routine, a query, or expand a branch).
    fn nav_open(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(index) = self.nav_index() else {
            return;
        };
        if let Some(open) = self.nav_rows[index].open.clone() {
            open(self, window, cx);
        }
        cx.notify();
    }

    /// Typing a name jumps to the next row whose label starts with it; a pause starts a new name.
    /// Returns whether the keystroke was used.
    ///
    /// The tree only ever *reads* the keystroke: it must not claim it (`stop_propagation`), because
    /// gpui treats a claimed key-down as handled and the Windows backend then skips
    /// `TranslateMessage`, so the matching character message never reaches an in-place editor
    /// inside the tree. Typing therefore belongs to an open rename editor and does nothing here.
    fn nav_typeahead(&mut self, keystroke: &Keystroke, cx: &mut Context<'_, Self>) -> bool {
        let modifiers = keystroke.modifiers;
        if modifiers.control
            || modifiers.alt
            || modifiers.platform
            || self.rename_row.is_some()
            || self.group_renaming
        {
            return false;
        }
        let Some(character) = keystroke
            .key_char
            .as_ref()
            .and_then(|text| text.chars().next())
            .filter(|character| !character.is_control())
        else {
            return false;
        };
        let now = std::time::Instant::now();
        if self
            .typeahead_at
            .is_none_or(|at| now.duration_since(at) > TYPEAHEAD_RESET)
        {
            self.typeahead.clear();
        }
        self.typeahead_at = Some(now);
        self.typeahead.push(character);
        let labels: Vec<&str> = self.nav_rows.iter().map(|row| row.label.as_str()).collect();
        if let Some(target) = typeahead_target(&labels, self.nav_index(), &self.typeahead) {
            self.nav_select(target, cx);
        }
        true
    }

    pub(super) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Keys no command claims: typing a name jumps to the matching row. The navigation keys
    /// (arrows, Page Up/Down, Home/End, Enter, F2, Ctrl+A) are keymap commands handled by the
    /// `tree_*` actions.
    fn tree_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<'_, Self>) {
        // Never consume the keystroke: type-ahead only moves the highlight, while claiming the key
        // would suppress the character the platform delivers to the tree's rename editor.
        self.nav_typeahead(&event.keystroke, cx);
    }

    /// Ctrl/Cmd+A selects every visible connection/group row.
    fn tree_select_all(&mut self, cx: &mut Context<'_, Self>) {
        let selectable: Vec<String> = self
            .visible_ids
            .iter()
            .filter(|id| list_ops::is_tree_selectable(id))
            .cloned()
            .collect();
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| app.tree_select_all(&selectable, cx));
        }
        cx.notify();
    }

    /// F2: rename the selected group or table in place.
    fn tree_rename(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        if let Some(id) = self.selected.as_deref()
            && let Some(rest) = id.strip_prefix("group-")
            && let Ok(group_id) = rest.parse::<u64>()
        {
            app.update(cx, |app, cx| app.begin_group_rename(group_id, window, cx));
            cx.notify();
            return;
        }
        if let Some(id) = self.selected.clone() {
            if let Some(rest) = id.strip_prefix("qry-") {
                let mut parts = rest.splitn(3, '-');
                let _connection = parts.next();
                let _database = parts.next();
                let index = parts.next().and_then(|value| value.parse::<usize>().ok());
                if let Some(index) = index {
                    app.update(cx, |app, cx| app.begin_rename_query(index, window, cx));
                    cx.notify();
                    return;
                }
            }
            if let Some(rest) = id.strip_prefix("bak-") {
                let mut parts = rest.splitn(4, '-');
                let _connection = parts.next();
                let _database = parts.next();
                let kind = parts.next();
                let index = parts.next().and_then(|value| value.parse::<usize>().ok());
                if let (Some(kind), Some(index)) = (kind, index) {
                    let target = match kind {
                        "f" => Some(BackupSelection::File(index)),
                        "c" => Some(BackupSelection::Config(index)),
                        _ => None,
                    };
                    if let Some(target) = target {
                        app.update(cx, |app, cx| app.begin_backup_rename(target, window, cx));
                        cx.notify();
                        return;
                    }
                }
            }
        }
        let Some((connection_index, database_index, name, is_view)) = self.selected_table.clone()
        else {
            return;
        };
        app.update(cx, |app, cx| {
            app.begin_rename_table(
                RowPane::Tree,
                connection_index,
                database_index,
                name,
                is_view,
                window,
                cx,
            );
            app.notify_rename_pane(RowPane::Tree, cx);
        });
        cx.notify();
    }

    /// Enter/click activation for a structural tree row.
    fn activate_tree_id(&mut self, id: &str, cx: &mut Context<'_, Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        if let Some(rest) = id.strip_prefix("conn-") {
            if let Ok(index) = rest.parse::<usize>() {
                app.update(cx, |app, cx| {
                    let connected = matches!(
                        app.connections.get(index).map(|node| &node.status),
                        Some(ConnectionStatus::Connected(_))
                    );
                    if connected {
                        app.toggle_expand(index, cx);
                    } else {
                        app.connect(index, cx);
                    }
                });
            }
        } else if let Some(rest) = id.strip_prefix("group-") {
            if let Ok(group_id) = rest.parse::<u64>() {
                app.update(cx, |app, cx| app.toggle_group(group_id, cx));
            }
        } else if let Some(rest) = id.strip_prefix("db-") {
            let mut parts = rest.split('-');
            let connection_index = parts.next().and_then(|value| value.parse::<usize>().ok());
            let database_index = parts.next().and_then(|value| value.parse::<usize>().ok());
            if let (Some(connection_index), Some(database_index)) =
                (connection_index, database_index)
            {
                app.update(cx, |app, cx| {
                    app.open_database(connection_index, database_index, cx)
                });
            }
        }
        cx.notify();
    }

    /// Commit an open in-place rename (e.g. before a click moves the selection elsewhere).
    fn commit_pending_rename(&self, cx: &mut Context<'_, Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, |app, cx| {
            app.submit_rename(cx);
            if app.query_rename.is_some() {
                app.submit_query_rename(cx);
            }
            if app.backup_rename.is_some() {
                app.submit_backup_rename(cx);
            }
        });
    }

    fn render_connection(
        &self,
        connection: &TreeConnection,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let index = connection.index;
        let conn_id = format!("conn-{index}");
        let selected = self.selection.contains(&conn_id);
        let connected = matches!(connection.status, TreeStatus::Connected);

        // A theme-independent badge color so the driver glyph keeps its contrast on both the
        // light and dark backgrounds.
        let badge = match &connection.status {
            TreeStatus::Connected => 0x2e9e5b,
            TreeStatus::Connecting => 0xb58900,
            TreeStatus::Failed => 0xd13438,
            TreeStatus::Disconnected => 0x6b6b6b,
        };

        let click_app = self.app.clone();
        let menu_app = self.app.clone();
        let drag_app = self.app.clone();
        let drag_id = connection.id.clone();
        // The environment colour is painted on the node itself, so it is visible even while the
        // connection is closed (no children to tint). A selected row uses the tree's strong
        // selection fill so it does not get lost in the tint; the colour stays visible through
        // the accent strip on its left edge.
        let node_tint = connection.color.map(|color| rgba(color.tint(theme, 0x66)));
        let node_hover = connection.color.map(|color| rgba(color.tint(theme, 0x80)));
        let node_accent = connection
            .color
            .filter(|_| selected)
            .map(|color| color.accent(theme));
        let row = div()
            .id(SharedString::from(conn_id.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl_1()
            .pr_2()
            .relative()
            .rounded_sm()
            .cursor_pointer()
            .overflow_hidden()
            .when_some(node_tint, |style, bg| style.bg(bg))
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| {
                    style.bg(node_hover.unwrap_or(rgba((theme.tree_hover_bg << 8) | 0xff)))
                })
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // Releasing a drag of the multi-selection must not collapse it to this row.
                if click_was_drag(event) {
                    return;
                }
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(format!("conn-{index}"));
                // Extend/replace the tree's multi-selection from the click's modifiers.
                let click_id = format!("conn-{index}");
                let visible: Vec<String> = this
                    .visible_ids
                    .iter()
                    .filter(|id| list_ops::is_tree_selectable(id))
                    .cloned()
                    .collect();
                let mode = selection_mode(event.modifiers());
                let shift = event.modifiers().shift;
                if let Some(app) = this.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.tree_row_click(&click_id, &visible, mode, shift, cx);
                        app.clear_info_selection();
                        cx.notify();
                    });
                }
                let click_count = event.click_count();
                let _ = click_app.update(cx, |app, cx| {
                    if app.click_opens(click_count, event.modifiers()) {
                        let connected = matches!(
                            app.connections.get(index).map(|node| &node.status),
                            Some(ConnectionStatus::Connected(_))
                        );
                        if connected {
                            app.toggle_expand(index, cx);
                        } else {
                            app.connect(index, cx);
                        }
                    }
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    if let Some(app) = drag_app.upgrade() {
                        app.update(cx, |app, _| {
                            app.tree_press_start(
                                TreeDragPayload::Connection(drag_id.clone()),
                                event.position,
                            );
                        });
                    }
                    let _ = this;
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    let row_id = format!("conn-{index}");
                    // Right-clicking inside a multi-selection keeps it and opens the multi menu;
                    // otherwise the row becomes the single selection with its own menu.
                    let multi = this.app.upgrade().is_some_and(|app| {
                        let app = app.read(cx);
                        app.tree_multi_selected() && app.tree_selection().contains(&row_id)
                    });
                    this.selected = Some(row_id.clone());
                    let _ = menu_app.update(cx, |app, cx| {
                        if multi {
                            app.context_menu = Some(ContextMenu {
                                target: ContextTarget::TreeSelection,
                                position: event.position,
                            });
                        } else {
                            app.tree_selection.select_one(row_id.clone());
                            app.context_menu = Some(ContextMenu {
                                target: ContextTarget::Connection(index),
                                position: event.position,
                            });
                        }
                        app.move_submenu = None;
                        app.color_submenu = None;
                        cx.notify();
                    });
                }),
            )
            .child(if connected {
                tree_chevron(connection.expanded, theme.chevron)
            } else {
                chevron_spacer()
            })
            .child(tree_driver_icon(
                connection.icon,
                connection.icon_style,
                badge,
                connected,
            ))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(connection.name.clone()),
            )
            .when(matches!(connection.status, TreeStatus::Connecting), |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.warning))
                        .child(t!("connection.connecting").to_string()),
                )
            })
            // Keep the environment colour readable on the selected row as a thin left strip.
            .when_some(node_accent, |row, color| {
                row.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .w(px(3.0))
                        .bg(rgb(color)),
                )
            });

        // A failed connection keeps its error, surfaced when the row is hovered (the dialog from
        // the attempt is long gone by then).
        let row = match connection.failed_error.clone() {
            Some(error) => row.tooltip(ui::text_tooltip(error)),
            None => row,
        };

        // A connection's environment colour paints a tint behind the connection and every row
        // under it, so the environment is visible at a glance (参考 Navicat). Rows keep their own
        // hover/selection background, which paints over this tint.
        let mut body = div().flex().flex_col().w_full();
        if let Some(color) = connection.color {
            body = body.bg(rgba(color.tint(theme, 0x33)));
        }
        let nav_id = format!("conn-{index}");
        let toggle_id = nav_id.clone();
        let body = body.child(self.nav_row(
            tree_row_with_rect(self.app.clone(), row, nav_id.clone()),
            NavRow::new(nav_id, connection.name.clone()).branch(
                connection.expanded,
                // Expanding a closed connection connects it; see `activate_tree_id`.
                Rc::new(move |this, _window, cx| this.activate_tree_id(&toggle_id, cx)),
            ),
        ));

        let mut sub = div().flex().flex_col().w_full();

        if connection.expanded {
            match &connection.databases {
                Loadable::Idle => {}
                Loadable::Loading => {
                    sub = sub.child(tree_message(
                        t!("common.loading").to_string(),
                        26.0,
                        theme.text_muted,
                    ));
                }
                Loadable::Failed(error) => {
                    sub = sub.child(tree_message(error.clone(), 26.0, theme.danger));
                }
                Loadable::Loaded(databases) => {
                    sub = self.nested(|| {
                        let mut sub = sub;
                        for database in databases {
                            sub = sub.child(self.render_database(
                                index,
                                database,
                                &connection.saved_queries,
                                &connection.backups,
                                connection.supports_routines,
                                connection.supports_schemas,
                                connection.supports_backups,
                                connection.supports_key_value,
                                cx,
                            ));
                        }
                        sub
                    });
                }
            }
        }

        body.child(sub)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_database(
        &self,
        connection_index: usize,
        database: &TreeDatabase,
        saved_queries: &[TreeSavedQuery],
        backups: &[TreeBackup],
        supports_routines: bool,
        supports_schemas: bool,
        supports_backups: bool,
        supports_key_value: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let database_index = database.index;
        let db_id = format!("db-{connection_index}-{database_index}");
        let selected = self.selected.as_deref() == Some(db_id.as_str());
        // The prototype puts the database's key count on its row. The namespace list covers every
        // scanned key (including the prefix-less ones under the empty name), so its counts are the
        // best available total; fall back to the loaded key table.
        let key_count = if supports_key_value {
            match &database.namespaces {
                Loadable::Loaded(namespaces) => Some(
                    namespaces
                        .iter()
                        .map(|namespace| namespace.objects)
                        .sum::<u64>(),
                ),
                _ => match &database.tables {
                    Loadable::Loaded(tables) => Some(tables.len() as u64),
                    _ => None,
                },
            }
        } else {
            None
        };

        let open_app = self.app.clone();
        let chevron_app = self.app.clone();
        let menu_app = self.app.clone();
        let row = div()
            .id(SharedString::from(db_id.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(18.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(format!("db-{connection_index}-{database_index}"));
                if let Some(app) = this.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.clear_info_selection();
                        cx.notify();
                    });
                }
                // A plain click shows an already-open database's objects for the active main tab; a
                // closed database is only highlighted (unless 单击模式 is on).
                let click_count = event.click_count();
                let _ = open_app.update(cx, |app, cx| {
                    if app.click_opens(click_count, event.modifiers()) {
                        app.open_database(connection_index, database_index, cx);
                    } else if app.click_is_plain(click_count) {
                        if app.driver_supports(connection_index, DriverCapability::KeyValue) {
                            app.set_object_namespace(connection_index, database_index, None, cx);
                        }
                        app.show_database_objects(connection_index, database_index, cx);
                    }
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.selected = Some(format!("db-{connection_index}-{database_index}"));
                    let _ = menu_app.update(cx, |app, cx| {
                        app.dump_submenu = None;
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Database {
                                connection_index,
                                database_index,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                }),
            )
            .child(if database.opened {
                div()
                    .id(SharedString::from(format!(
                        "db-toggle-{connection_index}-{database_index}"
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |_this, _event, _window, cx| {
                        let _ = chevron_app.update(cx, |app, cx| {
                            app.open_database(connection_index, database_index, cx);
                        });
                    }))
                    .child(tree_chevron(database.expanded, theme.chevron))
                    .into_any_element()
            } else {
                chevron_spacer()
            })
            .child(tree_icon(
                "icons/database.svg",
                if database.opened {
                    theme.icon_database_active
                } else {
                    theme.icon_database
                },
            ))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(database.name.clone()),
            )
            .when_some(key_count, |row, count| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(rgb(theme.text_muted))
                        .child(format!("({count})")),
                )
            });
        let nav_id = format!("db-{connection_index}-{database_index}");
        let toggle_id = nav_id.clone();
        let row = self.nav_row(
            row,
            NavRow::new(nav_id, database.name.clone()).branch(
                database.expanded,
                Rc::new(move |this, _window, cx| this.activate_tree_id(&toggle_id, cx)),
            ),
        );

        let mut sub = div().flex().flex_col().w_full();

        if database.expanded {
            self.nav_depth.set(self.nav_depth.get() + 1);
            if supports_key_value {
                // Redis: the database node expands to its key prefixes (the prototype's
                // namespace folders), not to SQL categories. The database row itself is the
                // "all keys" view, so there is no separate 全部 row and no 查询 category here
                // (saved queries live on the 查询 main tab).
                sub = sub.child(self.render_namespaces(
                    connection_index,
                    database_index,
                    database,
                    cx,
                ));
            } else {
                match &database.tables {
                    Loadable::Idle => {}
                    Loadable::Loading => {
                        sub = sub.child(tree_message(
                            t!("common.loading").to_string(),
                            36.0,
                            theme.text_muted,
                        ));
                    }
                    Loadable::Failed(error) => {
                        sub = sub.child(tree_message(error.clone(), 36.0, theme.danger));
                    }
                    Loadable::Loaded(_) => {
                        if !supports_schemas || database.schemas.is_empty() {
                            // Schema-less engines (MySQL/SQLite), or a schema engine whose database
                            // has no user schemas yet, keep the flat category list.
                            for category in Category::ALL {
                                if category == Category::Functions && !supports_routines {
                                    continue;
                                }
                                if category == Category::Backups && !supports_backups {
                                    continue;
                                }
                                sub = sub.child(self.render_category(
                                    connection_index,
                                    database_index,
                                    None,
                                    category,
                                    database,
                                    saved_queries,
                                    backups,
                                    cx,
                                ));
                            }
                        } else {
                            for schema in &database.schemas {
                                sub = sub.child(self.render_schema(
                                    connection_index,
                                    database_index,
                                    database,
                                    schema,
                                    saved_queries,
                                    backups,
                                    supports_routines,
                                    supports_backups,
                                    cx,
                                ));
                            }
                        }
                    }
                }
            }
            self.nav_depth.set(self.nav_depth.get() - 1);
        }

        div().flex().flex_col().w_full().child(row).child(sub)
    }

    /// The key-value namespaces (Redis key prefixes) under a database. Clicking one filters the
    /// key list to that prefix; clicking the database row clears the filter. This is the tree
    /// equivalent of Navicat's 命名空间选择 and of the HTML prototype's namespace folders.
    fn render_namespaces(
        &self,
        connection_index: usize,
        database_index: usize,
        database: &TreeDatabase,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let app = self.app.clone();
        let mut sub = div().flex().flex_col().w_full();

        match &database.namespaces {
            Loadable::Idle | Loadable::Loading => {}
            Loadable::Failed(error) => {
                sub = sub.child(tree_message(error.clone(), 36.0, theme.danger));
            }
            Loadable::Loaded(namespaces) => {
                for namespace in namespaces {
                    if namespace.name.is_empty() {
                        continue;
                    }
                    let id = format!(
                        "keys-{connection_index}-{database_index}-{}",
                        namespace.name
                    );
                    let selected = self.selected.as_deref() == Some(id.as_str());
                    let name = namespace.name.clone();
                    let label = format!("{name}*");
                    let clicked = namespace.name.clone();
                    let row_app = app.clone();
                    let menu_app = app.clone();
                    let menu_id = id.clone();
                    let menu_namespace = namespace.name.clone();
                    let nav_app = app.clone();
                    let nav_id = id.clone();
                    let nav_name = label.clone();
                    let nav_pick = name.clone();
                    let row = div()
                        .id(SharedString::from(id.clone()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .w_full()
                        .h(px(22.0))
                        .pl(px(36.0))
                        .pr_2()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(selected, move |style| {
                            style
                                .bg(rgb(theme.tree_active_bg))
                                .text_color(rgb(theme.tree_active_text))
                        })
                        .when(!selected, move |style| {
                            style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                        })
                        .on_click(cx.listener(move |this, _event, window, cx| {
                            this.commit_pending_rename(cx);
                            this.selected_table = None;
                            window.focus(&this.focus, cx);
                            this.selected = Some(id.clone());
                            let name = clicked.clone();
                            let app = row_app.clone();
                            // `select_key_namespace` updates the tree pane (to keep the selected
                            // row in step), so running it from inside this row's listener would
                            // borrow the tree a second time and abort the app. Defer one cycle,
                            // exactly like the saved-query and backup leaves do.
                            cx.defer(move |cx| {
                                let _ = app.update(cx, |app, cx| {
                                    app.select_key_namespace(
                                        connection_index,
                                        database_index,
                                        Some(name),
                                        cx,
                                    );
                                });
                            });
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                this.selected_table = None;
                                this.selected = Some(menu_id.clone());
                                let _ = menu_app.update(cx, |app, cx| {
                                    app.context_menu = Some(ContextMenu {
                                        target: ContextTarget::KeyNamespace {
                                            connection_index,
                                            database_index,
                                            namespace: menu_namespace.clone(),
                                        },
                                        position: event.position,
                                    });
                                    cx.notify();
                                });
                                cx.notify();
                            }),
                        )
                        .child(tree_icon("icons/folder.svg", theme.warning))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(theme.text_muted))
                                .child(namespace.objects.to_string()),
                        );
                    let nav_row =
                        NavRow::new(nav_id, nav_name).opens(Rc::new(move |_this, _window, cx| {
                            let app = nav_app.clone();
                            let pick = nav_pick.clone();
                            // Enter on a namespace row goes through the same app path as a
                            // click; defer it out of the tree's own listener (see above).
                            cx.defer(move |cx| {
                                let _ = app.update(cx, |app, cx| {
                                    app.select_key_namespace(
                                        connection_index,
                                        database_index,
                                        Some(pick.clone()),
                                        cx,
                                    );
                                });
                            });
                        }));
                    sub = sub.child(self.nav_row(row, nav_row));
                }
            }
        }
        sub
    }

    /// One schema node under a database (SQL Server), holding all of the database's categories
    /// for that schema.
    #[allow(clippy::too_many_arguments)]
    fn render_schema(
        &self,
        connection_index: usize,
        database_index: usize,
        database: &TreeDatabase,
        schema: &str,
        saved_queries: &[TreeSavedQuery],
        backups: &[TreeBackup],
        supports_routines: bool,
        supports_backups: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let expanded = database.expanded_schemas.contains(schema);
        let schema_id = format!("schema-{connection_index}-{database_index}-{schema}");
        let selected = self.selected.as_deref() == Some(schema_id.as_str());
        let click_id = schema_id.clone();
        let click_name = schema.to_string();
        let app = self.app.clone();
        let menu_app = self.app.clone();
        let menu_schema = schema.to_string();

        let row = div()
            .id(SharedString::from(schema_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(36.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                // A plain click shows an already-open schema's objects for the active main tab; a
                // closed schema is only highlighted (unless 单击模式 is on).
                let click_count = event.click_count();
                let name = click_name.clone();
                let _ = app.update(cx, |app, cx| {
                    if app.click_opens(click_count, event.modifiers()) {
                        app.select_schema(connection_index, database_index, name, cx);
                    } else if app.click_is_plain(click_count) {
                        app.show_schema_objects(connection_index, database_index, name, cx);
                    }
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.selected = Some(format!(
                        "schema-{connection_index}-{database_index}-{menu_schema}"
                    ));
                    let schema = menu_schema.clone();
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Schema {
                                connection_index,
                                database_index,
                                schema,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                }),
            )
            .child(tree_chevron(expanded, theme.chevron))
            .child(tree_icon(
                "icons/database.svg",
                if database.opened_schemas.contains(schema) {
                    theme.icon_database_active
                } else {
                    theme.icon_database
                },
            ))
            .child(div().child(schema.to_string()));
        let toggle_name = schema.to_string();
        let open_name = schema.to_string();
        let toggle_app = self.app.clone();
        let open_app = self.app.clone();
        let row = self.nav_row(
            row,
            NavRow::new(
                format!("schema-{connection_index}-{database_index}-{schema}"),
                schema,
            )
            .branch(
                expanded,
                Rc::new(move |_this, _window, cx| {
                    let name = toggle_name.clone();
                    let _ = toggle_app.update(cx, |app, cx| {
                        app.toggle_schema(connection_index, database_index, name, cx)
                    });
                }),
            )
            // Enter does what a double-click does: open the schema.
            .opens(Rc::new(move |_this, _window, cx| {
                let name = open_name.clone();
                let _ = open_app.update(cx, |app, cx| {
                    app.select_schema(connection_index, database_index, name, cx)
                });
            })),
        );

        let mut sub = div().flex().flex_col().w_full();
        if expanded {
            // Every category lives under the schema, so the whole tree reads as
            // database → schema → 表/视图/函数/查询/备份.
            sub = self.nested(|| {
                let mut sub = sub;
                for category in Category::ALL {
                    if category == Category::Functions && !supports_routines {
                        continue;
                    }
                    if category == Category::Backups && !supports_backups {
                        continue;
                    }
                    sub = sub.child(self.render_category(
                        connection_index,
                        database_index,
                        Some(schema),
                        category,
                        database,
                        saved_queries,
                        backups,
                        cx,
                    ));
                }
                sub
            });
        }

        div().flex().flex_col().w_full().child(row).child(sub)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_category(
        &self,
        connection_index: usize,
        database_index: usize,
        schema: Option<&str>,
        category: Category,
        database: &TreeDatabase,
        saved_queries: &[TreeSavedQuery],
        backups: &[TreeBackup],
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let expanded = database.categories.get(category);
        let scope = schema.unwrap_or("");
        let cat_id = format!(
            "cat-{connection_index}-{database_index}-{scope}-{}",
            category.id()
        );
        let selected = self.selected.as_deref() == Some(cat_id.as_str());
        // Under a schema the category sits one level deeper.
        let indent = if schema.is_some() { 54.0 } else { 36.0 };
        let label = t!(category.label()).to_string();
        let icon_color = match category {
            Category::Tables => theme.icon_tables,
            Category::Views => theme.icon_views,
            Category::Functions => theme.icon_functions,
            Category::Queries => theme.icon_queries,
            Category::Backups => theme.icon_backups,
        };
        let click_id = cat_id.clone();
        let click_schema = schema.map(str::to_string);
        let app = self.app.clone();
        let menu_app = self.app.clone();
        let menu_schema = schema.map(str::to_string);

        let row = div()
            .id(SharedString::from(cat_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(indent))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                let schema = click_schema.clone();
                let _ = app.update(cx, |app, cx| {
                    app.toggle_category(connection_index, database_index, schema, category, cx);
                });
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    // Tables offer New Table / Import / Export / Refresh; the other categories
                    // offer Refresh.
                    window.focus(&this.focus, cx);
                    this.selected_table = None;
                    this.selected = Some(format!(
                        "cat-{connection_index}-{database_index}-{}-{}",
                        menu_schema.clone().unwrap_or_default(),
                        category.id()
                    ));
                    let target = if category == Category::Tables {
                        ContextTarget::TableCategory {
                            connection_index,
                            database_index,
                            schema: menu_schema.clone(),
                        }
                    } else {
                        ContextTarget::ObjectCategory {
                            connection_index,
                            database_index,
                            category,
                        }
                    };
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target,
                            position: event.position,
                        });
                        cx.notify();
                    });
                    cx.notify();
                }),
            )
            .child(tree_chevron(expanded, theme.chevron))
            .child(tree_icon(category.icon_path(), icon_color))
            .child(div().child(label));
        let toggle_app = self.app.clone();
        let toggle_schema = schema.map(str::to_string);
        let row = self.nav_row(
            row,
            NavRow::new(
                format!(
                    "cat-{connection_index}-{database_index}-{scope}-{}",
                    category.id()
                ),
                t!(category.label()).to_string(),
            )
            .branch(
                expanded,
                Rc::new(move |_this, _window, cx| {
                    let schema = toggle_schema.clone();
                    let _ = toggle_app.update(cx, |app, cx| {
                        app.toggle_category(connection_index, database_index, schema, category, cx)
                    });
                }),
            ),
        );

        let mut sub = div().flex().flex_col().w_full();
        let depth = self.nav_depth.get();
        self.nav_depth.set(depth + 1);
        if expanded {
            match category {
                Category::Tables | Category::Views => {
                    if let Loadable::Loaded(tables) = &database.tables {
                        let want_view = category == Category::Views;
                        let mut leaf_index = 0usize;
                        for table in tables.iter() {
                            if table.is_view != want_view {
                                continue;
                            }
                            if let Some(schema) = schema
                                && table.schema.as_deref() != Some(schema)
                            {
                                continue;
                            }
                            let element = self.render_table(
                                connection_index,
                                database_index,
                                leaf_index,
                                &database.name,
                                indent + 18.0,
                                table,
                                cx,
                            );
                            // Enter opens the table's data grid, like a double-click.
                            let open_app = self.app.clone();
                            let open_database = database.name.clone();
                            let open_table = table.name.clone();
                            let is_view = table.is_view;
                            let mut nav = NavRow::new(
                                format!("tbl-{connection_index}-{database_index}-{}", table.name),
                                table.name.clone(),
                            )
                            .opens(Rc::new(
                                move |_this, _window, cx| {
                                    let (database, table) =
                                        (open_database.clone(), open_table.clone());
                                    let _ = open_app.update(cx, |app, cx| {
                                        app.select_table(
                                            connection_index,
                                            database,
                                            table,
                                            is_view,
                                            cx,
                                        )
                                    });
                                },
                            ));
                            nav.table = Some((
                                connection_index,
                                database_index,
                                table.name.clone(),
                                is_view,
                            ));
                            sub = sub.child(self.nav_row(element, nav));
                            leaf_index += 1;
                        }
                    }
                }
                Category::Functions => match &database.routines {
                    Loadable::Idle | Loadable::Loading => {
                        sub = sub.child(tree_message(
                            t!("common.loading").to_string(),
                            52.0,
                            theme.text_muted,
                        ));
                    }
                    Loadable::Failed(error) => {
                        sub = sub.child(tree_message(error.clone(), 52.0, theme.danger));
                    }
                    Loadable::Loaded(routines) => {
                        for (index, routine) in routines.iter().enumerate() {
                            if let Some(schema) = schema
                                && !split_schema(&routine.name)
                                    .is_some_and(|(candidate, _)| candidate == schema)
                            {
                                continue;
                            }
                            let element = self.render_routine(
                                connection_index,
                                database_index,
                                index,
                                &database.name,
                                indent + 18.0,
                                routine,
                                cx,
                            );
                            // Enter opens the routine's editor, like a double-click.
                            let open_app = self.app.clone();
                            let open_database = database.name.clone();
                            let open_name = routine.name.clone();
                            let kind = routine.kind;
                            let nav = NavRow::new(
                                format!("rtn-{connection_index}-{database_index}-{}", routine.name),
                                split_schema(&routine.name)
                                    .map_or(routine.name.as_str(), |(_, name)| name),
                            )
                            .opens(Rc::new(
                                move |_this, _window, cx| {
                                    let (database, name) =
                                        (open_database.clone(), open_name.clone());
                                    let _ = open_app.update(cx, |app, cx| {
                                        app.open_routine_by_name(
                                            connection_index,
                                            database,
                                            name,
                                            kind,
                                            cx,
                                        )
                                    });
                                },
                            ));
                            sub = sub.child(self.nav_row(element, nav));
                        }
                    }
                },
                Category::Queries => {
                    for query in saved_queries.iter().filter(|query| {
                        query.database == database.name
                            && match schema {
                                // A schema shows its own queries plus the database-level ones.
                                Some(scope) => {
                                    query.schema.is_none() || query.schema.as_deref() == Some(scope)
                                }
                                None => query.schema.is_none(),
                            }
                    }) {
                        let element =
                            self.render_saved_query(connection_index, database_index, query, cx);
                        // Enter opens the saved query, like a double-click.
                        let open_app = self.app.clone();
                        let file_index = query.index;
                        let nav = NavRow::new(
                            format!("qry-{connection_index}-{database_index}-{}", query.index),
                            query.name.clone(),
                        )
                        .opens(Rc::new(move |_this, _window, cx| {
                            let _ =
                                open_app.update(cx, |app, cx| app.open_saved_query(file_index, cx));
                        }));
                        sub = sub.child(self.nav_row(element, nav));
                    }
                }
                Category::Backups => {
                    for backup in backups
                        .iter()
                        .filter(|backup| backup.database == database.name)
                    {
                        let element =
                            self.render_tree_backup(connection_index, database_index, backup, cx);
                        // Enter opens it exactly as a double-click in the Backup list does: a
                        // `.rgbak` file opens Restore, a saved configuration opens its dialog.
                        let open_app = self.app.clone();
                        let index = backup.index;
                        let kind = backup.kind;
                        let nav = NavRow::new(
                            backup_tree_id(connection_index, database_index, backup),
                            backup.name.clone(),
                        )
                        .opens(Rc::new(move |_this, _window, cx| {
                            let _ = open_app.update(cx, |app, cx| match kind {
                                TreeBackupKind::File => app.open_restore_backup(index, cx),
                                TreeBackupKind::Config => app.open_backup_config(index, cx),
                            });
                        }));
                        sub = sub.child(self.nav_row(element, nav));
                    }
                }
            }
        }
        self.nav_depth.set(depth);

        div().flex().flex_col().w_full().child(row).child(sub)
    }

    /// One stored routine leaf under the connection tree's Functions category.
    #[allow(clippy::too_many_arguments)]
    fn render_routine(
        &self,
        connection_index: usize,
        database_index: usize,
        routine_index: usize,
        database_name: &str,
        indent: f32,
        routine: &RoutineInfo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        // The routine's schema-qualified name keeps the id unique across schema nodes.
        let _ = routine_index;
        let routine_id = format!("rtn-{connection_index}-{database_index}-{}", routine.name);
        let selected = self.selected.as_deref() == Some(routine_id.as_str());
        let name = routine.name.clone();
        let display = split_schema(&routine.name)
            .map(|(_, object)| object.to_string())
            .unwrap_or_else(|| routine.name.clone());
        let click_name = name.clone();
        let kind = routine.kind;
        let click_id = routine_id.clone();
        let database_name = database_name.to_string();
        let app = self.app.clone();
        let menu_app = self.app.clone();
        let menu_name = routine.name.clone();

        div()
            .id(SharedString::from(routine_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(indent))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                this.selected_table = None;
                let click_count = event.click_count();
                let _ = app.update(cx, |app, cx| {
                    if app.click_is_plain(click_count) {
                        app.set_info_routine(
                            connection_index,
                            database_index,
                            click_name.clone(),
                            kind,
                        );
                    }
                    if app.click_opens(click_count, event.modifiers()) {
                        app.open_routine_by_name(
                            connection_index,
                            database_name.clone(),
                            click_name.clone(),
                            kind,
                            cx,
                        );
                    }
                    cx.notify();
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.focus, cx);
                    this.selected = Some(format!(
                        "rtn-{connection_index}-{database_index}-{menu_name}"
                    ));
                    this.selected_table = None;
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Routine {
                                connection_index,
                                database_index,
                                name: menu_name.clone(),
                                kind,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                    cx.notify();
                }),
            )
            .child(tree_icon(
                routine_icon(kind),
                routine_icon_color(kind, theme),
            ))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(display),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table(
        &self,
        connection_index: usize,
        database_index: usize,
        table_index: usize,
        database_name: &str,
        indent: f32,
        table: &TreeTable,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let is_view = table.is_view;
        // The table's schema-qualified name keeps the id unique across schema nodes.
        let _ = table_index;
        let table_id = format!("tbl-{connection_index}-{database_index}-{}", table.name);
        let selected = self.selected.as_deref() == Some(table_id.as_str());
        let table_name = table.name.clone();
        let database_name = database_name.to_string();
        let click_id = table_id.clone();
        let app = self.app.clone();
        let menu_app = self.app.clone();
        let menu_name = table.name.clone();
        // The row being renamed draws the in-place editor instead of its label, so a bare
        // (transparent) editor never ghosts the old name behind the caret.
        let rename = self.rename_row.as_ref().filter(|row| {
            row.connection_index == connection_index
                && row.database_index == database_index
                && row.old_name == table.name
        });
        let editing_here = rename.is_some();
        let label: AnyElement = match rename {
            Some(row) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                .child(ui::rename_field(theme, row.input.clone()))
                .into_any_element(),
            None => div()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(table.display.clone())
                .into_any_element(),
        };

        div()
            .id(SharedString::from(table_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(indent))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if editing_here {
                    return;
                }
                this.commit_pending_rename(cx);
                this.selected_table = Some((
                    connection_index,
                    database_index,
                    table_name.clone(),
                    is_view,
                ));
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                let click_count = event.click_count();
                let _ = app.update(cx, |app, cx| {
                    if app.click_opens(click_count, event.modifiers()) {
                        app.select_table(
                            connection_index,
                            database_name.clone(),
                            table_name.clone(),
                            is_view,
                            cx,
                        );
                    } else if app.click_is_plain(click_count) {
                        // A plain click only selects the table (and updates the info pane);
                        // opening its data grid takes a double-click unless 单击模式 is on.
                        app.set_info_table(connection_index, database_index, table_name.clone());
                    }
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if editing_here {
                        return;
                    }
                    this.commit_pending_rename(cx);
                    window.focus(&this.focus, cx);
                    this.selected = Some(format!(
                        "tbl-{connection_index}-{database_index}-{table_index}"
                    ));
                    this.selected_table =
                        Some((connection_index, database_index, menu_name.clone(), is_view));
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Table {
                                connection_index,
                                database_index,
                                name: menu_name.clone(),
                                is_view,
                                pane: RowPane::Tree,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                    cx.notify();
                }),
            )
            .child(chevron_spacer())
            .child(tree_icon(
                if is_view {
                    "icons/views.svg"
                } else {
                    "icons/tables.svg"
                },
                if is_view {
                    theme.icon_views
                } else {
                    theme.icon_tables
                },
            ))
            .child(label)
    }

    /// One saved-query leaf under the connection tree's Queries category. A single click selects
    /// it (and scopes the Queries main tab to its database); a double-click opens it. Ctrl/Shift
    /// clicks extend the same selection the Queries list uses, and the row's context menu is the
    /// list's.
    fn render_saved_query(
        &self,
        connection_index: usize,
        database_index: usize,
        query: &TreeSavedQuery,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let query_id = format!("qry-{connection_index}-{database_index}-{}", query.index);
        let selected = self.selected.as_deref() == Some(query_id.as_str())
            || self
                .app
                .upgrade()
                .is_some_and(|app| app.read(cx).query_selection.contains(&query.key));
        let rename = self.app.upgrade().and_then(|app| {
            let app = app.read(cx);
            (app.main_tab != MainTab::Queries)
                .then(|| {
                    app.query_rename
                        .as_ref()
                        .filter(|edit| edit.index == query.index)
                        .map(|edit| edit.input.clone())
                })
                .flatten()
        });
        let name: AnyElement = match rename {
            Some(input) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                .child(ui::rename_field(theme, input))
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(query.name.clone())
                .into_any_element(),
        };
        let click_id = query_id.clone();
        let click_key = query.key.clone();
        let app = self.app.clone();
        let menu_app = self.app.clone();
        let menu_id = query_id.clone();
        let menu_key = query.key.clone();
        let file_index = query.index;

        div()
            .id(SharedString::from(query_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(54.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                let click_count = event.click_count();
                let modifiers = event.modifiers();
                let key = click_key.clone();
                let app = app.clone();
                // `hit_query` reads the tree's selected row to scope its list. Running it from
                // inside this row's listener would borrow the tree pane twice (the listener
                // already holds it), which aborts the app; defer one cycle so the event handler
                // has returned.
                cx.defer(move |cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.hit_query(&key, click_count, modifiers, cx);
                        if app.click_opens(click_count, modifiers) {
                            app.open_saved_query(file_index, cx);
                        }
                        cx.notify();
                    });
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.commit_pending_rename(cx);
                    window.focus(&this.focus, cx);
                    this.selected_table = None;
                    this.selected = Some(menu_id.clone());
                    let key = menu_key.clone();
                    let already = menu_app
                        .upgrade()
                        .is_some_and(|app| app.read(cx).query_selection.contains(&key));
                    let position = event.position;
                    let app = menu_app.clone();
                    // Same deferred scope as the click: `hit_query` must not re-enter the tree
                    // entity that is dispatching this mouse-down.
                    cx.defer(move |cx| {
                        let _ = app.update(cx, |app, cx| {
                            if !already {
                                app.hit_query(&key, 1, Modifiers::default(), cx);
                            }
                            app.context_menu = Some(ContextMenu {
                                target: ContextTarget::QueryFile { index: file_index },
                                position,
                            });
                            cx.notify();
                        });
                    });
                    cx.notify();
                }),
            )
            .child(chevron_spacer())
            .child(tree_icon("icons/queries.svg", theme.icon_queries))
            .child(name)
    }

    /// One backup file or saved configuration leaf under the connection tree's Backups category.
    /// It shares its selection and context menu with the Backup main tab's list, so Ctrl/Shift
    /// clicks and the right-click actions operate on the same rows.
    fn render_tree_backup(
        &self,
        connection_index: usize,
        database_index: usize,
        backup: &TreeBackup,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let id = backup_tree_id(connection_index, database_index, backup);
        let selected = self.selected.as_deref() == Some(id.as_str())
            || self
                .app
                .upgrade()
                .is_some_and(|app| app.read(cx).backups_selection.contains(&backup.key));
        let rename = self.app.upgrade().and_then(|app| {
            let app = app.read(cx);
            (app.main_tab != MainTab::Backups)
                .then(|| {
                    app.backup_rename
                        .as_ref()
                        .filter(|edit| match (edit.target, backup.kind) {
                            (BackupSelection::File(index), TreeBackupKind::File)
                            | (BackupSelection::Config(index), TreeBackupKind::Config) => {
                                index == backup.index
                            }
                            _ => false,
                        })
                        .map(|edit| edit.input.clone())
                })
                .flatten()
        });
        let name: AnyElement = match rename {
            Some(input) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                .child(ui::rename_field(theme, input))
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(backup.name.clone())
                .into_any_element(),
        };
        let click_id = id.clone();
        let click_key = backup.key.clone();
        let menu_id = id.clone();
        let menu_key = backup.key.clone();
        let index = backup.index;
        let kind = backup.kind;
        let app = self.app.clone();
        let menu_app = self.app.clone();

        div()
            .id(SharedString::from(id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(54.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                let click_count = event.click_count();
                let modifiers = event.modifiers();
                let key = click_key.clone();
                let app = app.clone();
                // See `render_saved_query`: the hit needs the tree's selected row, so it must run
                // after this row's listener releases the tree pane.
                cx.defer(move |cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.hit_backup(&key, click_count, modifiers, cx);
                        if app.click_opens(click_count, modifiers) {
                            match kind {
                                TreeBackupKind::File => app.open_restore_backup(index, cx),
                                TreeBackupKind::Config => app.open_backup_config(index, cx),
                            }
                        }
                        cx.notify();
                    });
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.commit_pending_rename(cx);
                    window.focus(&this.focus, cx);
                    this.selected_table = None;
                    this.selected = Some(menu_id.clone());
                    let key = menu_key.clone();
                    let already = menu_app
                        .upgrade()
                        .is_some_and(|app| app.read(cx).backups_selection.contains(&key));
                    let position = event.position;
                    let app = menu_app.clone();
                    cx.defer(move |cx| {
                        let _ = app.update(cx, |app, cx| {
                            if !already {
                                app.hit_backup(&key, 1, Modifiers::default(), cx);
                            }
                            app.context_menu = Some(ContextMenu {
                                target: match kind {
                                    TreeBackupKind::File => ContextTarget::BackupFile { index },
                                    TreeBackupKind::Config => ContextTarget::BackupConfig { index },
                                },
                                position,
                            });
                            cx.notify();
                        });
                    });
                    cx.notify();
                }),
            )
            .child(chevron_spacer())
            .child(tree_icon(
                match kind {
                    TreeBackupKind::File => "icons/backups.svg",
                    TreeBackupKind::Config => "icons/save.svg",
                },
                theme.icon_backups,
            ))
            .child(name)
    }

    /// Render one top-level or nested layout entry (a group or a connection).
    fn render_tree_entry(
        &self,
        entry: &GroupEntry,
        connections: &[TreeConnection],
        group_rename: Option<&(u64, Entity<TextInput>)>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        match entry {
            GroupEntry::Group(group) => self.render_group(group, connections, group_rename, cx),
            GroupEntry::Connection(id) => match connections.iter().find(|conn| &conn.id == id) {
                Some(conn) => self.render_connection(conn, cx).into_any_element(),
                None => div().into_any_element(),
            },
        }
    }

    /// One group row of the connection tree, plus its (nested) children when expanded.
    fn render_group(
        &self,
        group: &ConnectionGroup,
        connections: &[TreeConnection],
        group_rename: Option<&(u64, Entity<TextInput>)>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let id = group.id;
        let row_id = format!("group-{id}");
        let click_id = row_id.clone();
        let selected = self.selection.contains(&row_id);
        let renaming = group_rename.filter(|(rid, _)| *rid == id);
        let editing_here = renaming.is_some();

        let label: AnyElement = match renaming {
            Some((_, input)) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                // Keep clicks on the editor from reaching the row/background, so editing does not
                // immediately steal focus away (and the caret can be placed).
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event, _window, cx| cx.stop_propagation()),
                )
                .child(ui::rename_field(theme, input.clone()))
                .into_any_element(),
            None => div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(group.name.clone())
                .into_any_element(),
        };

        let select_app = self.app.clone();
        let toggle_app = self.app.clone();
        let drag_app = self.app.clone();
        let menu_app = self.app.clone();

        let row = div()
            .id(SharedString::from(row_id.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl_1()
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_active_bg))
                    .text_color(rgb(theme.tree_active_text))
            })
            .when(!selected, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if editing_here {
                    return;
                }
                // Releasing a drag of the multi-selection must not collapse it to this row.
                if click_was_drag(event) {
                    return;
                }
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                // Extend/replace the tree's multi-selection from the click's modifiers.
                let visible: Vec<String> = this
                    .visible_ids
                    .iter()
                    .filter(|id| list_ops::is_tree_selectable(id))
                    .cloned()
                    .collect();
                let mode = selection_mode(event.modifiers());
                let shift = event.modifiers().shift;
                let click_count = event.click_count();
                if let Some(app) = select_app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.tree_row_click(&click_id, &visible, mode, shift, cx);
                        if app.click_opens(click_count, event.modifiers()) {
                            app.toggle_group(id, cx);
                        }
                        cx.notify();
                    });
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    if editing_here {
                        return;
                    }
                    if let Some(app) = drag_app.upgrade() {
                        app.update(cx, |app, _| {
                            app.tree_press_start(TreeDragPayload::Group(id), event.position);
                        });
                    }
                    let _ = this;
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    let row_id = format!("group-{id}");
                    let multi = this.app.upgrade().is_some_and(|app| {
                        let app = app.read(cx);
                        app.tree_multi_selected() && app.tree_selection().contains(&row_id)
                    });
                    this.selected = Some(row_id.clone());
                    this.selected_table = None;
                    if let Some(app) = menu_app.upgrade() {
                        app.update(cx, |app, cx| {
                            app.move_submenu = None;
                            if multi {
                                app.context_menu = Some(ContextMenu {
                                    target: ContextTarget::TreeSelection,
                                    position: event.position,
                                });
                            } else {
                                app.tree_selection.select_one(row_id.clone());
                                app.context_menu = Some(ContextMenu {
                                    target: ContextTarget::Group { id },
                                    position: event.position,
                                });
                            }
                            cx.notify();
                        });
                    }
                }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("group-toggle-{id}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |_this, _event, _window, cx| {
                        let _ = toggle_app.update(cx, |app, cx| app.toggle_group(id, cx));
                    }))
                    .child(tree_chevron(group.expanded, theme.chevron)),
            )
            .child(tree_icon("icons/folder.svg", theme.icon_group))
            .child(label);
        let row = tree_row_with_rect(self.app.clone(), row, format!("group-{id}"));
        let toggle_id = row_id.clone();
        let row = self.nav_row(
            row,
            NavRow::new(row_id, group.name.clone()).branch(
                group.expanded,
                Rc::new(move |this, _window, cx| this.activate_tree_id(&toggle_id, cx)),
            ),
        );

        let mut body = div().flex().flex_col().w_full().child(row);
        if group.expanded {
            let children = self.nested(|| {
                let mut children = div().flex().flex_col().w_full().pl(px(12.0));
                for child in &group.children {
                    children = children.child(self.render_tree_entry(
                        child,
                        connections,
                        group_rename,
                        cx,
                    ));
                }
                children
            });
            body = body.child(children);
        }
        body.into_any_element()
    }
}

impl Render for TreePane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (theme, connections, layout, rename, group_rename, group_renaming, tree_selection) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            (
                app.theme,
                snapshot_connections(app),
                app.tree_layout.clone(),
                app.rename_row(RowPane::Tree),
                app.group_rename
                    .as_ref()
                    .map(|edit| (edit.id, edit.input.clone())),
                app.group_rename.is_some(),
                app.tree_selection.items(),
            )
        };
        self.theme = theme;
        self.rename_row = rename;
        self.group_renaming = group_renaming;
        // A database/schema/object click moves the single selection off the connections: the
        // connection/group multi-selection must not stay painted behind it and make two rows look
        // selected. (Keyboard navigation already clears the multi-selection the same way.)
        let moved_off_connections = self
            .selected
            .as_deref()
            .is_some_and(|id| !list_ops::is_tree_selectable(id));
        self.selection = if moved_off_connections {
            BTreeSet::new()
        } else {
            tree_selection.into_iter().collect()
        };

        // The row renderers record every drawn row here, in draw order (see `nav_row`).
        self.nav_build.borrow_mut().clear();
        self.nav_depth.set(0);

        let mut list = div().flex().flex_col().w_full();
        if layout.is_empty() {
            list = list.child(
                div()
                    .w_full()
                    .p_2()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("sidebar.no_connections").to_string()),
            );
        }
        for entry in &layout {
            list =
                list.child(self.render_tree_entry(entry, &connections, group_rename.as_ref(), cx));
        }
        self.nav_rows = std::mem::take(&mut *self.nav_build.borrow_mut());
        self.visible_ids = self.nav_rows.iter().map(|row| row.id.clone()).collect();
        // Rows that are no longer drawn must not keep stale bounds.
        let drawn: BTreeSet<&str> = self.nav_rows.iter().map(|row| row.id.as_str()).collect();
        self.row_bounds
            .borrow_mut()
            .retain(|id, _| drawn.contains(id.as_str()));

        // The drag hit-test reads last frame's rectangles; drop them now so the rows can republish
        // them during this frame's paint. The drop indicator itself is drawn by `AppView` (which
        // re-renders on every drag move), not here.
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| app.clear_tree_row_rects());
        }

        let container = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .min_h(px(0.0))
            .child(
                div()
                    .id("sidebar-scroll")
                    .key_context(TREE_CONTEXT)
                    .flex()
                    .flex_col()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .py_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .track_focus(&self.focus)
                    // Keyboard navigation: keymap commands, rebindable from Options → Shortcuts.
                    .on_action(cx.listener(|this, _: &TreeUp, _window, cx| this.nav_move(-1, cx)))
                    .on_action(cx.listener(|this, _: &TreeDown, _window, cx| this.nav_move(1, cx)))
                    .on_action(cx.listener(|this, _: &TreePageUp, _window, cx| {
                        let page = this.nav_page();
                        this.nav_move(-page, cx);
                    }))
                    .on_action(cx.listener(|this, _: &TreePageDown, _window, cx| {
                        let page = this.nav_page();
                        this.nav_move(page, cx);
                    }))
                    .on_action(cx.listener(|this, _: &TreeHome, _window, cx| {
                        if !this.nav_rows.is_empty() {
                            this.nav_select(0, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &TreeEnd, _window, cx| {
                        if let Some(last) = this.nav_rows.len().checked_sub(1) {
                            this.nav_select(last, cx);
                        }
                    }))
                    .on_action(
                        cx.listener(|this, _: &TreeExpand, window, cx| this.nav_expand(window, cx)),
                    )
                    .on_action(cx.listener(|this, _: &TreeCollapse, window, cx| {
                        this.nav_collapse(window, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &TreeOpen, window, cx| this.nav_open(window, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &TreeRename, window, cx| {
                            this.tree_rename(window, cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &TreeSelectAll, _window, cx| {
                            this.tree_select_all(cx)
                        }),
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.tree_key(event, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            // Clicking outside the in-place rename commits it (the editor swallows
                            // clicks on itself), then the tree takes focus so F2 works.
                            this.commit_pending_rename(cx);
                            window.focus(&this.focus, cx);
                            // Start a rubber band unless the press lands on a connection/group row,
                            // which starts a drag-reorder instead.
                            if let Some(app) = this.app.upgrade() {
                                let on_row = app.read(cx).tree_press_on_reorder_row(event.position);
                                if !on_row {
                                    app.update(cx, |app, cx| {
                                        app.begin_tree_marquee(event.position, event.modifiers, cx);
                                    });
                                }
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            // Only the blank area reaches here: every row's right-click handler
                            // stops propagation.
                            window.focus(&this.focus, cx);
                            this.selected = None;
                            this.selected_table = None;
                            if let Some(app) = this.app.upgrade() {
                                app.update(cx, |app, cx| {
                                    app.tree_selection.clear();
                                    app.dump_submenu = None;
                                    app.move_submenu = None;
                                    app.color_submenu = None;
                                    app.context_menu = Some(ContextMenu {
                                        target: ContextTarget::TreeBlank,
                                        position: event.position,
                                    });
                                    cx.notify();
                                });
                            }
                        }),
                    )
                    .child(list),
            )
            // gpui does not paint a scrollbar for an overflow container; this one can be dragged,
            // and clicking its track jumps there.
            .child(
                Scrollbar::vertical(&self.scroll)
                    .mode(ScrollbarMode::Scrolling)
                    .id("sidebar-scrollbar"),
            );
        let app = self.app.clone();
        div()
            .w_full()
            .h_full()
            .on_children_prepainted(move |bounds, _window, cx| {
                let Some(rect) = bounds.first().copied() else {
                    return;
                };
                let _ = app.update(cx, |app, _| app.note_tree_bounds(rect));
            })
            .child(container)
            .into_any_element()
    }
}

/// Whether a click event was actually a drag (press and release far apart), so a row drag of the
/// multi-selection does not collapse the selection on release.
fn click_was_drag(event: &ClickEvent) -> bool {
    let ClickEvent::Mouse(mouse) = event else {
        return false;
    };
    let dx = f32::from(mouse.up.position.x) - f32::from(mouse.down.position.x);
    let dy = f32::from(mouse.up.position.y) - f32::from(mouse.down.position.y);
    dx.hypot(dy) > 4.0
}

/// Wrap a connection-tree row so it publishes its window-space rectangle for the drag hit test.
fn tree_row_with_rect(
    app: WeakEntity<AppView>,
    row: impl IntoElement + 'static,
    key: String,
) -> AnyElement {
    div()
        .on_children_prepainted(move |bounds, _window, cx| {
            let Some(rect) = bounds.first().copied() else {
                return;
            };
            let _ = app.update(cx, |app, _| app.note_tree_row_rect(key.clone(), rect));
        })
        .child(row)
        .into_any_element()
}

/// Draw the drag-and-drop indicator (a 2px line or a group highlight) in window coordinates.
pub(super) fn tree_drop_indicator_element(hint: groups::TreeDropHint, theme: Theme) -> AnyElement {
    let element = match hint {
        groups::TreeDropHint::Line(bounds) => div()
            .absolute()
            .left(bounds.left())
            .top(bounds.top())
            .w(bounds.size.width)
            .h(px(2.0))
            .bg(rgb(theme.brand))
            .into_any_element(),
        groups::TreeDropHint::Highlight(bounds) => div()
            .absolute()
            .left(bounds.left())
            .top(bounds.top())
            .w(bounds.size.width)
            .h(bounds.size.height)
            .rounded_sm()
            .border_1()
            .border_color(rgb(theme.brand))
            .bg(rgba(0x3b82f622))
            .into_any_element(),
    };
    deferred(element).with_priority(60).into_any_element()
}

/// Draw the connection tree's rubber-band selection rectangle in window coordinates (like the
/// drop indicator, which `AppView` renders above the panes so it can track the pointer).
pub(super) fn tree_marquee_element(
    origin: Point<Pixels>,
    current: Point<Pixels>,
    bounds: Bounds<Pixels>,
    theme: Theme,
) -> AnyElement {
    let left = origin.x.min(current.x).max(bounds.left());
    let top = origin.y.min(current.y).max(bounds.top());
    let right = origin.x.max(current.x).min(bounds.right());
    let bottom = origin.y.max(current.y).min(bounds.bottom());
    let width = (right - left).max(px(0.0));
    let height = (bottom - top).max(px(0.0));
    if width <= px(0.0) || height <= px(0.0) {
        return div().into_any_element();
    }
    deferred(
        div()
            .absolute()
            .left(left)
            .top(top)
            .w(width)
            .h(height)
            .rounded(px(2.0))
            .bg(rgba((theme.brand << 8) | 0x22))
            .border_1()
            .border_color(rgb(theme.brand)),
    )
    .with_priority(61)
    .into_any_element()
}

#[cfg(test)]
mod nav_tests {
    use super::{nav_parent, typeahead_target};

    #[test]
    fn a_rows_parent_is_the_nearest_shallower_row_above_it() {
        // conn, db, Tables, t1, t2, Views, db2
        let depths = [0, 1, 2, 3, 3, 2, 1];
        assert_eq!(nav_parent(&depths, 0), None);
        assert_eq!(nav_parent(&depths, 3), Some(2));
        assert_eq!(nav_parent(&depths, 4), Some(2));
        assert_eq!(nav_parent(&depths, 5), Some(1));
        assert_eq!(nav_parent(&depths, 6), Some(0));
    }

    #[test]
    fn typing_jumps_to_the_next_matching_row_and_repeating_a_letter_cycles() {
        let labels = ["Tables", "customers", "orders", "Customer_notes", "users"];
        assert_eq!(typeahead_target(&labels, None, "c"), Some(1));
        assert_eq!(typeahead_target(&labels, Some(1), "c"), Some(3));
        assert_eq!(typeahead_target(&labels, Some(3), "c"), Some(1));
        // A longer name may stay on the current row.
        assert_eq!(typeahead_target(&labels, Some(1), "cust"), Some(1));
        assert_eq!(typeahead_target(&labels, Some(1), "customer_"), Some(3));
        assert_eq!(typeahead_target(&labels, Some(0), "zz"), None);
        assert_eq!(typeahead_target(&[], None, "a"), None);
    }
}

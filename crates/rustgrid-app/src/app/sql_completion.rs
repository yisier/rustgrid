//! The SQL completion provider for gpui-kit's `Editor`.
//!
//! The provider is a thin adapter: it snapshots the caret's statement with `sql::completion_context`
//! (the same analysis the hand-rolled popup used) and turns the resulting candidates into LSP
//! `CompletionItem`s. It reads the connection catalog (tables, columns, functions) from a
//! [`CompletionSource`] the app keeps refreshed.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, Task, Window};
use gpui_kit::component::input::CompletionProvider;
use gpui_kit::component::input::RopeExt as _;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range, TextEdit,
};
use ropey::Rope;
use rustgrid_core::DriverDialect;

use crate::sql;

/// One column in the catalog: its name, type and comment.
pub(crate) type CatalogColumn = (String, String, String);

/// The catalog completion draws on, keyed by connection index. `AppView` replaces this snapshot
/// whenever the loaded catalogs change.
#[derive(Default)]
pub(crate) struct CompletionCatalog {
    /// `(connection index, database)` → table names.
    pub tables: HashMap<(usize, String), Vec<String>>,
    /// `(connection index, database, table)` → columns.
    pub columns: HashMap<(usize, String, String), Vec<CatalogColumn>>,
    /// Connection index → stored function names.
    pub functions: HashMap<usize, Vec<String>>,
}

/// The connection / database / schema the active editor's completion and column lookups target.
#[derive(Clone, Default)]
pub(crate) struct CompletionScope {
    pub connection_index: Option<usize>,
    pub database: Option<String>,
    /// The selected schema (SQL Server); `None` leaves unqualified objects to the engine default.
    pub schema: Option<String>,
    /// Whether the target engine namespaces objects by schema (SQL Server). When true a referenced
    /// table's first qualifier is a schema, not a database.
    pub supports_schemas: bool,
    /// The connection's SQL dialect, used to pick the built-in function list so an engine is not
    /// offered another's functions.
    pub dialect: DriverDialect,
    /// Whether the target is a key-value engine (Redis): completion offers Redis commands and
    /// keys instead of SQL keywords, tables and fields.
    pub key_value: bool,
}

/// A shared catalog and the scope the active editor targets. The scope is behind an `Arc` so a
/// live provider can be re-scoped when the toolbar's connection/database/schema changes.
#[derive(Clone)]
pub(crate) struct CompletionSource {
    catalog: Arc<std::sync::RwLock<CompletionCatalog>>,
    scope: Arc<std::sync::RwLock<CompletionScope>>,
    /// Starts loading a table's columns into the catalog: `(connection, database, table)`.
    load_columns: Option<ColumnLoader>,
    /// The column targets a popup has already waited for, `(connection, database, table)`: a load
    /// that failed or was too slow must not cost every later popup the whole [`COLUMN_WAIT`] again.
    waited: Arc<std::sync::Mutex<std::collections::HashSet<(usize, String, String)>>>,
}

/// Asks the app to load a table's columns into the completion catalog.
pub(crate) type ColumnLoader = Rc<dyn Fn(&mut App, usize, String, String)>;

/// How long the popup waits for the columns of the statement's tables to arrive before it opens
/// without them (they are usually there within one round trip).
const COLUMN_WAIT: Duration = Duration::from_millis(1200);

impl CompletionSource {
    pub(crate) fn new(catalog: Arc<std::sync::RwLock<CompletionCatalog>>) -> Self {
        Self {
            catalog,
            scope: Arc::new(std::sync::RwLock::new(CompletionScope::default())),
            load_columns: None,
            waited: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// Let the provider request the columns it is missing (see [`ColumnLoader`]).
    pub(crate) fn with_column_loader(mut self, loader: ColumnLoader) -> Self {
        self.load_columns = Some(loader);
        self
    }

    /// The `(database, table)` catalog keys of the columns `context` would offer.
    fn column_targets(&self, context: &sql::SqlCompletionContext) -> Vec<(String, String)> {
        let scope = self.scope.read().expect("completion scope lock");
        match context.qualifier.last() {
            Some(qualifier) => resolve_qualified_table(context, qualifier)
                .and_then(|table| catalog_target(&scope, table))
                .into_iter()
                .collect(),
            None => context
                .tables
                .iter()
                .filter_map(|table| catalog_target(&scope, table))
                .collect(),
        }
    }

    /// The column targets whose columns the catalog does not hold yet.
    fn missing_columns(&self, targets: &[(String, String)]) -> Vec<(String, String)> {
        let Some(connection) = self
            .scope
            .read()
            .expect("completion scope lock")
            .connection_index
        else {
            return Vec::new();
        };
        let catalog = self.catalog.read().expect("completion catalog lock");
        targets
            .iter()
            .filter(|(database, table)| {
                !catalog
                    .columns
                    .contains_key(&(connection, database.clone(), table.clone()))
            })
            .cloned()
            .collect()
    }

    /// The targets of `missing` a popup has not waited for yet, recorded as waited for: the wait
    /// is worth it once per table, not on every keystroke.
    fn take_unwaited(
        &self,
        connection: usize,
        missing: &[(String, String)],
    ) -> Vec<(String, String)> {
        let mut waited = self.waited.lock().expect("completion wait lock");
        missing
            .iter()
            .filter(|(database, table)| {
                waited.insert((connection, database.clone(), table.clone()))
            })
            .cloned()
            .collect()
    }

    pub(crate) fn with_scope(
        self,
        connection_index: Option<usize>,
        database: Option<String>,
        schema: Option<String>,
        supports_schemas: bool,
        dialect: DriverDialect,
        key_value: bool,
    ) -> Self {
        *self.scope.write().expect("completion scope lock") = CompletionScope {
            connection_index,
            database,
            schema,
            supports_schemas,
            dialect,
            key_value,
        };
        self
    }

    /// The shared scope, so the editor wrapper can re-scope a live provider.
    pub(crate) fn scope_handle(&self) -> Arc<std::sync::RwLock<CompletionScope>> {
        self.scope.clone()
    }
}

/// Resolve a referenced table to the catalog's `(database, table-key)` coordinates. On schema
/// engines the reference's first qualifier is a schema (not a database), and the catalog keys
/// tables as `schema.table`; elsewhere the qualifier is a database and the key is the bare name.
pub(crate) fn catalog_target(
    scope: &CompletionScope,
    table: &sql::SqlTableRef,
) -> Option<(String, String)> {
    let qualifier = table.database.as_deref().filter(|name| !name.is_empty());
    if scope.supports_schemas {
        let database = scope.database.clone()?;
        let schema = qualifier
            .map(str::to_string)
            .or_else(|| scope.schema.clone());
        let key = match schema {
            Some(schema) => format!("{schema}.{}", table.name),
            None => table.name.clone(),
        };
        Some((database, key))
    } else {
        let database = qualifier
            .map(str::to_string)
            .or_else(|| scope.database.clone())?;
        Some((database, table.name.clone()))
    }
}

/// The gpui-kit completion provider over a [`CompletionSource`].
pub(crate) struct SqlCompletionProvider {
    source: CompletionSource,
}

impl SqlCompletionProvider {
    pub(crate) fn new(source: CompletionSource) -> Self {
        Self { source }
    }
}

impl CompletionProvider for SqlCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let key_value = self
            .source
            .scope
            .read()
            .expect("completion scope lock")
            .key_value;
        if key_value {
            return Task::ready(Ok(redis_completion(&self.source, text, offset)));
        }
        let sql_text = text.to_string();
        let dialect = self
            .source
            .scope
            .read()
            .expect("completion scope lock")
            .dialect;
        let context = sql::completion_context(&sql_text, offset, dialect);
        // Anchor the replacement range explicitly. gpui's completion menu derives it from the
        // offset of the *first* keystroke that opened the menu (`trigger_start_offset`), so
        // accepting "SELECT" after typing "sele" inserts at the wrong place ("seSELECT"). An
        // explicit `text_edit` overrides that range with the real identifier fragment
        // (`prefix_start..offset`).
        let range = replacement_range(text, &context, offset);
        let respond = move |source: &CompletionSource, context: &sql::SqlCompletionContext| {
            let mut items = build_items(source, context);
            for item in &mut items {
                item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: item.label.clone(),
                }));
            }
            CompletionResponse::Array(items)
        };

        // Where columns belong, wait (briefly) for the statement's tables' columns instead of
        // opening on table names: the first popup after naming a table is the one that matters.
        let missing = if context.suppress || context.kind == sql::SqlCompletionKind::Table {
            Vec::new()
        } else {
            self.source
                .missing_columns(&self.source.column_targets(&context))
        };
        let connection = self
            .source
            .scope
            .read()
            .expect("completion scope lock")
            .connection_index;
        let (Some(loader), Some(connection)) = (self.source.load_columns.clone(), connection)
        else {
            return Task::ready(Ok(respond(&self.source, &context)));
        };
        if missing.is_empty() {
            return Task::ready(Ok(respond(&self.source, &context)));
        }
        for (database, table) in missing.clone() {
            let loader = loader.clone();
            // Deferred: the app may be mid-update while the editor asks for completions.
            cx.defer(move |cx| loader(cx, connection, database, table));
        }
        // Wait for a target only the first time it is asked for: a load that failed or was too slow
        // then does not delay every later popup by the whole COLUMN_WAIT.
        let waiting = self.source.take_unwaited(connection, &missing);
        if waiting.is_empty() {
            return Task::ready(Ok(respond(&self.source, &context)));
        }
        let source = self.source.clone();
        cx.spawn(async move |cx| {
            let started = std::time::Instant::now();
            while started.elapsed() < COLUMN_WAIT && !source.missing_columns(&waiting).is_empty() {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
            }
            Ok(respond(&source, &context))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        // Open the menu on an identifier character or a `.` qualifier; the provider then filters by
        // the typed prefix. A Redis key may also contain `:`, `-` and `*` (namespace separators and
        // patterns), so those open it too.
        let key_value = self
            .source
            .scope
            .read()
            .expect("completion scope lock")
            .key_value;
        new_text.chars().all(|character| {
            character.is_alphanumeric()
                || matches!(character, '_' | '.')
                || (key_value && matches!(character, ':' | '-' | '*'))
        })
    }
}

/// The completion groups, in the order the popup lists them: the fields of the statement at hand,
/// then the language's everyday words, then the user's own tables — Navicat's order, where the most
/// used keyword leads the table names — then the rest of the language.
const GROUP_COLUMNS: u8 = 0;
const GROUP_COMMON_KEYWORDS: u8 = 1;
const GROUP_TABLES: u8 = 2;
const GROUP_KEYWORDS: u8 = 3;
const GROUP_FUNCTIONS: u8 = 4;
/// Where a column is expected: the tables and aliases the statement names (to qualify a column).
const GROUP_SCOPE: u8 = 5;
/// Where a column is expected: every other table of the catalog, after everything else.
const GROUP_OTHER_TABLES: u8 = 6;

/// The rank of a group in the popup. Where a column is expected (`SELECT |`, `WHERE |`, ...), the
/// statement's columns and its own tables / aliases come first and the rest of the catalog's tables
/// last; where a table is expected (`FROM |`), Navicat's order holds.
fn group_rank(group: u8, column_position: bool) -> u8 {
    if !column_position {
        return group;
    }
    match group {
        GROUP_COLUMNS => 0,
        GROUP_SCOPE => 1,
        GROUP_COMMON_KEYWORDS => 2,
        GROUP_FUNCTIONS => 3,
        GROUP_KEYWORDS => 4,
        _ => 5,
    }
}

/// Whether a group's candidates keep the order they were built in instead of being re-sorted by
/// length and then alphabetically: the table's own field order for columns, and the curated usage
/// order of [`sql::COMMON_KEYWORDS`] for the everyday keywords.
fn keeps_build_order(group: u8) -> bool {
    matches!(group, GROUP_COLUMNS | GROUP_COMMON_KEYWORDS | GROUP_SCOPE)
}

/// Build the completion items for a caret context, ordering columns → everyday keywords → tables →
/// other keywords → functions and de-duplicating by `(label, detail)`.
///
/// A candidate matches by what the user typed anywhere in its name — exactly, as a prefix, inside
/// it or as a loose subsequence ([`match_quality`]) — so `as` finds the table `wms_asn` and `wsn`
/// still does. The closest matches lead; at equal quality the order above holds, which is Navicat's:
/// the everyday keyword the caret is most likely asking for (`SELECT` for `se`) leads the user's
/// table names, unless a field of the statement's own tables matches.
///
/// Under a selected schema a table/function is offered by its bare name, so the typed fragment
/// matches and accepting it does not append the schema.
fn build_items(
    source: &CompletionSource,
    context: &sql::SqlCompletionContext,
) -> Vec<CompletionItem> {
    if context.suppress {
        return Vec::new();
    }
    let catalog = source.catalog.read().expect("completion catalog lock");
    let scope = source.scope.read().expect("completion scope lock");
    let connection = scope.connection_index;
    let qualifier = context.qualifier.last().map(String::as_str);

    let kind = if context.kind == sql::SqlCompletionKind::Table {
        CompletionItemKind::CLASS
    } else {
        CompletionItemKind::FIELD
    };

    let mut items: Vec<(u8, CompletionItem)> = Vec::new();

    // 1. Columns: from the statement's referenced tables, or the qualifier's table.
    let column_targets: Vec<(String, String)> = if let Some(qualifier) = qualifier {
        resolve_qualified_table(context, qualifier)
            .and_then(|table| catalog_target(&scope, table))
            .into_iter()
            .collect()
    } else {
        context
            .tables
            .iter()
            .filter_map(|table| catalog_target(&scope, table))
            .collect()
    };
    if let Some(connection) = connection {
        for (database, table) in &column_targets {
            if let Some(columns) =
                catalog
                    .columns
                    .get(&(connection, database.clone(), table.clone()))
            {
                for (name, data_type, comment) in columns {
                    let detail = if comment.is_empty() {
                        format!("{table} [{data_type}]")
                    } else {
                        format!("{table} [{data_type}]  {comment}")
                    };
                    items.push((GROUP_COLUMNS, completion_item(name, &detail, kind)));
                }
            }
        }
    }

    // Where a column is expected and the statement names tables: offer those tables' aliases
    // and names (to qualify a column) right after the columns.
    let column_position = qualifier.is_none()
        && context.kind == sql::SqlCompletionKind::Column
        && !context.tables.is_empty();
    if column_position {
        for table in &context.tables {
            if let Some(alias) = &table.alias {
                items.push((
                    GROUP_SCOPE,
                    completion_item(alias, &table.name, CompletionItemKind::VARIABLE),
                ));
            }
            items.push((
                GROUP_SCOPE,
                completion_item(&table.name, "table", CompletionItemKind::CLASS),
            ));
        }
    }

    // 2. Tables: all loaded tables, or the qualifier database's tables. With a schema selected,
    // only that schema's tables are offered (the schema selector's filtering).
    let table_kind = CompletionItemKind::CLASS;
    let tables_group = if column_position {
        GROUP_OTHER_TABLES
    } else {
        GROUP_TABLES
    };
    if let Some(connection) = connection {
        let mut tables: Vec<(String, String)> = Vec::new();
        for ((conn, database), names) in &catalog.tables {
            if *conn != connection {
                continue;
            }
            if let Some(qualifier) = qualifier
                && !database.eq_ignore_ascii_case(qualifier)
            {
                continue;
            }
            for name in names {
                if qualifier.is_none()
                    && let Some(schema) = &scope.schema
                    && !name_in_schema(name, schema)
                {
                    continue;
                }
                tables.push((name.clone(), database.clone()));
            }
        }
        tables.sort();
        tables.dedup();
        for (name, database) in tables {
            let label = display_name(&name, scope.schema.as_deref());
            items.push((tables_group, completion_item(&label, &database, table_kind)));
        }
    }

    // 3. Keywords and 4. functions: built-ins plus the connection's stored functions. The everyday
    // keywords are offered first, in their curated (most-typed first) order, so a shared prefix
    // leads with the word in every statement; the rest of the language's keywords follow, and
    // functions last.
    if qualifier.is_none() {
        for keyword in sql::COMMON_KEYWORDS {
            items.push((
                GROUP_COMMON_KEYWORDS,
                completion_item(keyword, "keyword", CompletionItemKind::KEYWORD),
            ));
        }
        for keyword in sql::keywords() {
            if sql::is_common_keyword(keyword) {
                continue;
            }
            items.push((
                GROUP_KEYWORDS,
                completion_item(keyword, "keyword", CompletionItemKind::KEYWORD),
            ));
        }
        for function in sql::functions_for(scope.dialect).iter().copied() {
            items.push((
                GROUP_FUNCTIONS,
                completion_item(function, "function", CompletionItemKind::FUNCTION),
            ));
        }
        if let Some(connection) = connection
            && let Some(functions) = catalog.functions.get(&connection)
        {
            for function in functions {
                if let Some(schema) = &scope.schema
                    && !name_in_schema(function, schema)
                {
                    continue;
                }
                let label = display_name(function, scope.schema.as_deref());
                items.push((
                    GROUP_FUNCTIONS,
                    completion_item(&label, "function", CompletionItemKind::FUNCTION),
                ));
            }
        }
    }

    rank_items(items, &context.prefix, column_position)
}

/// Filter `items` by the typed `prefix`, de-duplicate, rank them and keep the popup's 64 closest
/// candidates. Shared by the SQL and Redis completion paths.
fn rank_items(
    items: Vec<(u8, CompletionItem)>,
    prefix: &str,
    column_position: bool,
) -> Vec<CompletionItem> {
    let mut seen = std::collections::HashSet::new();
    let mut filtered: Vec<(Match, u8, CompletionItem)> = items
        .into_iter()
        .filter_map(|(group, item)| Some((match_quality(&item.label, prefix)?, group, item)))
        .filter(|(_, _, item)| {
            seen.insert((
                item.label.to_lowercase(),
                item.detail.clone().unwrap_or_default(),
            ))
        })
        .collect();
    filtered.sort_by(|a, b| {
        // The closest match leads (a whole-word or prefix match before a match inside the name),
        // and at equal closeness the group decides — the everyday keyword before the table names
        // (Navicat's order), and where a column is expected the statement's columns first.
        a.0.tier()
            .cmp(&b.0.tier())
            .then_with(|| group_rank(a.1, column_position).cmp(&group_rank(b.1, column_position)))
            .then_with(|| a.0.cmp(&b.0))
            // Within a group the candidate closest to what was typed leads: `fr` offers `FREE`
            // before `FREEZE` before `FRAME_ROW`. Columns keep the table's own field order and the
            // everyday keywords their curated usage order, so those two groups are left as built.
            .then_with(|| {
                if keeps_build_order(a.1) {
                    std::cmp::Ordering::Equal
                } else {
                    a.2.label.chars().count().cmp(&b.2.label.chars().count())
                }
            })
            .then_with(|| {
                if keeps_build_order(a.1) {
                    std::cmp::Ordering::Equal
                } else {
                    a.2.label.to_lowercase().cmp(&b.2.label.to_lowercase())
                }
            })
    });
    filtered.truncate(64);
    filtered.into_iter().map(|(_, _, item)| item).collect()
}

/// Build the response for a key-value (Redis) editor: commands plus the current database's keys.
fn redis_completion(source: &CompletionSource, text: &Rope, offset: usize) -> CompletionResponse {
    let Some((prefix, prefix_start)) = redis_prefix(&text.to_string(), offset) else {
        return CompletionResponse::Array(Vec::new());
    };
    let range = Range::new(
        text.offset_to_position(prefix_start),
        text.offset_to_position(offset),
    );
    let mut items = redis_items(source, &prefix);
    for item in &mut items {
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: item.label.clone(),
        }));
    }
    CompletionResponse::Array(items)
}

/// The identifier fragment just before `caret` in a Redis command line, or `None` inside a
/// quoted argument / on a `#` comment line (where a command popup would be noise). Redis keys
/// commonly contain `:`, `-` and `*`, so those stay part of the fragment.
fn redis_prefix(text: &str, caret: usize) -> Option<(String, usize)> {
    let caret = caret.min(text.len());
    let line_start = text[..caret]
        .rfind('\n')
        .map(|offset| offset + 1)
        .unwrap_or(0);
    let line = &text[line_start..caret];
    if line.trim_start().starts_with('#') {
        return None;
    }
    let mut quote = None;
    let mut escaped = false;
    for character in line.chars() {
        match quote {
            Some(active) => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == active {
                    quote = None;
                }
            }
            None => {
                if matches!(character, '"' | '\'') {
                    quote = Some(character);
                }
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    let mut start = caret;
    for (index, character) in text[..caret].char_indices().rev() {
        if character.is_alphanumeric() || matches!(character, '_' | '.' | ':' | '-' | '*') {
            start = index;
        } else {
            break;
        }
    }
    Some((text[start..caret].to_lowercase(), start))
}

/// SQL keywords and tables are replaced by Redis commands and keys when the connection is a
/// key-value engine.
fn redis_items(source: &CompletionSource, prefix: &str) -> Vec<CompletionItem> {
    let catalog = source.catalog.read().expect("completion catalog lock");
    let scope = source.scope.read().expect("completion scope lock");
    let mut items: Vec<(u8, CompletionItem)> = Vec::new();
    for command in crate::app::redis_lex::COMMANDS {
        items.push((
            GROUP_COMMON_KEYWORDS,
            completion_item(command, "command", CompletionItemKind::KEYWORD),
        ));
    }
    if let Some(connection) = scope.connection_index {
        let mut keys = std::collections::BTreeSet::new();
        match &scope.database {
            Some(database) => {
                if let Some(names) = catalog.tables.get(&(connection, database.clone())) {
                    keys.extend(names.iter().cloned());
                }
            }
            None => {
                for ((candidate, _), names) in &catalog.tables {
                    if *candidate == connection {
                        keys.extend(names.iter().cloned());
                    }
                }
            }
        }
        for key in keys {
            items.push((
                GROUP_TABLES,
                completion_item(&key, "key", CompletionItemKind::CLASS),
            ));
        }
    }
    rank_items(items, prefix, false)
}

/// How a candidate's label matched the typed fragment, best first: the popup lists the closest
/// matches above the loosest ones.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Match {
    /// The label is exactly what was typed.
    Exact,
    /// The label starts with what was typed (`users` for `use`).
    Prefix,
    /// The typed fragment appears inside the label (`wms_asn` for `as`).
    Substring,
    /// The typed characters appear in order but apart (`wms_asn` for `wsn`).
    Subsequence,
}

impl Match {
    /// Exact and prefix matches rank together (the group then decides), above a match inside the
    /// name, above a loose subsequence.
    fn tier(self) -> u8 {
        match self {
            Match::Exact | Match::Prefix => 0,
            Match::Substring => 1,
            Match::Subsequence => 2,
        }
    }
}

/// How `label` matches the already-lowercased `fragment`, or `None` when it does not match at all.
/// Table and column names rarely start with what the user remembers about them (`wms_asn` for
/// `asn`), so anything but the first fragment character may also appear inside the name.
fn match_quality(label: &str, fragment: &str) -> Option<Match> {
    let label = label.to_lowercase();
    if fragment.is_empty() {
        return Some(Match::Prefix);
    }
    if label == fragment {
        return Some(Match::Exact);
    }
    if label.starts_with(fragment) {
        return Some(Match::Prefix);
    }
    if label.contains(fragment) {
        return Some(Match::Substring);
    }
    let mut label = label.chars();
    fragment
        .chars()
        .all(|wanted| label.by_ref().any(|found| found == wanted))
        .then_some(Match::Subsequence)
}

/// The completion label for a possibly schema-qualified object name. Under a selected schema the
/// bare name is shown (and inserted), so accepting a suggestion does not append the schema.
fn display_name(name: &str, schema: Option<&str>) -> String {
    if let Some(schema) = schema
        && let Some((prefix, object)) = name.split_once('.')
        && !prefix.is_empty()
        && !object.is_empty()
        && prefix.eq_ignore_ascii_case(schema)
    {
        return object.to_string();
    }
    name.to_string()
}

/// Resolve a single-part qualifier (`alias` or `table` or `database`) to its referenced table.
fn resolve_qualified_table<'a>(
    context: &'a sql::SqlCompletionContext,
    qualifier: &str,
) -> Option<&'a sql::SqlTableRef> {
    context.tables.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case(qualifier))
            || table.name.eq_ignore_ascii_case(qualifier)
    })
}

/// Whether a (possibly schema-qualified) catalog object name belongs to `schema`.
fn name_in_schema(name: &str, schema: &str) -> bool {
    name.split_once('.').is_some_and(|(prefix, object)| {
        !prefix.is_empty() && !object.is_empty() && prefix.eq_ignore_ascii_case(schema)
    })
}

fn completion_item(label: &str, detail: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        label_details: None,
        kind: Some(kind),
        detail: Some(detail.to_string()),
        ..Default::default()
    }
}

/// The range (in gpui's char-column positions) that accepting a completion must replace: the
/// identifier fragment from `context.prefix_start` up to the caret offset.
fn replacement_range(text: &Rope, context: &sql::SqlCompletionContext, offset: usize) -> Range {
    Range::new(
        text.offset_to_position(context.prefix_start),
        text.offset_to_position(offset),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_range_covers_the_typed_identifier() {
        let sql = "SELECT * FROM ams";
        let context = crate::sql::completion_context(sql, sql.len(), DriverDialect::Mysql);
        let range = replacement_range(&Rope::from(sql), &context, sql.len());
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, "SELECT * FROM ".len() as u32);
        assert_eq!(range.end.character, sql.len() as u32);
    }

    #[test]
    fn replacement_range_is_empty_at_a_fresh_dot() {
        let sql = "SELECT t.";
        let context = crate::sql::completion_context(sql, sql.len(), DriverDialect::Mysql);
        let range = replacement_range(&Rope::from(sql), &context, sql.len());
        assert_eq!(range.start, range.end);
        assert_eq!(range.end.character, sql.len() as u32);
    }

    #[test]
    fn redis_prefix_keeps_namespace_colons_and_skips_strings_and_comments() {
        let sql = "GET sys_dict:us";
        assert_eq!(
            redis_prefix(sql, sql.len()),
            Some(("sys_dict:us".to_string(), 4))
        );
        assert_eq!(redis_prefix("GET \"sys_dict:us\"", 16), None);
        assert_eq!(redis_prefix("# GET us", 8), None);
        assert_eq!(
            redis_prefix("JSON.GET", 8),
            Some(("json.get".to_string(), 0))
        );
    }

    fn redis_source(keys: &[&str]) -> CompletionSource {
        let mut catalog = CompletionCatalog::default();
        catalog.tables.insert(
            (0, "db0".to_string()),
            keys.iter().map(|key| key.to_string()).collect(),
        );
        CompletionSource::new(Arc::new(std::sync::RwLock::new(catalog))).with_scope(
            Some(0),
            Some("db0".into()),
            None,
            false,
            DriverDialect::Generic,
            true,
        )
    }

    #[test]
    fn redis_completion_offers_commands_and_keys() {
        let source = redis_source(&["user:1", "user:2"]);
        let response = redis_completion(&source, &Rope::from("GE"), 2);
        let CompletionResponse::Array(items) = response else {
            panic!("expected an array response");
        };
        assert_eq!(items.first().map(|item| item.label.as_str()), Some("GET"));
        let edit = match items
            .iter()
            .find(|item| item.label == "GET")
            .and_then(|item| item.text_edit.as_ref())
        {
            Some(CompletionTextEdit::Edit(edit)) => edit,
            other => panic!("expected an explicit text edit, got {other:?}"),
        };
        assert_eq!(edit.new_text, "GET");
        assert_eq!(edit.range.start.line, 0);
        assert_eq!(edit.range.start.character, 0);
        assert_eq!(edit.range.end.character, 2);

        let response = redis_completion(&source, &Rope::from("GET us"), 6);
        let CompletionResponse::Array(items) = response else {
            panic!("expected an array response");
        };
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"user:1"));
        assert!(labels.contains(&"user:2"));
    }

    fn schema_scope(schema: Option<&str>) -> CompletionScope {
        CompletionScope {
            connection_index: Some(0),
            database: Some("appdb".into()),
            schema: schema.map(str::to_string),
            supports_schemas: true,
            dialect: DriverDialect::Generic,
            key_value: false,
        }
    }

    fn table(database: Option<&str>, name: &str) -> sql::SqlTableRef {
        sql::SqlTableRef {
            database: database.map(str::to_string),
            name: name.to_string(),
            alias: None,
        }
    }

    #[test]
    fn catalog_target_qualifies_unqualified_tables_on_schema_engines() {
        let scope = schema_scope(Some("sales"));
        assert_eq!(
            catalog_target(&scope, &table(None, "orders")),
            Some(("appdb".into(), "sales.orders".into()))
        );
    }

    #[test]
    fn catalog_target_prefers_the_reference_schema_over_the_selected_one() {
        let scope = schema_scope(Some("sales"));
        assert_eq!(
            catalog_target(&scope, &table(Some("dbo"), "users")),
            Some(("appdb".into(), "dbo.users".into()))
        );
    }

    #[test]
    fn catalog_target_leaves_unqualified_tables_bare_without_a_schema() {
        let scope = schema_scope(None);
        assert_eq!(
            catalog_target(&scope, &table(None, "orders")),
            Some(("appdb".into(), "orders".into()))
        );
    }

    #[test]
    fn catalog_target_keeps_database_qualifiers_on_schema_less_engines() {
        let scope = CompletionScope {
            connection_index: Some(0),
            database: Some("default".into()),
            schema: None,
            supports_schemas: false,
            dialect: DriverDialect::Generic,
            key_value: false,
        };
        assert_eq!(
            catalog_target(&scope, &table(None, "orders")),
            Some(("default".into(), "orders".into()))
        );
        assert_eq!(
            catalog_target(&scope, &table(Some("other"), "orders")),
            Some(("other".into(), "orders".into()))
        );
    }

    #[test]
    fn name_in_schema_matches_the_prefix() {
        assert!(name_in_schema("dbo.users", "DBO"));
        assert!(!name_in_schema("sales.users", "dbo"));
        assert!(!name_in_schema("users", "dbo"));
    }

    #[test]
    fn display_name_strips_the_selected_schema_prefix() {
        assert_eq!(display_name("dbo.users", Some("DBO")), "users");
        // A name outside the selected schema keeps its qualified label.
        assert_eq!(display_name("sales.users", Some("dbo")), "sales.users");
        // Without a selected schema (or for an unqualified name) the name is unchanged.
        assert_eq!(display_name("dbo.users", None), "dbo.users");
        assert_eq!(display_name("users", Some("dbo")), "users");
    }

    /// A source with an empty catalog: only the built-in keywords and functions are offered, which
    /// is all the ordering test needs.
    fn empty_catalog_source() -> CompletionSource {
        CompletionSource::new(Arc::new(std::sync::RwLock::new(
            CompletionCatalog::default(),
        )))
        .with_scope(
            Some(0),
            Some("appdb".into()),
            None,
            false,
            DriverDialect::Mysql,
            false,
        )
    }

    /// A source whose catalog holds `tables`, and `columns` of `table`, in `appdb`.
    fn catalog_source(tables: &[&str], columns: &[(&str, &[&str])]) -> CompletionSource {
        let mut catalog = CompletionCatalog::default();
        catalog.tables.insert(
            (0, "appdb".to_string()),
            tables.iter().map(|table| table.to_string()).collect(),
        );
        for (table, names) in columns {
            catalog.columns.insert(
                (0, "appdb".to_string(), table.to_string()),
                names
                    .iter()
                    .map(|name| (name.to_string(), "int".to_string(), String::new()))
                    .collect(),
            );
        }
        CompletionSource::new(Arc::new(std::sync::RwLock::new(catalog))).with_scope(
            Some(0),
            Some("appdb".into()),
            None,
            false,
            DriverDialect::Mysql,
            false,
        )
    }

    fn labels_for(source: &CompletionSource, sql: &str) -> Vec<String> {
        let context = crate::sql::completion_context(sql, sql.len(), DriverDialect::Mysql);
        build_items(source, &context)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn a_table_is_found_by_any_part_of_its_name() {
        let source = catalog_source(&["wms_asn", "users"], &[]);
        // `as` is inside `wms_asn`, so the table is offered rather than filtered away...
        let labels = labels_for(&source, "SELECT * FROM as");
        assert!(labels.iter().any(|label| label == "wms_asn"), "{labels:?}");

        // ...and a loose subsequence still finds it (w…s…n).
        let labels = labels_for(&source, "SELECT * FROM wsn");
        assert_eq!(
            labels.first().map(String::as_str),
            Some("wms_asn"),
            "{labels:?}"
        );

        // An identifier that shares nothing with the name is not offered.
        let labels = labels_for(&source, "SELECT * FROM zzz");
        assert!(!labels.iter().any(|label| label == "wms_asn"), "{labels:?}");
    }

    #[test]
    fn the_everyday_keyword_leads_the_table_names() {
        // Navicat's order: what the user is writing leads the objects.
        let source = catalog_source(&["sequence", "user_search"], &[]);
        let labels = labels_for(&source, "SELECT * FROM se");
        assert_eq!(
            labels.first().map(String::as_str),
            Some("SELECT"),
            "`se` must lead with SELECT, not the `sequence` table: {labels:?}"
        );
        let table = labels.iter().position(|label| label == "sequence");
        let keyword = labels.iter().position(|label| label == "SEED");
        assert!(
            matches!((table, keyword), (Some(table), Some(keyword)) if table < keyword),
            "the table names follow the everyday keywords: {labels:?}"
        );
    }

    #[test]
    fn a_field_of_the_statements_own_table_leads_the_keywords() {
        // `WHERE orde` on an `orders` table: the column is what the caret is asking for, so the
        // `ORDER` keyword must not take its place.
        let source = catalog_source(&["orders"], &[("orders", &["order_no", "status"])]);
        let labels = labels_for(&source, "SELECT * FROM orders WHERE orde");
        assert_eq!(
            labels.first().map(String::as_str),
            Some("order_no"),
            "{labels:?}"
        );
        let column = labels.iter().position(|label| label == "order_no");
        let keyword = labels.iter().position(|label| label == "ORDER");
        assert!(
            matches!((column, keyword), (Some(column), Some(keyword)) if column < keyword),
            "the statement's own fields lead the keywords: {labels:?}"
        );
    }

    #[test]
    fn a_keyword_outranks_a_function_family_sharing_its_prefix() {
        let source = empty_catalog_source();
        let sql = "SELECT * FROM fro";
        let context = crate::sql::completion_context(sql, sql.len(), DriverDialect::Mysql);
        let labels: Vec<String> = build_items(&source, &context)
            .into_iter()
            .map(|item| item.label)
            .collect();
        let from = labels.iter().position(|label| label == "FROM");
        let base64 = labels.iter().position(|label| label == "FROM_BASE64");
        assert!(
            matches!(from, Some(from) if matches!(base64, Some(base64) if from < base64)),
            "the FROM keyword must sort before FROM_BASE64: {labels:?}"
        );
    }

    #[test]
    fn a_common_keyword_leads_the_words_that_share_its_prefix() {
        let source = empty_catalog_source();
        // The curated list's order decides between two everyday keywords...
        let sql = "SELECT * se";
        let context = crate::sql::completion_context(sql, sql.len(), DriverDialect::Mysql);
        let labels: Vec<String> = build_items(&source, &context)
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(
            labels.first().map(String::as_str),
            Some("SELECT"),
            "`se` must lead with SELECT, not SET: {labels:?}"
        );

        // ...and between an everyday keyword and the rest of the language.
        let sql = "SELECT * fr";
        let context = crate::sql::completion_context(sql, sql.len(), DriverDialect::Mysql);
        let labels: Vec<String> = build_items(&source, &context)
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(
            labels.first().map(String::as_str),
            Some("FROM"),
            "`fr` must lead with FROM, not FRAME_ROW/FREE/FREEZE: {labels:?}"
        );
        // The rest of the family follows, closest first: the fewest extra characters to type.
        let position = |label: &str| {
            labels
                .iter()
                .position(|candidate| candidate == label)
                .unwrap_or_else(|| panic!("{label} is offered for `fr`: {labels:?}"))
        };
        assert!(
            position("FREE") < position("FREEZE") && position("FREEZE") < position("FRAME_ROW"),
            "the closer match leads within the group: {labels:?}"
        );
    }
}

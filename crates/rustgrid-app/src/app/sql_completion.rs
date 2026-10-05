//! The SQL completion provider for gpui-kit's `Editor`.
//!
//! The provider is a thin adapter: it snapshots the caret's statement with `sql::completion_context`
//! (the same analysis the hand-rolled popup used) and turns the resulting candidates into LSP
//! `CompletionItem`s. It reads the connection catalog (tables, columns, functions) from a
//! [`CompletionSource`] the app keeps refreshed.

use std::collections::HashMap;
use std::sync::Arc;

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
}

/// A shared catalog and the scope the active editor targets. The scope is behind an `Arc` so a
/// live provider can be re-scoped when the toolbar's connection/database/schema changes.
#[derive(Clone)]
pub(crate) struct CompletionSource {
    catalog: Arc<std::sync::RwLock<CompletionCatalog>>,
    scope: Arc<std::sync::RwLock<CompletionScope>>,
}

impl CompletionSource {
    pub(crate) fn new(catalog: Arc<std::sync::RwLock<CompletionCatalog>>) -> Self {
        Self {
            catalog,
            scope: Arc::new(std::sync::RwLock::new(CompletionScope::default())),
        }
    }

    pub(crate) fn with_scope(
        self,
        connection_index: Option<usize>,
        database: Option<String>,
        schema: Option<String>,
        supports_schemas: bool,
        dialect: DriverDialect,
    ) -> Self {
        *self.scope.write().expect("completion scope lock") = CompletionScope {
            connection_index,
            database,
            schema,
            supports_schemas,
            dialect,
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
        _cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let sql_text = text.to_string();
        let context = sql::completion_context(&sql_text, offset);
        let mut items = build_items(&self.source, &context);
        // Anchor the replacement range explicitly. gpui's completion menu derives it from the
        // offset of the *first* keystroke that opened the menu (`trigger_start_offset`), so
        // accepting "SELECT" after typing "sele" inserts at the wrong place ("seSELECT"). An
        // explicit `text_edit` overrides that range with the real identifier fragment
        // (`prefix_start..offset`).
        let range = replacement_range(text, &context, offset);
        for item in &mut items {
            item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: item.label.clone(),
            }));
        }
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        // Open the menu on an identifier character or a `.` qualifier; the provider then filters by
        // the typed prefix.
        new_text
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_' || character == '.')
    }
}

/// Build the completion items for a caret context, ordering columns → tables → functions →
/// keywords and de-duplicating by `(label, detail)`. Under a selected schema a table/function is
/// offered by its bare name, so the typed prefix matches and accepting it does not append the
/// schema.
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
                    items.push((0, completion_item(name, &detail, kind)));
                }
            }
        }
    }

    // 2. Tables: all loaded tables, or the qualifier database's tables. With a schema selected,
    // only that schema's tables are offered (the schema selector's filtering).
    let table_kind = CompletionItemKind::CLASS;
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
            items.push((1, completion_item(&label, &database, table_kind)));
        }
    }

    // 3. Functions: built-ins plus the connection's stored functions.
    if qualifier.is_none() {
        for function in sql::functions_for(scope.dialect).iter().copied() {
            items.push((
                2,
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
                    2,
                    completion_item(&label, "function", CompletionItemKind::FUNCTION),
                ));
            }
        }
        // 4. Keywords.
        for keyword in sql::keywords() {
            items.push((
                3,
                completion_item(keyword, "keyword", CompletionItemKind::KEYWORD),
            ));
        }
    }

    let prefix = context.prefix.as_str();
    let mut seen = std::collections::HashSet::new();
    let mut filtered: Vec<(u8, CompletionItem)> = items
        .into_iter()
        .filter(|(_, item)| prefix.is_empty() || item.label.to_lowercase().starts_with(prefix))
        .filter(|(_, item)| {
            seen.insert((
                item.label.to_lowercase(),
                item.detail.clone().unwrap_or_default(),
            ))
        })
        .collect();
    filtered.sort_by(|a, b| {
        let a_exact = a.1.label.eq_ignore_ascii_case(prefix);
        let b_exact = b.1.label.eq_ignore_ascii_case(prefix);
        // An item whose label is exactly what was typed is the most likely pick, so it sorts first
        // regardless of category (the `FROM` keyword before `FROM_BASE64`, a table before
        // keywords).
        b_exact
            .cmp(&a_exact)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| {
                if a.0 == 0 {
                    std::cmp::Ordering::Equal
                } else {
                    a.1.label.to_lowercase().cmp(&b.1.label.to_lowercase())
                }
            })
    });
    filtered.truncate(64);
    filtered.into_iter().map(|(_, item)| item).collect()
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
        let context = crate::sql::completion_context(sql, sql.len());
        let range = replacement_range(&Rope::from(sql), &context, sql.len());
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, "SELECT * FROM ".len() as u32);
        assert_eq!(range.end.character, sql.len() as u32);
    }

    #[test]
    fn replacement_range_is_empty_at_a_fresh_dot() {
        let sql = "SELECT t.";
        let context = crate::sql::completion_context(sql, sql.len());
        let range = replacement_range(&Rope::from(sql), &context, sql.len());
        assert_eq!(range.start, range.end);
        assert_eq!(range.end.character, sql.len() as u32);
    }

    fn schema_scope(schema: Option<&str>) -> CompletionScope {
        CompletionScope {
            connection_index: Some(0),
            database: Some("appdb".into()),
            schema: schema.map(str::to_string),
            supports_schemas: true,
            dialect: DriverDialect::Generic,
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
}

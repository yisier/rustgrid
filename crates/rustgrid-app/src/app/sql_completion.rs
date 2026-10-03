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

/// A shared catalog and the connection/database the active editor targets.
#[derive(Clone)]
pub(crate) struct CompletionSource {
    catalog: Arc<std::sync::RwLock<CompletionCatalog>>,
    connection_index: Option<usize>,
    database: Option<String>,
}

impl CompletionSource {
    pub(crate) fn new(catalog: Arc<std::sync::RwLock<CompletionCatalog>>) -> Self {
        Self {
            catalog,
            connection_index: None,
            database: None,
        }
    }

    pub(crate) fn with_scope(
        mut self,
        connection_index: Option<usize>,
        database: Option<String>,
    ) -> Self {
        self.connection_index = connection_index;
        self.database = database;
        self
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
/// keywords and de-duplicating by `(label, detail)`.
fn build_items(
    source: &CompletionSource,
    context: &sql::SqlCompletionContext,
) -> Vec<CompletionItem> {
    if context.suppress {
        return Vec::new();
    }
    let catalog = source.catalog.read().expect("completion catalog lock");
    let qualifier = context.qualifier.last().map(String::as_str);

    let kind = if context.kind == sql::SqlCompletionKind::Table {
        CompletionItemKind::CLASS
    } else {
        CompletionItemKind::FIELD
    };

    let mut items: Vec<(u8, CompletionItem)> = Vec::new();

    // 1. Columns: from the statement's referenced tables, or the qualifier's table.
    let column_targets: Vec<(&str, &str)> = if let Some(qualifier) = qualifier {
        resolve_qualified_table(context, qualifier)
            .into_iter()
            .collect()
    } else {
        context
            .tables
            .iter()
            .filter_map(|table| {
                let database = table
                    .database
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .or(source.database.as_deref())?;
                Some((database, table.name.as_str()))
            })
            .collect()
    };
    if let Some(connection) = source.connection_index {
        for (database, table) in &column_targets {
            if let Some(columns) =
                catalog
                    .columns
                    .get(&(connection, (*database).to_string(), (*table).to_string()))
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

    // 2. Tables: all loaded tables, or the qualifier database's tables.
    let table_kind = CompletionItemKind::CLASS;
    if let Some(connection) = source.connection_index {
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
                tables.push((name.clone(), database.clone()));
            }
        }
        tables.sort();
        tables.dedup();
        for (name, database) in tables {
            items.push((1, completion_item(&name, &database, table_kind)));
        }
    }

    // 3. Functions: built-ins plus the connection's stored functions.
    if qualifier.is_none() {
        for function in sql::functions().iter().copied() {
            items.push((
                2,
                completion_item(function, "function", CompletionItemKind::FUNCTION),
            ));
        }
        if let Some(connection) = source.connection_index
            && let Some(functions) = catalog.functions.get(&connection)
        {
            for function in functions {
                items.push((
                    2,
                    completion_item(function, "function", CompletionItemKind::FUNCTION),
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
        a.0.cmp(&b.0).then_with(|| {
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

/// Resolve a single-part qualifier (`alias` or `table` or `database`) to a `(database, table)`.
fn resolve_qualified_table<'a>(
    context: &'a sql::SqlCompletionContext,
    qualifier: &str,
) -> Option<(&'a str, &'a str)> {
    let table = context.tables.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case(qualifier))
            || table.name.eq_ignore_ascii_case(qualifier)
    })?;
    let database = table.database.as_deref().filter(|name| !name.is_empty())?;
    Some((database, table.name.as_str()))
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
}

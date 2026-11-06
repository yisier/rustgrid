//! The whole-document half of the row form (document-store engines).
//!
//! A MongoDB row is a document, but the grid's columns are only a projection of the fields a
//! sample found. The form's JSON view therefore asks the grid for the selected row's authoritative
//! document (loaded on demand by its primary key) and can replace it as a whole, including fields
//! the sampled columns never showed.

use super::*;

impl GridView {
    /// The identity columns of a page row: the primary key, or every visible column when the table
    /// has none (the same fallback the write path uses).
    pub(super) fn row_key(&self, row: usize) -> Vec<(String, Option<String>)> {
        let state = &self.state;
        let primary: Vec<usize> = state
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        let columns: Vec<usize> = if primary.is_empty() {
            (0..state.visible_columns).collect()
        } else {
            primary
        };
        let Some(values) = state.rows.get(row) else {
            return Vec::new();
        };
        columns
            .into_iter()
            .filter_map(|col| {
                let column = state.columns.get(col)?;
                values
                    .get(state.source_col(col))
                    .map(|value| (column.name.clone(), value.as_edit_value()))
            })
            .collect()
    }

    /// A stable signature for one row key, so the document is refetched only when the selection
    /// moves to another row.
    fn document_signature(keys: &[(String, Option<String>)]) -> String {
        let mut signature = String::new();
        for (name, value) in keys {
            signature.push_str(name);
            signature.push('\u{1}');
            signature.push_str(value.as_deref().unwrap_or("\u{0}"));
            signature.push('\u{2}');
        }
        signature
    }

    /// Stage the JSON editor's text for `row`. This deliberately does not notify the grid: the
    /// pane already repaints on the keystroke, and notifying would re-clone the whole page while
    /// the user types (the same rule the field editors follow).
    pub(super) fn stage_document_edit(&mut self, row: usize, text: &str) {
        self.state.document_edits.insert(row, text.to_string());
        self.state.document_error = None;
    }

    /// Drop one row's staged JSON edit (the form's 回滚).
    pub(super) fn cancel_document_edit(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        let removed = self.state.document_edits.remove(&row).is_some();
        let cleared = self.state.document_error.take().is_some();
        if removed || cleared {
            cx.notify();
        }
    }

    /// Load the selected row's whole document when the form's JSON view asks for it. One request
    /// per row key; a failed load is remembered so a render loop cannot hammer the server.
    pub(super) fn ensure_row_document(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        if !self.state.documents || self.state.sql.is_some() {
            return;
        }
        let keys = self.row_key(row);
        if keys.is_empty() {
            return;
        }
        let signature = Self::document_signature(&keys);
        if self.state.document_key.as_deref() == Some(signature.as_str()) {
            return;
        }
        if self.document_request.as_deref() == Some(signature.as_str()) {
            return;
        }
        self.document_request = Some(signature.clone());
        self.state.document_loading = true;
        self.state.document_json = None;
        self.state.document_error = None;

        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let runtime = self.runtime.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.row_document(&database, &table, &keys).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |grid, cx| {
                // A later selection may already have requested another row; that response wins.
                if grid.document_request.as_deref() != Some(signature.as_str()) {
                    return;
                }
                grid.document_request = None;
                grid.state.document_loading = false;
                grid.state.document_key = Some(signature);
                match result {
                    Ok(Some(document)) => {
                        grid.state.document_json = Some(document);
                        grid.state.document_error = None;
                    }
                    Ok(None) => {
                        grid.state.document_json = None;
                        grid.state.document_error =
                            Some(t!("info.form.document_missing").to_string());
                    }
                    Err(error) => {
                        let message = match error {
                            Error::Query(text) => text,
                            other => other.to_string(),
                        };
                        grid.state.document_json = None;
                        grid.state.document_error = Some(message.clone());
                        grid.show_error(message, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Replace the selected row with the JSON view's text, then reload the page so the grid and
    /// the form show the stored document again (a replacement may add or remove fields).
    pub(super) fn save_row_document(
        &mut self,
        row: usize,
        document: String,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.state.documents || self.state.sql.is_some() {
            return;
        }
        if self.state.document_json.as_deref() == Some(document.as_str()) {
            self.show_toast(ToastKind::Info, t!("grid.no_changes").to_string(), cx);
            return;
        }
        let keys = self.row_key(row);
        if keys.is_empty() {
            self.state.document_error = Some(t!("info.form.document_missing").to_string());
            cx.notify();
            return;
        }
        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    connection
                        .replace_row_document(&database, &table, &keys, &document)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |grid, cx| match result {
                Ok(()) => {
                    grid.state.document_edits.remove(&row);
                    grid.state.document_error = None;
                    grid.show_toast(ToastKind::Success, t!("grid.saved").to_string(), cx);
                    grid.reload_after_write(cx);
                }
                Err(error) => {
                    let message = match error {
                        Error::Query(text) => text,
                        other => other.to_string(),
                    };
                    grid.state.document_error = Some(message.clone());
                    grid.show_error(message, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

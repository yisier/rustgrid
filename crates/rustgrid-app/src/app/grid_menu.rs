//! The grid's right-click menu and its row clipboard: 删除记录 / 复制 / 复制为 INSERT / 粘贴 / 刷新.
//!
//! The menu itself is drawn by `AppView` (like every other context menu), so this module owns the
//! grid-side behaviour the menu items call into: copying the selected cells to the OS clipboard
//! (and an app-wide clipboard that remembers NULLs), building `INSERT` statements, and pasting a
//! copied block into the selected records.

use super::grid::row_origin;
use super::*;

use crate::session::EditAction;
use base64::Engine as _;

impl GridView {
    /// Open the grid's context menu at `position` (window coordinates). `AppView` renders it, so
    /// the position stays correct regardless of the grid's own offset inside the window.
    pub(super) fn open_grid_menu(
        &mut self,
        position: Point<Pixels>,
        cell: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        window.focus(&self.focus, cx);
        let grid_id = self.state.id;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.context_menu = Some(ContextMenu {
                    target: ContextTarget::Grid { grid_id, cell },
                    position,
                });
                cx.notify();
            });
        }
    }

    /// Right-click on a column header: select that column (Navicat-style) and open the header menu
    /// (冻结列 / 取消冻结 / 隐藏列 / 刷新) at the pointer.
    pub(super) fn grid_header_right_mouse_down(
        &mut self,
        column: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cell_editor.is_some() {
            self.finish_cell_editor(cx);
        }
        let rows = self.display_row_count();
        if rows > 0 && column < self.state.visible_columns {
            self.state.selection = Some(CellSelection::single((rows - 1, column), (0, column)));
        }
        self.selecting_cells = false;
        window.focus(&self.focus, cx);
        let grid_id = self.state.id;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.context_menu = Some(ContextMenu {
                    target: ContextTarget::GridHeader { grid_id, column },
                    position,
                });
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Freeze the field at display position `column`: it moves to the front of the field order and
    /// stays put while the other columns pan. Freezing another field later puts it after the
    /// already frozen ones, so the first frozen field is the first column, the second the second,
    /// and so on. The unfrozen order stays in [`GridState::field_layout`], so unfreezing restores
    /// each field to its original position.
    pub(super) fn freeze_column(&mut self, column: usize, cx: &mut Context<'_, Self>) {
        if column >= self.state.visible_columns {
            return;
        }
        let scroll = self.grid_column_scroll();
        let name = self.state.columns[column].name.clone();
        if !self.state.frozen_columns.contains(&name) {
            self.state.frozen_columns.push(name);
        }
        self.state.reproject_fields();
        self.column_scroll = scroll.clamp(0.0, self.grid_scroll_max());
        cx.notify();
    }

    /// 取消冻结: unfreeze the field at display position `column` and put it back where it sat in
    /// the field layout. The other frozen fields stay pinned to the front.
    pub(super) fn unfreeze_columns(&mut self, column: usize, cx: &mut Context<'_, Self>) {
        let Some(name) = self
            .state
            .columns
            .get(column)
            .map(|column| column.name.clone())
        else {
            return;
        };
        if !self.state.frozen_columns.contains(&name) {
            return;
        }
        self.state.frozen_columns.retain(|frozen| *frozen != name);
        self.finish_unfreeze(cx);
    }

    /// 取消冻结全部列: clear every frozen field of this grid at once.
    pub(super) fn unfreeze_all_columns(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.frozen_columns.is_empty() {
            return;
        }
        self.state.frozen_columns.clear();
        self.finish_unfreeze(cx);
    }

    /// Re-project the grid after the frozen set changed and put the column pan back to a valid
    /// position. Without a frozen field the tracked scroll handle drives the columns again; with
    /// one still frozen the offset stays in `column_scroll`.
    fn finish_unfreeze(&mut self, cx: &mut Context<'_, Self>) {
        let scroll = self.grid_column_scroll();
        self.state.reproject_fields();
        if self.state.frozen_columns.is_empty() {
            self.column_scroll = 0.0;
            let y = self.hscroll.offset().y;
            self.hscroll.set_offset(Point::new(px(-scroll), y));
        } else {
            self.column_scroll = scroll.clamp(0.0, self.grid_scroll_max());
        }
        cx.notify();
    }

    /// 隐藏列: hide the field through the same layout the 字段筛选 popup edits, so the popup lists
    /// it unchecked the next time it opens. Returns whether the field was hidden; the caller
    /// persists [`Self::field_layout_snapshot`]. A menu listener already runs inside
    /// `AppView::update`, so this method must not update `AppView` through its weak handle (that
    /// would re-enter the same entity and crash).
    pub(super) fn hide_column(&mut self, column: usize, cx: &mut Context<'_, Self>) -> bool {
        if self.state.sql.is_some()
            || column >= self.state.visible_columns
            || self.state.visible_columns <= 1
        {
            return false;
        }
        let name = self.state.columns[column].name.clone();
        // Hiding a frozen field unfreezes it: it must not jump back to the front when it is shown
        // again. `reproject_fields` also prunes it, but doing it here keeps the scroll maths below
        // honest for this frame.
        self.state.frozen_columns.retain(|frozen| *frozen != name);
        self.state.field_layout.set_hidden(&name, true);
        self.state.reproject_fields();
        self.column_scroll = self.column_scroll.min(self.grid_scroll_max());
        cx.notify();
        true
    }

    /// The edit value of a cell: a staged edit/insert value if present, otherwise the loaded
    /// `CellValue`. `None` is SQL `NULL`.
    fn cell_edit_value(&self, row: usize, col: usize) -> Option<String> {
        if col >= self.state.visible_columns {
            return None;
        }
        let origin = self.row_at(row)?;
        if let Some(staged) = self.staged_value(row, col) {
            return staged.clone();
        }
        match origin {
            GridRow::Data(data) => {
                match self
                    .state
                    .rows
                    .get(data)
                    .and_then(|values| values.get(self.state.source_col(col)))
                {
                    Some(CellValue::Null) | None => None,
                    Some(cell) => Some(cell.as_edit_string()),
                }
            }
            GridRow::Insert(_) => None,
        }
    }

    /// Right-click on the grid: select the row/cell under the pointer when it is not already
    /// selected (Navicat-style), then open the context menu.
    pub(super) fn grid_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cell_editor.is_some() {
            let editing_here = self.grid_hit(event.position).is_some_and(|hit| match hit {
                GridHit::Cell(row, col) => self
                    .cell_editor
                    .as_ref()
                    .is_some_and(|editor| editor.row == row && editor.col == col),
                GridHit::Gutter(_) => false,
            });
            if editing_here {
                return;
            }
            self.finish_cell_editor(cx);
        }
        if let Some(hit) = self.grid_hit(event.position) {
            let column_count = self.state.visible_columns;
            let inside = match hit {
                GridHit::Gutter(row) => self.state.selection.as_ref().is_some_and(|selection| {
                    let (start_row, end_row) = selection.rows();
                    let (start_col, end_col) = selection.cols();
                    row >= start_row
                        && row <= end_row
                        && start_col == 0
                        && end_col + 1 == column_count
                }),
                GridHit::Cell(row, col) => self
                    .state
                    .selection
                    .as_ref()
                    .is_some_and(|selection| selection.contains(row, col)),
            };
            if !inside {
                self.state.selection = match hit {
                    GridHit::Gutter(row) => Some(CellSelection::single(
                        (row, 0),
                        (row, column_count.saturating_sub(1)),
                    )),
                    GridHit::Cell(row, col) => Some(CellSelection::new(row, col)),
                };
            }
        }
        let cell = match self.grid_hit(event.position) {
            Some(GridHit::Cell(row, col)) => Some((row, col)),
            _ => None,
        };
        self.open_grid_menu(event.position, cell, window, cx);
        cx.notify();
    }

    /// Copy the selected cells (their bounding rectangle) to the OS clipboard as TSV, and keep the
    /// exact values (including NULLs) in the in-app clipboard for 粘贴.
    pub(super) fn copy_selection(&mut self, cx: &mut Context<'_, Self>) {
        let Some(selection) = self.state.selection.clone() else {
            return;
        };
        let (start_row, end_row) = selection.rows();
        let (start_col, end_col) = selection.cols();
        let mut block = Vec::new();
        let mut lines = Vec::new();
        for row in start_row..=end_row {
            let mut values = Vec::new();
            let mut text_cells = Vec::new();
            for col in start_col..=end_col {
                let value = if selection.contains(row, col) {
                    self.cell_edit_value(row, col)
                } else {
                    None
                };
                text_cells.push(value.clone().unwrap_or_default());
                values.push(value);
            }
            block.push(values);
            lines.push(text_cells.join("\t"));
        }
        let text = lines.join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        {
            let mut clipboard = self.clipboard.borrow_mut();
            clipboard.block = Some(block);
            clipboard.text = Some(text);
        }
        cx.notify();
    }

    /// The JSON `insert` command for the selected rows of a document store: each row becomes one
    /// document, and each cell is re-wrapped as Extended JSON (`$oid`, `$date`, `$numberDecimal`,
    /// ...) so the command keeps the BSON types it was copied from.
    fn document_insert_command(
        &self,
        selection: &CellSelection,
        collection: &str,
    ) -> serde_json::Value {
        let (start_row, end_row) = selection.rows();
        let (start_col, end_col) = selection.cols();
        let mut documents = Vec::new();
        for row in start_row..=end_row {
            let mut document = serde_json::Map::new();
            for col in start_col..=end_col {
                if !selection.contains(row, col) {
                    continue;
                }
                let Some(column) = self.state.columns.get(col) else {
                    continue;
                };
                let value = self.cell_edit_value(row, col);
                document.insert(
                    column.name.clone(),
                    document_cell_value(value, &column.data_type),
                );
            }
            documents.push(serde_json::Value::Object(document));
        }
        serde_json::json!({ "insert": collection, "documents": documents })
    }

    /// Copy the selected rows as `INSERT` statements.
    pub(super) fn copy_selection_as_insert(&mut self, cx: &mut Context<'_, Self>) {
        let Some(selection) = self.state.selection.clone() else {
            return;
        };
        let (start_row, end_row) = selection.rows();
        let (start_col, end_col) = selection.cols();
        if self.state.visible_columns == 0 {
            return;
        }
        let database = self.state.database.clone();
        let table = if self.state.table.is_empty() {
            "table".to_string()
        } else {
            self.state.table.clone()
        };
        // A document store's rows are documents: a SQL `INSERT` would not run in its query tab.
        // Emit the equivalent JSON `insert` command (Extended JSON for the typed cells) instead.
        if self.state.documents {
            let command = self.document_insert_command(&selection, &table);
            let text = serde_json::to_string_pretty(&command).unwrap_or_default();
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            {
                let mut clipboard = self.clipboard.borrow_mut();
                clipboard.block = None;
                clipboard.text = Some(text);
            }
            cx.notify();
            return;
        }
        let columns: Vec<String> = (start_col..=end_col)
            .map(|col| format!("`{}`", self.state.columns[col].name.replace('`', "``")))
            .collect();
        let mut sql = String::new();
        for row in start_row..=end_row {
            let values: Vec<String> = (start_col..=end_col)
                .map(|col| {
                    let value = if selection.contains(row, col) {
                        self.cell_edit_value(row, col)
                    } else {
                        None
                    };
                    sql_literal(&value)
                })
                .collect();
            sql.push_str(&format!(
                "INSERT INTO `{}`.`{}` ({}) VALUES ({});\n",
                database,
                table,
                columns.join(", "),
                values.join(", ")
            ));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(sql.clone()));
        // An INSERT copy is not a cell block; make paste treat the matching OS text as unusable
        // rather than replaying the previous cell copy.
        {
            let mut clipboard = self.clipboard.borrow_mut();
            clipboard.block = None;
            clipboard.text = Some(sql);
        }
        cx.notify();
    }

    /// Paste the in-app clipboard (or, when it is empty, the OS clipboard's TSV text) into the
    /// selected records. A copied block whose row count matches the selection maps one-to-one onto
    /// the selected rows; otherwise the block is anchored at the selection's top-left cell.
    pub(super) fn paste_clipboard(&mut self, cx: &mut Context<'_, Self>) {
        if !self.state.editable || self.state.visible_columns == 0 {
            return;
        }
        // Prefer the OS clipboard when it changed since our own copy (so external / other-app
        // data pastes); otherwise use the in-app block, which preserves NULLs. The comparison
        // normalises line endings: the platform may hand our own text back with CRLF (or a
        // trailing newline), which would otherwise look external and turn NULLs into `''`.
        let (own_block, own_text) = {
            let clipboard = self.clipboard.borrow();
            (clipboard.block.clone(), clipboard.text.clone())
        };
        let system_text = cx.read_from_clipboard().and_then(|item| item.text());
        let block = match system_text {
            Some(text)
                if Some(normalize_clipboard_text(&text))
                    == own_text.as_deref().map(normalize_clipboard_text) =>
            {
                own_block
            }
            Some(text) => Some(parse_clipboard_text(&text)),
            None => own_block,
        };
        let Some(block) = block else {
            return;
        };
        if block.is_empty() {
            return;
        }
        let selection = self.state.selection.clone();
        let (start_row, start_col) = match selection.as_ref() {
            Some(selection) => {
                let range = selection.active();
                (range.rows().0, range.cols().0)
            }
            None => (0, 0),
        };
        let selected_rows = selection
            .as_ref()
            .map(|selection| selection.row_indices())
            .unwrap_or_default();
        let map_rows = !selected_rows.is_empty() && selected_rows.len() == block.len();
        let mut action = Vec::new();
        for (index, values) in block.iter().enumerate() {
            let target_row = if map_rows {
                selected_rows[index]
            } else {
                start_row + index
            };
            if map_rows {
                // A one-to-one paste onto selected records never creates rows; it only fills the
                // selected ones, which may include pending insert rows the user added.
                if target_row >= self.display_row_count() {
                    break;
                }
            } else {
                // A free paste that runs past the grid grows appended pending rows to hold the
                // overflow.
                while target_row >= self.display_row_count() {
                    self.inserts.push(PendingInsert::appended());
                }
            }
            for (offset, value) in values.iter().enumerate() {
                let target_col = start_col + offset;
                if target_col >= self.state.visible_columns {
                    break;
                }
                self.stage_paste_cell(target_row, target_col, value, &mut action);
            }
        }
        if !action.is_empty() {
            self.state.redo.clear();
            self.state.undo.push(action);
            if self.state.undo.len() > 256 {
                self.state.undo.remove(0);
            }
        }
        cx.notify();
    }

    /// Stage one pasted value into an existing row's edit map or a pending insert row.
    fn stage_paste_cell(
        &mut self,
        row: usize,
        col: usize,
        value: &Option<String>,
        action: &mut EditAction,
    ) {
        // An empty clipboard cell is "no value". A numeric/temporal column cannot store `''`
        // (MySQL rejects it with 1366), so pasting one must stage SQL NULL rather than an empty
        // string; text columns keep an explicit empty string.
        let null = None;
        let value = if value.as_deref() == Some("")
            && empty_paste_is_null(&self.state.columns[col].data_type)
        {
            &null
        } else {
            value
        };
        let data_rows = self.state.rows.len();
        match row_origin(&self.inserts, data_rows, row) {
            Some(GridRow::Data(row)) => {
                let previous = self.state.edits.get(&(row, col)).cloned();
                if previous.as_ref() == Some(value) {
                    return;
                }
                action.push(((row, col), previous));
                self.state.edits.insert((row, col), value.clone());
            }
            Some(GridRow::Insert(index)) => {
                if let Some(insert) = self.inserts.get_mut(index) {
                    insert.values.insert(col, value.clone());
                }
            }
            None => {}
        }
    }
}

/// Render one clipboard cell as a SQL literal (`NULL`, or a quoted, escaped string).
fn sql_literal(value: &Option<String>) -> String {
    match value {
        None => "NULL".to_string(),
        Some(text) => format!("'{}'", text.replace('\'', "''")),
    }
}

/// One cell of a document-store row as Extended JSON, so a copied `insert` command keeps the BSON
/// types the grid showed (ObjectId, date, decimal, ...). `None` is BSON `null`.
fn document_cell_value(value: Option<String>, data_type: &str) -> serde_json::Value {
    use serde_json::{Value, json};

    let Some(text) = value else {
        return Value::Null;
    };
    match data_type.trim().to_ascii_lowercase().as_str() {
        "objectid" | "object id" | "oid" => json!({ "$oid": text }),
        "date" | "datetime" => json!({ "$date": text }),
        "decimal" | "decimal128" => json!({ "$numberDecimal": text }),
        "int" | "int32" | "integer" => json!({ "$numberInt": text }),
        "long" | "int64" => json!({ "$numberLong": text }),
        "double" | "float" | "number" => text
            .parse::<f64>()
            .map(|number| json!(number))
            .unwrap_or(Value::String(text)),
        "bool" | "boolean" => text
            .parse::<bool>()
            .map(Value::Bool)
            .unwrap_or(Value::String(text)),
        // Embedded documents/arrays already arrive as Extended JSON text.
        "object" | "document" | "array" | "json" => {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        }
        "uuid" => json!({ "$uuid": text }),
        "timestamp" => timestamp_json_value(&text),
        "regex" | "regularexpression" => regex_json_value(&text),
        "bindata" | "binary" => binary_json_value(&text),
        "null" => Value::Null,
        "minkey" => json!({ "$minKey": 1 }),
        "maxkey" => json!({ "$maxKey": 1 }),
        _ => Value::String(text),
    }
}

/// `Timestamp(秒, 序号)` → `{"$timestamp":{"t":…,"i":…}}`.
fn timestamp_json_value(text: &str) -> serde_json::Value {
    let inner = text
        .trim()
        .strip_prefix("Timestamp(")
        .and_then(|rest| rest.strip_suffix(')'));
    if let Some(inner) = inner
        && let Some((time, increment)) = inner.split_once(',')
        && let (Ok(time), Ok(increment)) =
            (time.trim().parse::<u64>(), increment.trim().parse::<u64>())
    {
        return serde_json::json!({ "$timestamp": { "t": time, "i": increment } });
    }
    serde_json::Value::String(text.to_string())
}

/// `/pattern/flags` → `{"$regularExpression":{"pattern":…,"options":…}}`.
fn regex_json_value(text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    if let Some(inner) = trimmed.strip_prefix('/')
        && let Some((pattern, options)) = inner.rsplit_once('/')
    {
        return serde_json::json!({
            "$regularExpression": { "pattern": pattern, "options": options }
        });
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
}

/// Hex cell bytes → `{"$binary":{"base64":…,"subType":"00"}}`.
fn binary_json_value(text: &str) -> serde_json::Value {
    fn decode_hex(text: &str) -> Option<Vec<u8>> {
        let text = text.trim();
        if !text.len().is_multiple_of(2) {
            return None;
        }
        let mut bytes = Vec::with_capacity(text.len() / 2);
        for pair in text.as_bytes().chunks(2) {
            let pair = std::str::from_utf8(pair).ok()?;
            bytes.push(u8::from_str_radix(pair, 16).ok()?);
        }
        Some(bytes)
    }

    match decode_hex(text) {
        Some(bytes) => serde_json::json!({
            "$binary": {
                "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                "subType": "00",
            }
        }),
        None => serde_json::Value::String(text.to_string()),
    }
}

/// Normalize clipboard text for the "is this our own copy?" comparison. Windows and clipboard
/// managers may hand CRLF (or a trailing line break) back for text written with LF; without this
/// the own block is skipped and its `NULL` cells are re-read from text as empty strings.
fn normalize_clipboard_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_string()
}

/// Whether an empty pasted cell must become SQL `NULL` for this column type: numeric and temporal
/// columns reject `''`, while text columns can hold an explicit empty string. Unknown types keep
/// the empty string (the conservative choice, since the engine may accept it).
pub(super) fn empty_paste_is_null(data_type: &str) -> bool {
    let kind = data_type.to_ascii_lowercase();
    if kind.contains("char") || kind.contains("text") || kind.contains("clob") {
        return false;
    }
    [
        "int",
        "decimal",
        "numeric",
        "number",
        "float",
        "double",
        "real",
        "money",
        "bit",
        "bool",
        "year",
        "date",
        "time",
        "interval",
        "serial",
        "identity",
        "uniqueidentifier",
        "uuid",
        "json",
    ]
    .iter()
    .any(|marker| kind.contains(marker))
}

/// Parse pasted text into rows of cells: newline-separated rows, tab-separated columns. A literal
/// `NULL` cell becomes SQL `NULL`.
fn parse_clipboard_text(text: &str) -> Vec<Vec<Option<String>>> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split('\t')
                .map(|cell| {
                    if cell == "NULL" {
                        None
                    } else {
                        Some(cell.to_string())
                    }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_copy_is_recognised_after_line_ending_normalisation() {
        // What the app writes (LF) and what Windows / a clipboard manager can hand back (CRLF,
        // trailing newline) must compare equal, or the NULL-preserving block is skipped.
        let written = "1\tNULL\n2\tx";
        let read_back = "1\tNULL\r\n2\tx\r\n";
        assert_eq!(
            normalize_clipboard_text(written),
            normalize_clipboard_text(read_back)
        );
    }

    #[test]
    fn parsed_text_keeps_nulls_and_empty_cells_apart() {
        let block = parse_clipboard_text("NULL\t\r\n\tx");
        assert_eq!(block[0][0], None);
        assert_eq!(block[0][1], Some(String::new()));
    }

    #[test]
    fn an_empty_paste_is_null_only_for_non_text_columns() {
        assert!(empty_paste_is_null("int"));
        assert!(empty_paste_is_null("bigint unsigned"));
        assert!(empty_paste_is_null("decimal(10,2)"));
        assert!(empty_paste_is_null("timestamp without time zone"));
        assert!(empty_paste_is_null("uniqueidentifier"));
        assert!(empty_paste_is_null("jsonb"));
        assert!(!empty_paste_is_null("varchar(64)"));
        assert!(!empty_paste_is_null("nvarchar(max)"));
        assert!(!empty_paste_is_null("longtext"));
        assert!(!empty_paste_is_null("enum('a','b')"));
    }

    #[test]
    fn copied_document_cells_keep_their_bson_types() {
        use serde_json::json;

        assert_eq!(
            document_cell_value(Some("65f0c0ffee00000000000001".into()), "objectId"),
            json!({ "$oid": "65f0c0ffee00000000000001" })
        );
        assert_eq!(
            document_cell_value(Some("1943.80".into()), "decimal"),
            json!({ "$numberDecimal": "1943.80" })
        );
        assert_eq!(
            document_cell_value(Some("42".into()), "int"),
            json!({ "$numberInt": "42" })
        );
        assert_eq!(
            document_cell_value(Some("2026-04-04T00:00:00.000Z".into()), "date"),
            json!({ "$date": "2026-04-04T00:00:00.000Z" })
        );
        assert_eq!(
            document_cell_value(Some("[1, 2]".into()), "array"),
            json!([1, 2])
        );
        assert_eq!(document_cell_value(None, "string"), json!(null));
        // A string column stays a string even when its text looks like a number.
        assert_eq!(
            document_cell_value(Some("123".into()), "string"),
            json!("123")
        );
    }

    #[test]
    fn copied_timestamp_and_regex_cells_use_extended_json() {
        use serde_json::json;

        assert_eq!(
            document_cell_value(Some("Timestamp(1768461000, 1)".into()), "timestamp"),
            json!({ "$timestamp": { "t": 1768461000u64, "i": 1u64 } })
        );
        assert_eq!(
            document_cell_value(Some("/^rg-.*$/i".into()), "regex"),
            json!({ "$regularExpression": { "pattern": "^rg-.*$", "options": "i" } })
        );
    }
}

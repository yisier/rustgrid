//! The grid's right-click menu and its row clipboard: 删除记录 / 复制 / 复制为 INSERT / 粘贴 / 刷新.
//!
//! The menu itself is drawn by `AppView` (like every other context menu), so this module owns the
//! grid-side behaviour the menu items call into: copying the selected cells to the OS clipboard
//! (and an app-wide clipboard that remembers NULLs), building `INSERT` statements, and pasting a
//! copied block into the selected records.

use super::grid::row_origin;
use super::*;

use crate::session::EditAction;

impl GridView {
    /// Open the grid's context menu at `position` (window coordinates). `AppView` renders it, so
    /// the position stays correct regardless of the grid's own offset inside the window.
    pub(super) fn open_grid_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        window.focus(&self.focus, cx);
        let grid_id = self.state.id;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.context_menu = Some(ContextMenu {
                    target: ContextTarget::Grid { grid_id },
                    position,
                });
                cx.notify();
            });
        }
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
        self.open_grid_menu(event.position, window, cx);
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
fn empty_paste_is_null(data_type: &str) -> bool {
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
}

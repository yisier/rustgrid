//! The grid's right-click menu and its row clipboard: 删除记录 / 复制 / 复制为 INSERT / 粘贴 / 刷新.
//!
//! The menu itself is drawn by `AppView` (like every other context menu), so this module owns the
//! grid-side behaviour the menu items call into: copying the selected cells to the OS clipboard
//! (and an in-app clipboard that remembers NULLs), building `INSERT` statements, and pasting a
//! copied block into the selected records.

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
        if col >= self.state.columns.len() {
            return None;
        }
        if let Some(staged) = self.staged_value(row, col) {
            return staged.clone();
        }
        let data_rows = self.state.rows.len();
        if row < data_rows {
            match &self.state.rows[row][col] {
                CellValue::Null => None,
                cell => Some(cell.as_edit_string()),
            }
        } else {
            None
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
            let column_count = self.state.columns.len();
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
        self.clipboard = Some(block);
        self.clipboard_text = Some(text);
        cx.notify();
    }

    /// Copy the selected rows as `INSERT` statements.
    pub(super) fn copy_selection_as_insert(&mut self, cx: &mut Context<'_, Self>) {
        let Some(selection) = self.state.selection.clone() else {
            return;
        };
        let (start_row, end_row) = selection.rows();
        let (start_col, end_col) = selection.cols();
        if self.state.columns.is_empty() {
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
        self.clipboard = None;
        self.clipboard_text = Some(sql);
        cx.notify();
    }

    /// Paste the in-app clipboard (or, when it is empty, the OS clipboard's TSV text) into the
    /// selected records. A copied block whose row count matches the selection maps one-to-one onto
    /// the selected rows; otherwise the block is anchored at the selection's top-left cell.
    pub(super) fn paste_clipboard(&mut self, cx: &mut Context<'_, Self>) {
        if !self.state.editable || self.state.columns.is_empty() {
            return;
        }
        // Prefer the OS clipboard when it changed since our own copy (so external / other-app
        // data pastes); otherwise use the in-app block, which preserves NULLs.
        let system_text = cx.read_from_clipboard().and_then(|item| item.text());
        let block = match system_text {
            Some(text) if Some(&text) == self.clipboard_text.as_ref() => self.clipboard.clone(),
            Some(text) => Some(parse_clipboard_text(&text)),
            None => self.clipboard.clone(),
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
        let data_rows = self.state.rows.len();
        let mut action = Vec::new();
        for (index, values) in block.iter().enumerate() {
            let target_row = if map_rows {
                selected_rows[index]
            } else {
                start_row + index
            };
            if target_row >= data_rows {
                // A one-to-one paste onto selected records never creates rows; a free paste that
                // runs past the page grows pending insert rows to hold the overflow.
                if map_rows {
                    break;
                }
                let needed = target_row - data_rows + 1;
                while self.inserts.len() < needed {
                    self.inserts.push(BTreeMap::new());
                }
            }
            for (offset, value) in values.iter().enumerate() {
                let target_col = start_col + offset;
                if target_col >= self.state.columns.len() {
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
        let data_rows = self.state.rows.len();
        if row < data_rows {
            let previous = self.state.edits.get(&(row, col)).cloned();
            if previous.as_ref() == Some(value) {
                return;
            }
            action.push(((row, col), previous));
            self.state.edits.insert((row, col), value.clone());
        } else if let Some(insert) = self.inserts.get_mut(row - data_rows) {
            insert.insert(col, value.clone());
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

use super::grid::row_origin;
use super::grid_menu::empty_paste_is_null;
use super::*;
use chrono::Duration as ChronoDuration;

/// Excel's 自动填充: the fill handle at the bottom-right corner of the selection, dragged
/// vertically. The source block is read when the pointer is released, so a drag never copies a
/// whole page's worth of values up front, and nothing is written to the database before the usual
/// Save / 提交 path.
#[derive(Clone, Copy, Debug)]
pub(super) struct FillDrag {
    /// The active selection's bounding rectangle at the start of the drag (display rows).
    start_row: usize,
    end_row: usize,
    start_col: usize,
    end_col: usize,
    /// The row the pointer currently targets; clamped to the displayed rows.
    target_row: usize,
}

impl FillDrag {
    /// The rows a release would fill, beyond the source block: `(first, last)` in display order,
    /// or `None` while the pointer is still inside the selection.
    fn preview_rows(&self) -> Option<(usize, usize)> {
        if self.target_row > self.end_row {
            Some((self.end_row + 1, self.target_row))
        } else if self.target_row < self.start_row {
            Some((self.target_row, self.start_row.saturating_sub(1)))
        } else {
            None
        }
    }

    /// Whether a cell is part of the extended range the drag is about to fill.
    pub(super) fn previews(&self, row: usize, col: usize) -> bool {
        (self.start_col..=self.end_col).contains(&col)
            && self
                .preview_rows()
                .is_some_and(|(first, last)| (first..=last).contains(&row))
    }
}

/// The auto-increment rule inferred from one source column.
enum FillSeries {
    Integer {
        first: i128,
        step: i128,
        /// Digit width kept from a leading-zero source (`001` -> `002`).
        pad: usize,
    },
    Decimal {
        first: f64,
        step: f64,
        decimals: usize,
    },
    DateTime {
        first: NaiveDateTime,
        step_seconds: i64,
        with_time: bool,
        with_seconds: bool,
    },
    /// Anything else repeats the source block cyclically, exactly like Excel's copy fill.
    Pattern(Vec<Option<String>>),
}

impl FillSeries {
    fn build(values: &[Option<String>]) -> Self {
        if values.is_empty() {
            return Self::Pattern(Vec::new());
        }
        if values.iter().all(Option::is_some) {
            let texts: Vec<&str> = values.iter().filter_map(|value| value.as_deref()).collect();
            if let Some(series) = Self::integer(&texts) {
                return series;
            }
            if let Some(series) = Self::decimal(&texts) {
                return series;
            }
            if let Some(series) = Self::datetime(&texts) {
                return series;
            }
        }
        Self::Pattern(values.to_vec())
    }

    fn integer(texts: &[&str]) -> Option<Self> {
        let numbers: Vec<i128> = texts
            .iter()
            .map(|text| text.trim().parse().ok())
            .collect::<Option<_>>()?;
        let step = if numbers.len() >= 2 {
            numbers[numbers.len() - 1] - numbers[numbers.len() - 2]
        } else {
            1
        };
        let pad = texts
            .first()
            .map(|text| leading_zero_width(text))
            .unwrap_or(0);
        Some(Self::Integer {
            first: numbers[0],
            step,
            pad,
        })
    }

    fn decimal(texts: &[&str]) -> Option<Self> {
        let numbers: Vec<f64> = texts
            .iter()
            .map(|text| text.trim().parse().ok())
            .collect::<Option<_>>()?;
        if !numbers.iter().all(|number| number.is_finite()) {
            return None;
        }
        let step = if numbers.len() >= 2 {
            numbers[numbers.len() - 1] - numbers[numbers.len() - 2]
        } else {
            1.0
        };
        if !step.is_finite() {
            return None;
        }
        let decimals = texts
            .iter()
            .map(|text| decimal_places(text))
            .max()
            .unwrap_or(0)
            .min(10);
        Some(Self::Decimal {
            first: numbers[0],
            step,
            decimals,
        })
    }

    fn datetime(texts: &[&str]) -> Option<Self> {
        let dates: Vec<NaiveDateTime> = texts
            .iter()
            .map(|text| parse_datetime(text))
            .collect::<Option<_>>()?;
        let with_time = texts.iter().any(|text| text.contains(':'));
        let with_seconds = texts.iter().any(|text| text.matches(':').count() >= 2);
        let step = if dates.len() >= 2 {
            dates[dates.len() - 1] - dates[dates.len() - 2]
        } else {
            ChronoDuration::days(1)
        };
        Some(Self::DateTime {
            first: dates[0],
            step_seconds: step.num_seconds(),
            with_time,
            with_seconds,
        })
    }

    /// The value at `offset` from the source block's first row (negative when filling upwards).
    fn value_at(&self, offset: isize) -> Option<String> {
        match self {
            Self::Integer { first, step, pad } => {
                let value = first.checked_add(step.checked_mul(offset as i128)?)?;
                Some(format_integer(value, *pad))
            }
            Self::Decimal {
                first,
                step,
                decimals,
            } => {
                let value = first + step * offset as f64;
                if !value.is_finite() {
                    return None;
                }
                // Keep -0 from rendering as "-0" when the series crosses zero.
                let value = if value == 0.0 { 0.0 } else { value };
                Some(format!("{value:.decimals$}"))
            }
            Self::DateTime {
                first,
                step_seconds,
                with_time,
                with_seconds,
            } => {
                let total = step_seconds.checked_mul(offset as i64)?;
                let value = first.checked_add_signed(ChronoDuration::seconds(total))?;
                Some(if !with_time {
                    value.format("%Y-%m-%d").to_string()
                } else if *with_seconds {
                    value.format("%Y-%m-%d %H:%M:%S").to_string()
                } else {
                    value.format("%Y-%m-%d %H:%M").to_string()
                })
            }
            Self::Pattern(values) => {
                if values.is_empty() {
                    return None;
                }
                let index = (offset as i128).rem_euclid(values.len() as i128) as usize;
                values.get(index).cloned().flatten()
            }
        }
    }
}

/// The digit count of a leading-zero integer (`001` -> 3); `0` when the text is not one.
fn leading_zero_width(text: &str) -> usize {
    let digits = text.trim().trim_start_matches(['-', '+']);
    if digits.len() > 1 && digits.starts_with('0') && digits.chars().all(|c| c.is_ascii_digit()) {
        digits.len()
    } else {
        0
    }
}

fn format_integer(value: i128, pad: usize) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let digits = value.unsigned_abs().to_string();
    if digits.len() < pad {
        format!("{sign}{}{}", "0".repeat(pad - digits.len()), digits)
    } else {
        format!("{sign}{digits}")
    }
}

fn decimal_places(text: &str) -> usize {
    text.trim()
        .split_once('.')
        .map(|(_, fraction)| fraction.chars().take_while(char::is_ascii_digit).count())
        .unwrap_or(0)
}

impl GridView {
    /// Begin dragging the selection's fill handle. The block's range is captured now; its values
    /// are read on release, so later mouse moves only move the target.
    pub(super) fn begin_fill_drag(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.state.editable
            || self.state.loading
            || self.state.visible_columns == 0
            || self.display_row_count() == 0
        {
            return;
        }
        if self.cell_editor.is_some() {
            self.commit_editor(cx);
        }
        self.date_picker = None;
        let Some(selection) = self.state.selection.clone() else {
            return;
        };
        let range = selection.active();
        let (start_row, end_row) = range.rows();
        let (start_col, mut end_col) = range.cols();
        end_col = end_col.min(self.state.visible_columns.saturating_sub(1));
        if start_col > end_col {
            return;
        }
        let end_row = end_row.min(self.display_row_count().saturating_sub(1));
        if start_row > end_row {
            return;
        }
        self.fill_drag = Some(FillDrag {
            start_row,
            end_row,
            start_col,
            end_col,
            target_row: end_row,
        });
        self.cell_press = None;
        self.cell_dragged = false;
        self.selecting_cells = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The cell value a fill reads: its staged edit when it has one, otherwise the page's own
    /// value. A pending insert row's unset cell is `NULL`.
    fn fill_source_value(&self, row: usize, col: usize) -> Option<String> {
        match self.staged_value(row, col) {
            Some(value) => value.clone(),
            None => match self.row_at(row) {
                Some(GridRow::Data(data)) => self
                    .state
                    .rows
                    .get(data)
                    .and_then(|values| values.get(self.state.source_col(col)))
                    .and_then(CellValue::as_edit_value),
                _ => None,
            },
        }
    }

    /// Move the fill target with the pointer. Called from the window-level listener registered
    /// while a drag is active, so it also works when the pointer leaves the rows.
    pub(super) fn fill_drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<'_, Self>) {
        let Some(target) = self.fill_target_row(position) else {
            return;
        };
        if let Some(drag) = self.fill_drag.as_mut()
            && drag.target_row != target
        {
            drag.target_row = target;
            // Keep the target row on screen while the pointer is at the viewport edge.
            self.list_scroll
                .scroll_to_item(target, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    /// The display row under a window-space pointer position, clamped to the rows actually
    /// loaded. Unlike [`Self::grid_hit`] this deliberately reports a row for a pointer beyond the
    /// grid's bottom edge, so dragging past the last visible row still targets the last row.
    fn fill_target_row(&self, position: Point<Pixels>) -> Option<usize> {
        let rows = self.display_row_count();
        if rows == 0 {
            return None;
        }
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        if bounds.size.height <= px(0.0) {
            return None;
        }
        let local_y =
            f32::from(position.y) - f32::from(bounds.top()) - f32::from(handle.offset().y);
        let row = (local_y / GRID_ROW_HEIGHT).floor();
        Some(row.clamp(0.0, (rows - 1) as f32) as usize)
    }

    /// Release the fill handle: stage the inferred sequence into every extended row, then leave
    /// the whole filled block selected (Excel's behaviour). Nothing reaches the database here.
    pub(super) fn finish_fill_drag(&mut self, cx: &mut Context<'_, Self>) {
        let Some(drag) = self.fill_drag.take() else {
            return;
        };
        let Some((first, last)) = drag.preview_rows() else {
            cx.notify();
            return;
        };
        let series: Vec<FillSeries> = (drag.start_col..=drag.end_col)
            .map(|col| {
                let column: Vec<Option<String>> = (drag.start_row..=drag.end_row)
                    .map(|row| self.fill_source_value(row, col))
                    .collect();
                FillSeries::build(&column)
            })
            .collect();
        let mut action = Vec::new();
        for row in first..=last {
            let offset = row as isize - drag.start_row as isize;
            for (index, series) in series.iter().enumerate() {
                let col = drag.start_col + index;
                self.stage_fill_cell(row, col, series.value_at(offset), &mut action);
            }
        }
        if !action.is_empty() {
            self.state.redo.clear();
            self.state.undo.push(action);
            if self.state.undo.len() > 256 {
                self.state.undo.remove(0);
            }
        }
        let top = first.min(drag.start_row);
        let bottom = last.max(drag.end_row);
        self.state.selection = Some(CellSelection::single(
            (top, drag.start_col),
            (bottom, drag.end_col),
        ));
        self.cell_press = None;
        self.cell_dragged = false;
        self.selecting_cells = false;
        cx.notify();
    }

    /// Stage one filled value, keeping the edit map free of no-op entries. Filling an empty
    /// string into a numeric/temporal column becomes SQL `NULL`, exactly like a paste.
    fn stage_fill_cell(
        &mut self,
        row: usize,
        col: usize,
        value: Option<String>,
        action: &mut EditAction,
    ) {
        let data_type = self
            .state
            .columns
            .get(col)
            .map(|column| column.data_type.as_str())
            .unwrap_or_default();
        let value = if value.as_deref() == Some("") && empty_paste_is_null(data_type) {
            None
        } else {
            value
        };
        match row_origin(&self.inserts, self.state.rows.len(), row) {
            Some(GridRow::Data(row)) => {
                let previous = self.state.edits.get(&(row, col)).cloned();
                let page_value = self
                    .state
                    .rows
                    .get(row)
                    .and_then(|values| values.get(self.state.source_col(col)))
                    .and_then(CellValue::as_edit_value);
                let current = previous.clone().unwrap_or_else(|| page_value.clone());
                if current == value {
                    return;
                }
                action.push(((row, col), previous));
                if value == page_value {
                    // Filling the page's own value back over a staged edit restores it.
                    self.state.edits.remove(&(row, col));
                } else {
                    self.state.edits.insert((row, col), value);
                }
            }
            Some(GridRow::Insert(index)) => {
                let Some(insert) = self.inserts.get_mut(index) else {
                    return;
                };
                let previous = insert.values.get(&col).cloned();
                if previous.as_ref() == Some(&value) || (previous.is_none() && value.is_none()) {
                    return;
                }
                insert.values.insert(col, value);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    #[test]
    fn single_number_increments_by_one() {
        let series = FillSeries::build(&[text("1")]);
        assert_eq!(series.value_at(1).as_deref(), Some("2"));
        assert_eq!(series.value_at(-1).as_deref(), Some("0"));
    }

    #[test]
    fn number_series_uses_the_last_step() {
        let series = FillSeries::build(&[text("0"), text("1"), text("2")]);
        assert_eq!(series.value_at(3).as_deref(), Some("3"));
        assert_eq!(series.value_at(4).as_deref(), Some("4"));
        assert_eq!(series.value_at(-1).as_deref(), Some("-1"));
    }

    #[test]
    fn leading_zeroes_are_kept() {
        let series = FillSeries::build(&[text("001")]);
        assert_eq!(series.value_at(1).as_deref(), Some("002"));
        assert_eq!(series.value_at(10).as_deref(), Some("011"));
    }

    #[test]
    fn decimal_series_keeps_its_precision() {
        let series = FillSeries::build(&[text("1.5"), text("1.7")]);
        assert_eq!(series.value_at(2).as_deref(), Some("1.9"));
        assert_eq!(series.value_at(3).as_deref(), Some("2.1"));
    }

    #[test]
    fn date_series_advances_a_day() {
        let series = FillSeries::build(&[text("2025-01-30")]);
        assert_eq!(series.value_at(1).as_deref(), Some("2025-01-31"));
        assert_eq!(series.value_at(2).as_deref(), Some("2025-02-01"));
    }

    #[test]
    fn text_repeats_the_source_block() {
        let series = FillSeries::build(&[text("A"), text("B")]);
        assert_eq!(series.value_at(2).as_deref(), Some("A"));
        assert_eq!(series.value_at(3).as_deref(), Some("B"));
        assert_eq!(series.value_at(-1).as_deref(), Some("B"));
    }

    #[test]
    fn nulls_repeat_like_text() {
        let series = FillSeries::build(&[None, text("x")]);
        assert_eq!(series.value_at(2), None);
        assert_eq!(series.value_at(3).as_deref(), Some("x"));
    }
}

//! Source-file readers for RustGrid's Import Wizard.
//!
//! The wizard needs a source's table names (to auto-match fields) and their data rows (to write
//! into the destination table), with every cell rendered as text. The readers live here rather
//! than in the UI so adding another source format is a change to one crate; the app only depends
//! on [`SourceKind`], [`SheetData`] and [`ColumnDef`](rustgrid_core::ColumnDef).
//!
//! Excel workbooks (`.xlsx`, `.xls`, `.xlsm`, `.xlsb`, `.ods`) are read through `calamine` with
//! its `dates` feature, so date-formatted cells arrive as dates instead of serial numbers. CSV
//! (comma) and text (tab) are read with a small delimiter-aware parser that understands RFC 4180
//! quoting; both decode as UTF-8, falling back to GB18030 for legacy Chinese files.

use std::collections::HashSet;
use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};
use rustgrid_core::ColumnDef;

/// A source-file error, rendered to the user by the wizard.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not open the file: {0}")]
    Open(String),

    #[error("could not read the file: {0}")]
    Read(String),
}

pub type Result<T> = std::result::Result<T, ImportError>;

/// The source formats the Import Wizard can read, in the order it offers them. A workbook has one
/// table per worksheet; a delimited text file has a single table named after the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Excel,
    Csv,
    Text,
}

impl SourceKind {
    /// The source's table names: a workbook's worksheets, or the file stem for delimited text.
    pub fn tables(self, path: &Path) -> Result<Vec<String>> {
        match self {
            SourceKind::Excel => sheet_names(path),
            SourceKind::Csv | SourceKind::Text => Ok(vec![file_stem(path)]),
        }
    }

    /// Read one of [`SourceKind::tables`]. For delimited text `table` is only a label and the whole
    /// file is read.
    pub fn read(self, path: &Path, table: &str) -> Result<SheetData> {
        match self {
            SourceKind::Excel => read_sheet(path, table),
            // CSV is defined as comma-separated; TXT has no fixed delimiter, so detect it.
            SourceKind::Csv => read_delimited(path, Some(b',')),
            SourceKind::Text => read_delimited(path, None),
        }
    }
}

/// One source table's header and data rows. Every cell is text; `None` is an empty cell.
#[derive(Debug, Clone, Default)]
pub struct SheetData {
    /// The column names, taken from the first non-empty row. Empty headers are named
    /// `column_N` and duplicates are suffixed so the names are usable as table columns.
    pub headers: Vec<String>,
    /// The data rows, each padded/truncated to `headers.len()`. Fully empty rows are dropped.
    pub rows: Vec<Vec<Option<String>>>,
}

/// The worksheet names of a workbook, in workbook order.
pub fn sheet_names(path: &Path) -> Result<Vec<String>> {
    let workbook =
        open_workbook_auto(path).map_err(|error| ImportError::Open(error.to_string()))?;
    Ok(workbook.sheet_names().to_vec())
}

/// Read one worksheet's header and data rows.
pub fn read_sheet(path: &Path, sheet: &str) -> Result<SheetData> {
    let mut workbook =
        open_workbook_auto(path).map_err(|error| ImportError::Open(error.to_string()))?;
    let range = workbook
        .worksheet_range(sheet)
        .map_err(|error| ImportError::Read(error.to_string()))?;

    let raw = range
        .rows()
        .map(|row| row.iter().map(cell_to_string).collect())
        .collect();
    Ok(build_sheet(raw))
}

/// Read a delimited text file. `delimiter` is `None` to auto-detect the field delimiter.
fn read_delimited(path: &Path, delimiter: Option<u8>) -> Result<SheetData> {
    let bytes = std::fs::read(path).map_err(|error| ImportError::Open(error.to_string()))?;
    let text = decode_text(&bytes);
    let delimiter = delimiter.unwrap_or_else(|| detect_delimiter(&text));
    Ok(build_sheet(parse_delimited(&text, delimiter)))
}

/// The candidates tried when a text file's field delimiter is not fixed.
const DELIMITER_CANDIDATES: [u8; 4] = *b"\t,;|";
/// Non-empty lines sampled when detecting the delimiter, keeping detection cheap on large files.
const DELIMITER_SAMPLE_LINES: usize = 50;

/// Guess a text file's field delimiter by trying each candidate and keeping the one that splits
/// the sample into the most consistent number of columns per line. Falls back to a tab, so a
/// single-column file stays one column.
fn detect_delimiter(text: &str) -> u8 {
    let sample: Vec<&str> = text
        .split(['\r', '\n'])
        .filter(|line| !line.trim().is_empty())
        .take(DELIMITER_SAMPLE_LINES)
        .collect();
    if sample.is_empty() {
        return b'\t';
    }
    let sample = sample.join("\n");

    let mut best = (b'\t', 0usize);
    for candidate in DELIMITER_CANDIDATES {
        // Count lines per column count, ignoring single-column lines (no delimiter of this kind).
        let mut counts: Vec<usize> = parse_delimited(&sample, candidate)
            .iter()
            .map(Vec::len)
            .filter(|&columns| columns > 1)
            .collect();
        if counts.is_empty() {
            continue;
        }
        counts.sort_unstable();
        let (mut modal, mut modal_count) = (0usize, 0usize);
        let mut index = 0;
        while index < counts.len() {
            let mut end = index;
            while end < counts.len() && counts[end] == counts[index] {
                end += 1;
            }
            if end - index > modal_count {
                modal_count = end - index;
                modal = counts[index];
            }
            index = end;
        }
        let score = modal_count * modal;
        if score > best.1 {
            best = (candidate, score);
        }
    }
    best.0
}

/// Decode file bytes as UTF-8, falling back to GB18030 so legacy Chinese CSV/TXT files stay
/// readable. A UTF-8 BOM is stripped.
fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

/// Parse delimited text into rows of optional cells. Understands RFC 4180 quoting (`"` quoting,
/// `""` escapes, embedded delimiters/newlines) and treats an empty field as `None` (SQL `NULL`).
fn parse_delimited(text: &str, delimiter: u8) -> Vec<Vec<Option<String>>> {
    let delimiter = delimiter as char;
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut row: Vec<Option<String>> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        if in_quotes {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(character);
            }
        } else if character == '"' && field.is_empty() {
            in_quotes = true;
        } else if character == delimiter {
            row.push(take_cell(&mut field));
        } else if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(take_cell(&mut field));
            rows.push(std::mem::take(&mut row));
        } else if character == '\n' {
            row.push(take_cell(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(character);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(take_cell(&mut field));
        rows.push(row);
    }
    rows
}

/// Take the current field as a cell, mapping an empty field to `None`.
fn take_cell(field: &mut String) -> Option<String> {
    if field.is_empty() {
        None
    } else {
        Some(std::mem::take(field))
    }
}

/// The table name a delimited file imports as: its file stem, else `import`.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or_else(|| "import".to_string())
}

/// Turn raw rows into a [`SheetData`]: the first non-empty row is the header, later rows are data,
/// padded/truncated to the header width, with fully empty rows dropped.
fn build_sheet(mut raw: Vec<Vec<Option<String>>>) -> SheetData {
    let mut iter = raw.drain(..);
    let headers = loop {
        match iter.next() {
            Some(row) if row.iter().any(Option::is_some) => break normalise_headers(row),
            Some(_) => continue,
            None => return SheetData::default(),
        }
    };

    let mut rows = Vec::new();
    for mut row in iter {
        row.resize(headers.len(), None);
        row.truncate(headers.len());
        if row.iter().all(Option::is_none) {
            continue;
        }
        rows.push(row);
    }
    SheetData { headers, rows }
}

/// Turn a raw header row into usable column names: blank cells become `column_N` and duplicates
/// get a numeric suffix, so the names can be used verbatim as table columns.
fn normalise_headers(raw: Vec<Option<String>>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    raw.into_iter()
        .enumerate()
        .map(|(index, value)| {
            let mut name = value
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("column_{}", index + 1));
            if !seen.insert(name.clone()) {
                let mut suffix = 2;
                loop {
                    let candidate = format!("{name}_{suffix}");
                    if seen.insert(candidate.clone()) {
                        name = candidate;
                        break;
                    }
                    suffix += 1;
                }
            }
            name
        })
        .collect()
}

/// Render one cell as text. Dates become `YYYY-MM-DD` (date-only) or `YYYY-MM-DD HH:MM:SS`;
/// booleans become `1`/`0` so they read as numbers into any destination column.
fn cell_to_string(cell: &Data) -> Option<String> {
    match cell {
        Data::Empty => None,
        Data::String(value) => {
            if value.is_empty() {
                None
            } else {
                Some(value.clone())
            }
        }
        Data::Int(value) => Some(value.to_string()),
        Data::Float(value) => Some(format_float(*value)),
        Data::Bool(value) => Some(if *value { "1" } else { "0" }.to_string()),
        Data::DateTime(value) => Some(excel_datetime_to_string(value)),
        Data::DateTimeIso(value) => Some(value.clone()),
        Data::DurationIso(value) => Some(value.clone()),
        Data::Error(_) => None,
    }
}

/// Format a float without a trailing `.0` for whole numbers.
fn format_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// Format an Excel date/time cell. Time-only cells (`[hh]:mm:ss`) render as `HH:MM:SS`.
fn excel_datetime_to_string(value: &calamine::ExcelDateTime) -> String {
    if value.is_duration() {
        let seconds = (value.as_f64() * 86_400.0).round() as i64;
        let seconds = seconds.rem_euclid(86_400);
        return format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        );
    }
    let (year, month, day, hour, minute, second, _milli) = value.to_ymd_hms_milli();
    if hour == 0 && minute == 0 && second == 0 {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
    }
}

/// Rows sampled per column when inferring a destination type, so a large sheet does not scan
/// every value on the mapping page.
const INFER_SAMPLE_ROWS: usize = 200;

/// The coarse type inferred for one source column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnKind {
    Integer,
    Decimal,
    Date,
    DateTime,
    Bool,
    Text,
}

/// Infer a destination [`ColumnDef`] per source column from a worksheet's data, for a table the
/// wizard creates. Every column is nullable; the caller marks a primary key if it wants one.
pub fn infer_columns(headers: &[String], rows: &[Vec<Option<String>>]) -> Vec<ColumnDef> {
    headers
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let values = rows
                .iter()
                .take(INFER_SAMPLE_ROWS)
                .filter_map(|row| row.get(index))
                .filter_map(|cell| cell.as_deref());
            let (kind, max_length) = infer_kind(values);
            let mut column = ColumnDef {
                name: name.clone(),
                nullable: true,
                ..Default::default()
            };
            match kind {
                ColumnKind::Integer => column.data_type = "bigint".to_string(),
                ColumnKind::Decimal => {
                    column.data_type = "decimal".to_string();
                    column.length = "20".to_string();
                    column.decimals = "6".to_string();
                }
                ColumnKind::Date => column.data_type = "date".to_string(),
                ColumnKind::DateTime => column.data_type = "datetime".to_string(),
                ColumnKind::Bool => {
                    column.data_type = "tinyint".to_string();
                    column.length = "1".to_string();
                }
                ColumnKind::Text => {
                    if max_length <= 255 {
                        column.data_type = "varchar".to_string();
                        column.length = "255".to_string();
                    } else {
                        column.data_type = "longtext".to_string();
                    }
                }
            }
            column
        })
        .collect()
}

/// Classify a set of sample values: all integers, all decimals, all dates/datetimes, all
/// booleans, or text. An empty sample is text. Also returns the longest value so text columns can
/// choose between `varchar` and `longtext`.
fn infer_kind<'a>(values: impl Iterator<Item = &'a str>) -> (ColumnKind, usize) {
    let mut any = false;
    let mut integer = true;
    let mut decimal = true;
    let mut boolean = true;
    let mut date_only = true;
    let mut datetime = true;
    let mut max_length = 0usize;

    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        any = true;
        max_length = max_length.max(value.chars().count());
        if integer && !is_integer(value) {
            integer = false;
        }
        if decimal && !is_decimal(value) {
            decimal = false;
        }
        if boolean && !is_bool(value) {
            boolean = false;
        }
        match datetime_kind(value) {
            Some(true) => datetime = false,
            Some(false) => date_only = false,
            None => {
                date_only = false;
                datetime = false;
            }
        }
    }

    if !any {
        return (ColumnKind::Text, max_length);
    }
    let kind = if integer {
        ColumnKind::Integer
    } else if decimal {
        ColumnKind::Decimal
    } else if date_only {
        ColumnKind::Date
    } else if datetime {
        ColumnKind::DateTime
    } else if boolean {
        ColumnKind::Bool
    } else {
        ColumnKind::Text
    };
    (kind, max_length)
}

/// Whether `value` is an integer that can be stored losslessly. Values with a leading zero
/// (`007`, postal codes) stay text so the digits survive.
fn is_integer(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || digits.len() > 19 {
        return false;
    }
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return false;
    }
    value.parse::<i64>().is_ok()
}

/// Whether `value` parses as a decimal number. A leading `+` or a leading-zero integer is
/// treated as text (phone numbers, codes) rather than a number.
fn is_decimal(value: &str) -> bool {
    if value.starts_with('+') {
        return false;
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    let digit_only = digits.bytes().all(|byte| byte.is_ascii_digit());
    if digit_only && digits.len() > 1 && digits.starts_with('0') {
        return false;
    }
    value.parse::<f64>().is_ok_and(f64::is_finite)
}

/// Whether `value` is a boolean literal.
fn is_bool(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no"
    )
}

/// Classify a date-like value: `Some(true)` for date only, `Some(false)` for date+time, `None`
/// when it is not a date. Accepts `YYYY-MM-DD[ HH:MM[:SS]]` and `YYYY/MM/DD` with `:`/`-` times.
fn datetime_kind(value: &str) -> Option<bool> {
    let (date, time) = match value.split_once(['T', ' ']) {
        Some((date, time)) => (date, Some(time)),
        None => (value, None),
    };
    if !is_date(date) {
        return None;
    }
    match time {
        Some(time) if is_time(time) => Some(false),
        Some(_) => None,
        None => Some(true),
    }
}

/// Whether `value` is `YYYY-MM-DD` or `YYYY/MM/DD` with a real calendar date.
fn is_date(value: &str) -> bool {
    if value.starts_with(['-', '+', '/']) {
        return false;
    }
    let separator = if value.contains('/') { "/" } else { "-" };
    let mut parts = value.split(separator);
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if year.len() != 4 || month.is_empty() || day.is_empty() {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<i32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    chrono::NaiveDate::from_ymd_opt(year, month, day).is_some()
}

/// Whether `value` is `HH:MM` or `HH:MM:SS`, optionally with trailing fractional seconds and a
/// `Z` suffix.
fn is_time(value: &str) -> bool {
    let value = value.trim_end_matches('Z');
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return false;
    }
    parts.iter().all(|part| {
        let (whole, fraction) = part.split_once('.').unwrap_or((part, ""));
        !whole.is_empty()
            && whole.bytes().all(|byte| byte.is_ascii_digit())
            && (fraction.is_empty() || fraction.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_duplicate_headers_are_made_unique() {
        let headers = normalise_headers(vec![
            None,
            Some("  a  ".to_string()),
            Some("a".to_string()),
            Some(String::new()),
        ]);
        assert_eq!(headers, vec!["column_1", "a", "a_2", "column_4"]);
    }

    #[test]
    fn infers_numeric_date_and_text_columns() {
        let headers: Vec<String> = ["id", "price", "when", "code"]
            .iter()
            .map(|name| name.to_string())
            .collect();
        let cell = |value: &str| Some(value.to_string());
        let rows = vec![
            vec![cell("1"), cell("1.5"), cell("2024-01-02"), cell("007")],
            vec![cell("2"), cell("2.25"), cell("2024-03-04"), cell("x")],
        ];
        let columns = infer_columns(&headers, &rows);
        assert_eq!(columns[0].data_type, "bigint");
        assert_eq!(columns[1].data_type, "decimal");
        assert_eq!(columns[2].data_type, "date");
        // A leading-zero code stays text even though the other values are mixed.
        assert_eq!(columns[3].data_type, "varchar");
        assert!(columns.iter().all(|column| column.nullable));
    }

    #[test]
    fn an_empty_column_is_text() {
        let columns = infer_columns(&["note".to_string()], &[vec![None], vec![None]]);
        assert_eq!(columns[0].data_type, "varchar");
    }

    #[test]
    fn datetime_and_boolean_are_detected() {
        let headers: Vec<String> = ["at", "flag"].iter().map(|name| name.to_string()).collect();
        let rows = vec![
            vec![
                Some("2024-01-02 03:04:05".to_string()),
                Some("true".to_string()),
            ],
            vec![
                Some("2024-01-03 04:05:06".to_string()),
                Some("false".to_string()),
            ],
        ];
        let columns = infer_columns(&headers, &rows);
        assert_eq!(columns[0].data_type, "datetime");
        assert_eq!(columns[1].data_type, "tinyint");
        assert_eq!(columns[1].length, "1");
    }

    #[test]
    fn csv_parses_quotes_escapes_and_empty_cells() {
        let rows = parse_delimited(
            "id,name,note\r\n1,\"a,b\",\r\n2,\"say \"\"hi\"\"\",x\n",
            b',',
        );
        assert_eq!(
            rows,
            vec![
                vec![
                    Some("id".to_string()),
                    Some("name".to_string()),
                    Some("note".to_string())
                ],
                vec![Some("1".to_string()), Some("a,b".to_string()), None],
                vec![
                    Some("2".to_string()),
                    Some("say \"hi\"".to_string()),
                    Some("x".to_string())
                ],
            ]
        );
    }

    #[test]
    fn tab_delimited_keeps_commas_inside_fields() {
        let rows = parse_delimited("id\tname\n1\ta,b\n", b'\t');
        assert_eq!(
            rows,
            vec![
                vec![Some("id".to_string()), Some("name".to_string())],
                vec![Some("1".to_string()), Some("a,b".to_string())],
            ]
        );
    }

    #[test]
    fn delimited_sheet_uses_first_row_as_header() {
        let sheet = build_sheet(parse_delimited("\nid,name\n1,a\n\n2,b\n", b','));
        assert_eq!(sheet.headers, vec!["id", "name"]);
        assert_eq!(sheet.rows.len(), 2);
        assert_eq!(sheet.rows[0][0], Some("1".to_string()));
    }

    #[test]
    fn file_stem_is_the_table_name() {
        assert_eq!(file_stem(Path::new("/tmp/Report.csv")), "Report");
        assert_eq!(file_stem(Path::new("")), "import");
    }

    #[test]
    fn detects_common_field_delimiters() {
        assert_eq!(detect_delimiter("a\tb\tc\n1\t2\t3\n"), b'\t');
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n"), b',');
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n"), b';');
        assert_eq!(detect_delimiter("a|b|c\n1|2|3\n"), b'|');
    }

    #[test]
    fn detection_ignores_delimiters_inside_quotes() {
        // Commas inside tab-separated fields must not make comma win.
        assert_eq!(detect_delimiter("\"a,b\"\tc\n\"d,e\"\tf\n"), b'\t');
        // Tabs inside comma-separated fields must not make tab win.
        assert_eq!(detect_delimiter("\"a\tb\",c\n\"d\te\",f\n"), b',');
    }

    #[test]
    fn single_column_text_defaults_to_tab() {
        assert_eq!(detect_delimiter("name\nalice\nbob\n"), b'\t');
    }

    #[test]
    fn every_line_ending_style_is_understood() {
        for text in ["a,b\n1,2\n", "a,b\r\n1,2\r\n", "a,b\r1,2\r"] {
            let sheet = build_sheet(parse_delimited(text, b','));
            assert_eq!(sheet.headers, vec!["a", "b"], "for {text:?}");
            assert_eq!(sheet.rows, vec![vec![Some("1".into()), Some("2".into())]]);
        }
    }
}

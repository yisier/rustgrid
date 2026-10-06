//! Table-data export writers for RustGrid: `.xlsx`, `.csv`, `.sql` and `.txt`.
//!
//! The app decodes table rows into engine-agnostic [`CellValue`]s through
//! [`rustgrid_core::Connection`] and feeds them in pages, so memory stays bounded for every
//! format except `.xlsx` (rust_xlsxwriter has no streaming writer and buffers the workbook).
//! CSV, SQL and text stream straight through a [`BufWriter`].
//!
//! Formatting lives here rather than in the UI so a new output format is a change to one crate.
//! The SQL writer is parameterised by [`SqlDialect`] because identifier quoting and string
//! escaping differ per engine; only MySQL is implemented today (the one compiled-in driver).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use rust_xlsxwriter::{Format, Workbook, Worksheet, XlsxError};
use rustgrid_core::CellValue;

/// Cell strings Excel accepts; longer ones would make `write_string` fail.
const XLSX_MAX_CELL_CHARS: usize = 32_767;
/// Rows per `INSERT` statement in the SQL writer.
const SQL_BATCH_ROWS: usize = 500;
/// Byte budget per `INSERT` statement, keeping it under the server's `max_allowed_packet`.
const SQL_BATCH_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("export I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("spreadsheet error: {0}")]
    Xlsx(#[from] XlsxError),
}

pub type Result<T> = std::result::Result<T, ExportError>;

/// The output file kinds offered by the export wizard's first page, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Xlsx,
    Csv,
    Sql,
    Txt,
}

impl ExportFormat {
    /// Every format, in the order P1 presents them.
    pub const ALL: [ExportFormat; 4] = [
        ExportFormat::Xlsx,
        ExportFormat::Csv,
        ExportFormat::Sql,
        ExportFormat::Txt,
    ];

    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Xlsx => "xlsx",
            ExportFormat::Csv => "csv",
            ExportFormat::Sql => "sql",
            ExportFormat::Txt => "txt",
        }
    }

    /// The i18n key of the format's P1 label.
    pub fn label_key(self) -> &'static str {
        match self {
            ExportFormat::Xlsx => "export.format.xlsx",
            ExportFormat::Csv => "export.format.csv",
            ExportFormat::Sql => "export.format.sql",
            ExportFormat::Txt => "export.format.txt",
        }
    }
}

/// The per-run options gathered on the wizard's options page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    /// Whether the header row (CSV/TXT/XLSX) or column list (SQL) is included.
    pub include_header: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_header: true,
        }
    }
}

/// How a SQL script quotes identifiers and string literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlDialect {
    #[default]
    MySql,
    Postgres,
}

impl SqlDialect {
    fn quote_identifier(self, name: &str) -> String {
        match self {
            SqlDialect::MySql => format!("`{}`", name.replace('`', "``")),
            SqlDialect::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// Render one value as a SQL literal. `BIT` columns accept the hex form for byte values.
    fn literal(self, value: &CellValue) -> String {
        match self {
            SqlDialect::MySql => match value {
                CellValue::Null => "NULL".to_string(),
                CellValue::Bool(value) => u8::from(*value).to_string(),
                CellValue::Int(value) => value.to_string(),
                CellValue::Uint(value) => value.to_string(),
                CellValue::Float(value) => {
                    if value.is_finite() {
                        value.to_string()
                    } else {
                        "NULL".to_string()
                    }
                }
                CellValue::Text(text) => quote_mysql_string(text),
                CellValue::Bytes(bytes) => {
                    let mut hex = String::with_capacity(bytes.len() * 2 + 2);
                    hex.push_str("0x");
                    for byte in bytes {
                        hex.push_str(&format!("{byte:02X}"));
                    }
                    hex
                }
            },
            SqlDialect::Postgres => match value {
                CellValue::Null => "NULL".to_string(),
                CellValue::Bool(value) => {
                    if *value {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }
                }
                CellValue::Int(value) => value.to_string(),
                CellValue::Uint(value) => value.to_string(),
                CellValue::Float(value) => {
                    if value.is_finite() {
                        value.to_string()
                    } else {
                        "NULL".to_string()
                    }
                }
                CellValue::Text(text) => quote_postgres_string(text),
                CellValue::Bytes(bytes) => {
                    let mut hex = String::with_capacity(bytes.len() * 2 + 3);
                    hex.push_str("'\\x");
                    for byte in bytes {
                        hex.push_str(&format!("{byte:02x}"));
                    }
                    hex.push('\'');
                    hex
                }
            },
        }
    }
}

/// Escape a string as a PostgreSQL standard-conforming single-quoted literal.
fn quote_postgres_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Escape a string as a MySQL single-quoted literal.
fn quote_mysql_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        match character {
            '\0' => out.push_str("\\0"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{1a}' => out.push_str("\\Z"),
            _ => out.push(character),
        }
    }
    out.push('\'');
    out
}

/// A streaming writer for one table's data. Callers feed pages of decoded rows through
/// [`TableWriter::write_rows`] and seal the file with [`TableWriter::finish`].
pub struct TableWriter {
    inner: Inner,
}

enum Inner {
    Xlsx(Box<XlsxSink>),
    Csv(TextSink),
    Txt(TextSink),
    Sql(SqlSink),
}

impl TableWriter {
    /// Create the output file and write any header the format/options call for.
    pub fn create(
        path: &Path,
        table: &str,
        columns: &[String],
        format: ExportFormat,
        options: ExportOptions,
        dialect: SqlDialect,
    ) -> Result<Self> {
        let inner = match format {
            ExportFormat::Xlsx => Inner::Xlsx(Box::new(XlsxSink::create(
                path,
                table,
                columns,
                options.include_header,
            )?)),
            ExportFormat::Csv => Inner::Csv(TextSink::create(
                path,
                columns,
                options.include_header,
                ',',
                false,
            )?),
            ExportFormat::Txt => Inner::Txt(TextSink::create(
                path,
                columns,
                options.include_header,
                '\t',
                true,
            )?),
            ExportFormat::Sql => Inner::Sql(SqlSink::create(
                path,
                table,
                columns,
                options.include_header,
                dialect,
            )?),
        };
        Ok(Self { inner })
    }

    /// Append a page of rows.
    pub fn write_rows(&mut self, rows: &[Vec<CellValue>]) -> Result<()> {
        match &mut self.inner {
            Inner::Xlsx(sink) => sink.write_rows(rows),
            Inner::Csv(sink) => sink.write_rows(rows),
            Inner::Txt(sink) => sink.write_rows(rows),
            Inner::Sql(sink) => sink.write_rows(rows),
        }
    }

    /// Flush the remaining buffered data and close the file.
    pub fn finish(self) -> Result<()> {
        match self.inner {
            Inner::Xlsx(sink) => (*sink).finish(),
            Inner::Csv(sink) => sink.finish(),
            Inner::Txt(sink) => sink.finish(),
            Inner::Sql(sink) => sink.finish(),
        }
    }
}

/// Write a `.xlsx` workbook. rust_xlsxwriter buffers every row until `save`, so this is the only
/// sink whose memory use grows with the table.
struct XlsxSink {
    path: PathBuf,
    workbook: Workbook,
    sheet: usize,
    next_row: u32,
}

impl XlsxSink {
    fn create(path: &Path, table: &str, columns: &[String], include_header: bool) -> Result<Self> {
        let mut workbook = Workbook::new();
        let next_row = {
            let worksheet = workbook.add_worksheet();
            worksheet.set_name(sheet_name(table))?;
            if include_header {
                let header = Format::new().set_bold();
                for (col, name) in columns.iter().enumerate() {
                    worksheet.write_string_with_format(0, col as u16, name, &header)?;
                }
                1
            } else {
                0
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            workbook,
            sheet: 0,
            next_row,
        })
    }

    fn write_rows(&mut self, rows: &[Vec<CellValue>]) -> Result<()> {
        let worksheet = self.workbook.worksheet_from_index(self.sheet)?;
        for row in rows {
            for (col, value) in row.iter().enumerate() {
                write_xlsx_cell(worksheet, self.next_row, col, value)?;
            }
            self.next_row += 1;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.workbook.worksheet_from_index(self.sheet)?.autofit();
        self.workbook.save(&self.path)?;
        Ok(())
    }
}

/// Write one cell, choosing the Excel type from the decoded value. `NULL` cells are left blank.
fn write_xlsx_cell(
    worksheet: &mut Worksheet,
    row: u32,
    col: usize,
    value: &CellValue,
) -> Result<()> {
    let col = col as u16;
    match value {
        CellValue::Null => {}
        CellValue::Bool(value) => {
            worksheet.write_boolean(row, col, *value)?;
        }
        CellValue::Int(value) => {
            worksheet.write_number(row, col, *value as f64)?;
        }
        CellValue::Uint(value) => {
            worksheet.write_number(row, col, *value as f64)?;
        }
        CellValue::Float(value) => {
            if value.is_finite() {
                worksheet.write_number(row, col, *value)?;
            }
        }
        CellValue::Text(value) => {
            worksheet.write_string(row, col, truncate(value))?;
        }
        CellValue::Bytes(_) => {
            worksheet.write_string(row, col, truncate(&value.as_display()))?;
        }
    }
    Ok(())
}

/// An Excel sheet name: invalid characters replaced, capped at 31 characters.
fn sheet_name(table: &str) -> String {
    let cleaned: String = table
        .chars()
        .map(|character| {
            if "[]:*?/\\".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    let name = if trimmed.is_empty() {
        "Sheet1".to_string()
    } else {
        trimmed.to_string()
    };
    name.chars().take(31).collect()
}

fn truncate(value: &str) -> String {
    value.chars().take(XLSX_MAX_CELL_CHARS).collect()
}

/// Write delimiter-separated text (CSV with RFC 4180 quoting, or tab-separated plain text).
struct TextSink {
    out: BufWriter<File>,
    delimiter: char,
    /// Plain text keeps one record per line, so it replaces rather than quotes special characters.
    plain: bool,
}

impl TextSink {
    fn create(
        path: &Path,
        columns: &[String],
        include_header: bool,
        delimiter: char,
        plain: bool,
    ) -> Result<Self> {
        let mut sink = Self {
            out: file_writer(path)?,
            delimiter,
            plain,
        };
        if include_header {
            let cells: Vec<String> = columns.to_vec();
            sink.write_row(&cells)?;
        }
        Ok(sink)
    }

    fn write_rows(&mut self, rows: &[Vec<CellValue>]) -> Result<()> {
        for row in rows {
            let cells: Vec<String> = row.iter().map(text_cell).collect();
            self.write_row(&cells)?;
        }
        Ok(())
    }

    fn write_row(&mut self, cells: &[String]) -> Result<()> {
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                write!(self.out, "{}", self.delimiter)?;
            }
            if self.plain {
                write!(self.out, "{}", escape_plain(cell))?;
            } else {
                write!(self.out, "{}", escape_csv(cell, self.delimiter))?;
            }
        }
        writeln!(self.out)?;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.out.flush()?;
        Ok(())
    }
}

/// RFC 4180 CSV escaping: quote when the field contains the delimiter, a quote or a line break.
fn escape_csv(value: &str, delimiter: char) -> String {
    if value.contains(delimiter)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
    {
        let escaped = value.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

/// Keep one plain-text record on one line by replacing the delimiter and line breaks with spaces.
fn escape_plain(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

/// The text representation of a cell for CSV/TXT: `NULL` is an empty field, everything else is its
/// display form (bytes as uppercase hex).
fn text_cell(value: &CellValue) -> String {
    match value {
        CellValue::Null => String::new(),
        other => other.as_display(),
    }
}

/// Write an engine-flavoured SQL script: a table comment, then batched multi-row `INSERT`s.
struct SqlSink {
    out: BufWriter<File>,
    table: String,
    columns: Vec<String>,
    dialect: SqlDialect,
    include_columns: bool,
    batch: Vec<String>,
    batch_bytes: usize,
}

impl SqlSink {
    fn create(
        path: &Path,
        table: &str,
        columns: &[String],
        include_columns: bool,
        dialect: SqlDialect,
    ) -> Result<Self> {
        let mut out = file_writer(path)?;
        writeln!(out, "-- Table: {table}")?;
        Ok(Self {
            out,
            table: table.to_string(),
            columns: columns.to_vec(),
            dialect,
            include_columns,
            batch: Vec::new(),
            batch_bytes: 0,
        })
    }

    fn write_rows(&mut self, rows: &[Vec<CellValue>]) -> Result<()> {
        for row in rows {
            let values: Vec<String> = row
                .iter()
                .map(|value| self.dialect.literal(value))
                .collect();
            let literal = format!("({})", values.join(", "));
            let bytes = literal.len() + 2;
            let full = !self.batch.is_empty()
                && (self.batch.len() >= SQL_BATCH_ROWS
                    || self.batch_bytes + bytes > SQL_BATCH_BYTES);
            if full {
                self.flush()?;
            }
            self.batch.push(literal);
            self.batch_bytes += bytes;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let target = self.dialect.quote_identifier(&self.table);
        if self.include_columns && !self.columns.is_empty() {
            let columns = self
                .columns
                .iter()
                .map(|column| self.dialect.quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(self.out, "INSERT INTO {target} ({columns}) VALUES")?;
        } else {
            writeln!(self.out, "INSERT INTO {target} VALUES")?;
        }
        writeln!(self.out, "{}", self.batch.join(",\n"))?;
        writeln!(self.out, ";")?;
        self.batch.clear();
        self.batch_bytes = 0;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.flush()?;
        self.out.flush()?;
        Ok(())
    }
}

fn file_writer(path: &Path) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(File::create(path)?))
}

/// Derive a default output path for `table` in `directory`.
pub fn default_output_path(directory: &str, table: &str, format: ExportFormat) -> String {
    let file = format!("{}.{}", sanitize_file_stem(table), format.extension());
    Path::new(directory)
        .join(file)
        .to_string_lossy()
        .into_owned()
}

/// Strip path separators and other characters that cannot appear in a file name.
pub fn sanitize_file_stem(table: &str) -> String {
    let cleaned: String = table
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => character,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "table".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(extension: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustgrid-export-{unique}.{extension}"))
    }

    fn columns() -> Vec<String> {
        vec!["id".to_string(), "name".to_string()]
    }

    fn rows() -> Vec<Vec<CellValue>> {
        vec![
            vec![CellValue::Int(1), CellValue::Text("a,b".to_string())],
            vec![CellValue::Null, CellValue::Text("say \"hi\"".to_string())],
        ]
    }

    fn write(format: ExportFormat, options: ExportOptions) -> (PathBuf, String) {
        let path = temp_file(format.extension());
        let mut writer = TableWriter::create(
            &path,
            "users",
            &columns(),
            format,
            options,
            SqlDialect::MySql,
        )
        .unwrap();
        writer.write_rows(&rows()).unwrap();
        writer.finish().unwrap();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        (path, text)
    }

    #[test]
    fn csv_quotes_and_headers() {
        let (path, text) = write(ExportFormat::Csv, ExportOptions::default());
        assert_eq!(text, "id,name\n1,\"a,b\"\n,\"say \"\"hi\"\"\"\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn csv_without_headers() {
        let (path, text) = write(
            ExportFormat::Csv,
            ExportOptions {
                include_header: false,
            },
        );
        assert_eq!(text.lines().count(), 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn sql_batches_and_escapes() {
        let (path, text) = write(ExportFormat::Sql, ExportOptions::default());
        assert!(text.contains("-- Table: users"));
        assert!(text.contains("INSERT INTO `users` (`id`, `name`) VALUES"));
        assert!(text.contains("(1, 'a,b')"));
        assert!(text.contains("(NULL, 'say \\\"hi\\\"')"));
        assert!(text.trim_end().ends_with(';'));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn txt_is_tab_delimited_and_single_line() {
        let path = temp_file("txt");
        let mut writer = TableWriter::create(
            &path,
            "users",
            &columns(),
            ExportFormat::Txt,
            ExportOptions::default(),
            SqlDialect::MySql,
        )
        .unwrap();
        writer
            .write_rows(&[vec![
                CellValue::Int(1),
                CellValue::Text("a\tb\nc".to_string()),
            ]])
            .unwrap();
        writer.finish().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "id\tname\n1\ta b c\n"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn xlsx_writes_a_non_empty_workbook() {
        let path = temp_file("xlsx");
        let mut writer = TableWriter::create(
            &path,
            "users",
            &columns(),
            ExportFormat::Xlsx,
            ExportOptions::default(),
            SqlDialect::MySql,
        )
        .unwrap();
        writer.write_rows(&rows()).unwrap();
        writer.finish().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn default_paths_are_sanitized() {
        let path = default_output_path("C:\\out", "a/b", ExportFormat::Csv);
        assert!(path.ends_with("a_b.csv"));
    }
}

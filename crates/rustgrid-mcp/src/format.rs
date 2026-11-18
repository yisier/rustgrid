use std::time::Duration;

use rustgrid_core::{CellValue, QueryResult};

pub const MAX_PAYLOAD_BYTES: usize = 200 * 1024; // 200 KB

/// Formats a QueryResult into a clean Markdown table with an execution header.
pub fn format_query_result_markdown(
    connection_name: &str,
    database_name: &str,
    result: &QueryResult,
    max_rows: usize,
    elapsed: Duration,
) -> String {
    let mut out = String::new();
    let total_rows = result.rows.len();
    let (display_rows, is_truncated) = if max_rows == 0 {
        (total_rows, false)
    } else {
        (total_rows.min(max_rows), total_rows > max_rows)
    };

    let header_info = format!(
        "[Connection: {} | Database: {} | Rows: {}{} | Duration: {}ms]\n\n",
        connection_name,
        database_name,
        display_rows,
        if is_truncated {
            format!(" / {}", total_rows)
        } else {
            String::new()
        },
        elapsed.as_millis()
    );
    out.push_str(&header_info);

    if result.columns.is_empty() {
        if result.rows_affected > 0 {
            out.push_str(&format!(
                "Query executed. Affected rows: {}\n",
                result.rows_affected
            ));
        } else {
            out.push_str("Empty result set.\n");
        }
        return out;
    }

    // Markdown Table Header
    out.push('|');
    for col in &result.columns {
        out.push(' ');
        out.push_str(&escape_cell(&col.name));
        out.push_str(" |");
    }
    out.push('\n');

    // Separator
    out.push('|');
    for _ in &result.columns {
        out.push_str("---|");
    }
    out.push('\n');

    // Rows
    for (i, row) in result.rows.iter().take(display_rows).enumerate() {
        if out.len() >= MAX_PAYLOAD_BYTES {
            out.push_str(&format!(
                "\n*Note: Output truncated at row {} due to payload size limits.*\n",
                i
            ));
            return out;
        }

        out.push('|');
        for cell in row {
            out.push(' ');
            out.push_str(&format_cell_value(cell));
            out.push_str(" |");
        }
        out.push('\n');
    }

    if is_truncated {
        out.push_str(&format!(
            "\n*Note: Results were capped at {} rows (total returned: {}). Add explicit LIMIT or WHERE clauses to inspect specific records.*\n",
            display_rows, total_rows
        ));
    }

    out
}

pub fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn format_cell_value(cell: &CellValue) -> String {
    match cell {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Int(i) => i.to_string(),
        CellValue::Uint(u) => u.to_string(),
        CellValue::Float(f) => f.to_string(),
        CellValue::Text(t) => escape_cell(t),
        CellValue::Bytes(b) => format!("<binary: {} bytes>", b.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgrid_core::ColumnInfo;

    #[test]
    fn test_format_empty_result() {
        let res = QueryResult {
            statement: "SELECT 1".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: 0,
            has_result_set: false,
            last_insert_id: None,
        };
        let md = format_query_result_markdown(
            "test_conn",
            "test_db",
            &res,
            50,
            Duration::from_millis(5),
        );
        assert!(
            md.contains("[Connection: test_conn | Database: test_db | Rows: 0 | Duration: 5ms]")
        );
        assert!(md.contains("Empty result set."));

        let res_affected = QueryResult {
            statement: "UPDATE users".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: 3,
            has_result_set: false,
            last_insert_id: None,
        };
        let md_affected = format_query_result_markdown(
            "test_conn",
            "test_db",
            &res_affected,
            50,
            Duration::from_millis(10),
        );
        assert!(md_affected.contains("Affected rows: 3"));
    }

    #[test]
    fn test_format_table_result() {
        let res = QueryResult {
            statement: "SELECT id, name FROM users".to_string(),
            columns: vec![
                ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                    nullable: false,
                    primary_key: true,
                    auto_increment: false,
                    generated: false,
                    comment: String::new(),
                },
                ColumnInfo {
                    name: "name".to_string(),
                    data_type: "varchar".to_string(),
                    nullable: true,
                    primary_key: false,
                    auto_increment: false,
                    generated: false,
                    comment: String::new(),
                },
            ],
            rows: vec![
                vec![CellValue::Int(1), CellValue::Text("Alice".to_string())],
                vec![
                    CellValue::Int(2),
                    CellValue::Text("Bob|Builder".to_string()),
                ],
            ],
            rows_affected: 2,
            has_result_set: true,
            last_insert_id: None,
        };
        let md =
            format_query_result_markdown("my_conn", "my_db", &res, 50, Duration::from_millis(15));
        assert!(md.contains("| id | name |"));
        assert!(md.contains("|---|---|"));
        assert!(md.contains("| 1 | Alice |"));
        assert!(md.contains("| 2 | Bob\\|Builder |"));
    }
}

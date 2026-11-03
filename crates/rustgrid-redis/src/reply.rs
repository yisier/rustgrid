//! Turning a Redis reply into the engine-agnostic [`QueryResult`] the grid renders.
//!
//! A reply is either a scalar (a status line, an integer, a bulk string) or a nested array. Scalar
//! replies become a one-row `value` grid; an array of scalars becomes one row per element; an array
//! of arrays becomes `index`/`value` rows with the nested part rendered as JSON-like text.

use redis::Value;
use rustgrid_core::{CellValue, ColumnInfo, QueryResult};

/// Builds the result grid for one command's reply.
pub(crate) fn reply_result(statement: &str, value: &Value) -> QueryResult {
    let mut result = QueryResult {
        statement: statement.to_string(),
        columns: Vec::new(),
        rows: Vec::new(),
        rows_affected: 0,
        has_result_set: true,
        last_insert_id: None,
    };

    match unwrap_envelope(value) {
        Value::Array(items) | Value::Set(items) => {
            if items.is_empty() {
                result.columns = vec![column("value")];
                return result;
            }
            let nested = items
                .iter()
                .any(|item| matches!(item, Value::Array(_) | Value::Map(_) | Value::Set(_)));
            if nested {
                result.columns = vec![column("index"), column("value")];
                result.rows = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        vec![
                            CellValue::Int(index as i64),
                            CellValue::Text(nested_text(item)),
                        ]
                    })
                    .collect();
            } else {
                result.columns = vec![column("value")];
                result.rows = items.iter().map(|item| vec![cell(item)]).collect();
            }
        }
        Value::Map(pairs) => {
            result.columns = vec![column("key"), column("value")];
            result.rows = pairs
                .iter()
                .map(|(key, value)| vec![cell(key), cell(value)])
                .collect();
        }
        Value::Push { data, .. } => return reply_result(statement, &Value::Array(data.clone())),
        other => {
            result.columns = vec![column("value")];
            result.rows = vec![vec![cell(other)]];
        }
    }

    result
}

/// A reply's payload, unwrapping the RESP3 attribute envelope.
fn unwrap_envelope(value: &Value) -> &Value {
    match value {
        Value::Attribute { data, .. } => unwrap_envelope(data),
        other => other,
    }
}

fn column(name: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        data_type: "text".to_string(),
        nullable: true,
        primary_key: false,
        auto_increment: false,
        generated: false,
        comment: String::new(),
    }
}

/// Builds a two-column `key`/`value` grid for replies that are a flat list of pairs.
///
/// `HGETALL`, `CONFIG GET` and `PUBSUB NUMSUB` return `[k1, v1, k2, v2, …]`; rendering them as four
/// rows would lose the pairing. Other array replies keep the plain one-row-per-element shape.
pub(crate) fn pairs_result(statement: &str, value: &Value) -> QueryResult {
    let mut result = reply_result(statement, &Value::Array(Vec::new()));
    result.columns = vec![column("key"), column("value")];
    result.rows = match unwrap_envelope(value) {
        Value::Array(items) => items
            .chunks(2)
            .map(|pair| {
                vec![
                    cell(pair.first().unwrap_or(&Value::Nil)),
                    cell(pair.get(1).unwrap_or(&Value::Nil)),
                ]
            })
            .collect(),
        Value::Map(pairs) => pairs
            .iter()
            .map(|(key, value)| vec![cell(key), cell(value)])
            .collect(),
        _ => Vec::new(),
    };
    result
}

/// One reply value as a grid cell.
pub(crate) fn cell(value: &Value) -> CellValue {
    match unwrap_envelope(value) {
        Value::Nil => CellValue::Null,
        Value::Int(number) => CellValue::Int(*number),
        Value::Double(number) => CellValue::Float(*number),
        Value::Boolean(flag) => CellValue::Bool(*flag),
        Value::BulkString(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => CellValue::Text(text.to_string()),
            Err(_) => CellValue::Bytes(bytes.clone()),
        },
        Value::SimpleString(text) => CellValue::Text(text.clone()),
        Value::VerbatimString { text, .. } => CellValue::Text(text.clone()),
        Value::Okay => CellValue::Text("OK".to_string()),
        Value::BigNumber(number) => CellValue::Text(number.to_string()),
        Value::ServerError(error) => CellValue::Text(format!("{error:?}")),
        nested => CellValue::Text(nested_text(nested)),
    }
}

/// A nested reply rendered as compact JSON-like text for a single cell.
pub(crate) fn nested_text(value: &Value) -> String {
    match unwrap_envelope(value) {
        Value::Nil => "null".to_string(),
        Value::Int(number) => number.to_string(),
        Value::Double(number) => number.to_string(),
        Value::Boolean(flag) => flag.to_string(),
        Value::BulkString(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        Value::SimpleString(text) => text.clone(),
        Value::VerbatimString { text, .. } => text.clone(),
        Value::Okay => "OK".to_string(),
        Value::BigNumber(number) => number.to_string(),
        Value::ServerError(error) => format!("{error:?}"),
        Value::Array(items) | Value::Set(items) => {
            let rendered: Vec<String> = items.iter().map(nested_text).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Map(pairs) => {
            let rendered: Vec<String> = pairs
                .iter()
                .map(|(key, value)| format!("{}: {}", nested_text(key), nested_text(value)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        Value::Push { data, .. } => {
            let rendered: Vec<String> = data.iter().map(nested_text).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Attribute { data, .. } => nested_text(data),
    }
}

/// A reply read as an integer, accepting the bulk-string form older servers use.
pub(crate) fn reply_integer(value: &Value) -> Option<i64> {
    match unwrap_envelope(value) {
        Value::Int(number) => Some(*number),
        Value::BulkString(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse().ok(),
        Value::SimpleString(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A reply read as text (`TYPE`, `INFO`, `GET` on a text value, …).
pub(crate) fn reply_text(value: &Value) -> Option<String> {
    match unwrap_envelope(value) {
        Value::BulkString(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Value::SimpleString(text) => Some(text.clone()),
        Value::VerbatimString { text, .. } => Some(text.clone()),
        Value::Okay => Some("OK".to_string()),
        Value::Int(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_replies_become_one_row() {
        let result = reply_result("PING", &Value::SimpleString("PONG".into()));
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.rows.len(), 1);
        assert!(matches!(&result.rows[0][0], CellValue::Text(text) if text == "PONG"));
    }

    #[test]
    fn flat_arrays_are_one_column() {
        let result = reply_result(
            "KEYS *",
            &Value::Array(vec![
                Value::BulkString(b"a".to_vec()),
                Value::BulkString(b"b".to_vec()),
            ]),
        );
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.rows.len(), 2);
        assert!(matches!(&result.rows[1][0], CellValue::Text(text) if text == "b"));
    }

    #[test]
    fn nested_arrays_add_an_index_column() {
        let result = reply_result(
            "XRANGE s - +",
            &Value::Array(vec![Value::Array(vec![
                Value::BulkString(b"1-0".to_vec()),
                Value::Array(vec![
                    Value::BulkString(b"f".to_vec()),
                    Value::BulkString(b"v".to_vec()),
                ]),
            ])]),
        );
        assert_eq!(result.columns.len(), 2);
        assert_eq!(result.rows.len(), 1);
        assert!(matches!(&result.rows[0][0], CellValue::Int(0)));
        assert!(matches!(&result.rows[0][1], CellValue::Text(text) if text.contains("1-0")));
    }

    #[test]
    fn empty_arrays_still_expose_a_column() {
        let result = reply_result("KEYS x", &Value::Array(vec![]));
        assert_eq!(result.columns.len(), 1);
        assert!(result.rows.is_empty());
        assert!(result.has_result_set);
    }

    #[test]
    fn reads_numbers_from_both_encodings() {
        assert_eq!(reply_integer(&Value::Int(3)), Some(3));
        assert_eq!(reply_integer(&Value::BulkString(b"42".to_vec())), Some(42));
    }
}

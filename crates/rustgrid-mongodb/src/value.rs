//! BSON value handling for the grid.
//!
//! The app's grid only knows [`CellValue`], so every BSON shape is flattened into one of its
//! variants and re-parsed through the column's declared BSON type when the user edits it. Embedded
//! documents and arrays keep their full structure as Extended JSON text, which the row form's JSON
//! editor can pretty-print and write back.

use base64::Engine as _;
use chrono::{DateTime as ChronoDateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use mongodb::bson::{
    Binary, Bson, Decimal128, Document, Regex, Timestamp, oid::ObjectId, spec::BinarySubtype,
};
use rustgrid_core::CellValue;
use serde_json::{Map as JsonMap, Value as JsonValue, json};

/// The BSON type name shown in the grid header and used to parse an edited value back.
pub fn type_name(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Boolean(_) => "bool",
        Bson::Null => "null",
        Bson::RegularExpression(_) => "regex",
        Bson::JavaScriptCode(_) | Bson::JavaScriptCodeWithScope(_) => "javascript",
        Bson::Int32(_) => "int",
        Bson::Int64(_) => "long",
        Bson::Timestamp(_) => "timestamp",
        Bson::Binary(binary) if binary.subtype == BinarySubtype::Uuid => "uuid",
        Bson::Binary(_) => "binData",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Symbol(_) => "symbol",
        Bson::Decimal128(_) => "decimal",
        Bson::Undefined => "undefined",
        Bson::MaxKey => "maxKey",
        Bson::MinKey => "minKey",
        Bson::DbPointer(_) => "dbPointer",
    }
}

/// The grid cell for one BSON value. Scalars are shown the way a user edits them (an ObjectId as
/// hex, a date as ISO 8601, a decimal as its exact text); documents and arrays become compact
/// Extended JSON so no structure is lost.
pub fn to_cell(value: &Bson) -> CellValue {
    match value {
        Bson::Double(value) => CellValue::Float(*value),
        Bson::String(value) => CellValue::Text(value.clone()),
        Bson::Document(_) | Bson::Array(_) => CellValue::Text(compact_json(value)),
        Bson::Boolean(value) => CellValue::Bool(*value),
        Bson::Null | Bson::Undefined => CellValue::Null,
        Bson::RegularExpression(regex) => {
            CellValue::Text(format!("/{}/{}", regex.pattern, regex.options))
        }
        Bson::JavaScriptCode(code) => CellValue::Text(code.clone()),
        Bson::JavaScriptCodeWithScope(code) => CellValue::Text(code.code.clone()),
        Bson::Int32(value) => CellValue::Int(i64::from(*value)),
        Bson::Int64(value) => CellValue::Int(*value),
        Bson::Timestamp(value) => CellValue::Text(timestamp_text(*value)),
        Bson::Binary(binary) if binary.subtype == BinarySubtype::Uuid => {
            CellValue::Text(uuid_text(binary))
        }
        Bson::Binary(binary) => CellValue::Bytes(binary.bytes.clone()),
        Bson::ObjectId(value) => CellValue::Text(value.to_hex()),
        Bson::DateTime(value) => CellValue::Text(date_text(*value)),
        Bson::Symbol(value) => CellValue::Text(value.clone()),
        Bson::Decimal128(value) => CellValue::Text(value.to_string()),
        Bson::MaxKey => CellValue::Text("MaxKey".to_string()),
        Bson::MinKey => CellValue::Text("MinKey".to_string()),
        Bson::DbPointer(_) => CellValue::Text(compact_json(value)),
    }
}

/// Parse an edited cell back into BSON, using the column's declared BSON type.
pub fn from_text(type_name: &str, text: &str) -> Result<Bson, String> {
    let trimmed = text.trim();
    let kind = type_name.trim().to_ascii_lowercase();
    match kind.as_str() {
        "objectid" | "object id" | "oid" => ObjectId::parse_str(trimmed)
            .map(Bson::ObjectId)
            .map_err(|error| format!("不是一个有效的 ObjectId：{error}")),
        "string" | "text" => Ok(Bson::String(text.to_string())),
        "int" | "int32" | "integer" => {
            let value: i32 = trimmed
                .parse()
                .map_err(|error| format!("不是整数：{error}"))?;
            Ok(Bson::Int32(value))
        }
        "long" | "int64" => {
            let value: i64 = trimmed
                .parse()
                .map_err(|error| format!("不是整数：{error}"))?;
            Ok(Bson::Int64(value))
        }
        "double" | "float" | "number" => {
            let value: f64 = trimmed
                .parse()
                .map_err(|error| format!("不是数字：{error}"))?;
            Ok(Bson::Double(value))
        }
        "decimal" | "decimal128" => trimmed
            .parse::<Decimal128>()
            .map(Bson::Decimal128)
            .map_err(|error| format!("不是 Decimal128：{error}")),
        "bool" | "boolean" => trimmed
            .parse::<bool>()
            .map(Bson::Boolean)
            .map_err(|error| format!("不是布尔值（true/false）：{error}")),
        "date" | "datetime" => parse_date(trimmed).map(Bson::DateTime),
        "timestamp" => parse_timestamp(trimmed),
        "object" | "document" => match parse_json(trimmed)? {
            Bson::Document(document) => Ok(Bson::Document(document)),
            other => Ok(other),
        },
        "array" => match parse_json(trimmed)? {
            Bson::Array(values) => Ok(Bson::Array(values)),
            other => Ok(Bson::Array(vec![other])),
        },
        "json" => parse_json(trimmed),
        "binary" | "bindata" => parse_binary(trimmed),
        "uuid" => parse_uuid(trimmed),
        "regex" | "regularexpression" => parse_regex(trimmed),
        "null" => Ok(Bson::Null),
        "minkey" => Ok(Bson::MinKey),
        "maxkey" => Ok(Bson::MaxKey),
        "" | "auto" | "unknown" => infer(trimmed),
        _ => infer(trimmed),
    }
}

/// Parse free text when the column type is unknown: Extended JSON, then a JSON literal, then a
/// number, a boolean and finally the raw string.
fn infer(text: &str) -> Result<Bson, String> {
    if text.is_empty() {
        return Ok(Bson::String(String::new()));
    }
    if (text.starts_with('{') || text.starts_with('['))
        && let Ok(value) = parse_json(text)
    {
        return Ok(value);
    }
    if text == "null" {
        return Ok(Bson::Null);
    }
    if let Ok(value) = text.parse::<bool>() {
        return Ok(Bson::Boolean(value));
    }
    if let Ok(value) = text.parse::<i64>() {
        return Ok(Bson::Int64(value));
    }
    if let Ok(value) = text.parse::<f64>() {
        return Ok(Bson::Double(value));
    }
    Ok(Bson::String(text.to_string()))
}

/// Compact Extended JSON for a document or array (the grid's cell text).
pub fn compact_json(value: &Bson) -> String {
    serde_json::to_string(&to_json(value)).unwrap_or_else(|_| String::new())
}

/// Pretty Extended JSON (the row form's 格式化 switch does this itself, but it is handy for
/// diagnostics).
pub fn pretty_json(value: &Bson) -> String {
    serde_json::to_string_pretty(&to_json(value)).unwrap_or_else(|_| String::new())
}

/// Whether a BSON value is a document/array that should be shown as JSON.
pub fn is_document(value: &Bson) -> bool {
    matches!(value, Bson::Document(_) | Bson::Array(_))
}

/// Parse a text document (Extended JSON, or plain JSON with extended fields) into BSON.
pub fn parse_json(text: &str) -> Result<Bson, String> {
    let value: JsonValue =
        serde_json::from_str(text).map_err(|error| format!("JSON 语法错误：{error}"))?;
    from_json(&value)
}

/// Convert a `serde_json` value into BSON, decoding the standard Extended JSON wrappers.
pub fn from_json(value: &JsonValue) -> Result<Bson, String> {
    match value {
        JsonValue::Null => Ok(Bson::Null),
        JsonValue::Bool(value) => Ok(Bson::Boolean(*value)),
        JsonValue::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(Bson::Int64(value))
            } else if let Some(value) = number.as_u64() {
                Ok(Bson::Double(value as f64))
            } else if let Some(value) = number.as_f64() {
                Ok(Bson::Double(value))
            } else {
                Err("无法解析数字".to_string())
            }
        }
        JsonValue::String(value) => Ok(Bson::String(value.clone())),
        JsonValue::Array(values) => {
            let mut items = Vec::with_capacity(values.len());
            for value in values {
                items.push(from_json(value)?);
            }
            Ok(Bson::Array(items))
        }
        JsonValue::Object(object) => from_json_object(object),
    }
}

fn from_json_object(object: &JsonMap<String, JsonValue>) -> Result<Bson, String> {
    if object.len() == 1 {
        let (key, value) = object.iter().next().expect("one key");
        match key.as_str() {
            "$oid" => {
                return value
                    .as_str()
                    .ok_or_else(|| "$oid 必须是字符串".to_string())
                    .and_then(|text| {
                        ObjectId::parse_str(text)
                            .map(Bson::ObjectId)
                            .map_err(|error| format!("无效的 ObjectId：{error}"))
                    });
            }
            "$date" => return parse_extended_date(value).map(Bson::DateTime),
            "$numberDecimal" => {
                return value
                    .as_str()
                    .ok_or_else(|| "$numberDecimal 必须是字符串".to_string())
                    .and_then(|text| {
                        text.parse::<Decimal128>()
                            .map(Bson::Decimal128)
                            .map_err(|error| format!("无效的 Decimal128：{error}"))
                    });
            }
            "$numberLong" => {
                return value
                    .as_str()
                    .ok_or_else(|| "$numberLong 必须是字符串".to_string())
                    .and_then(|text| {
                        text.parse::<i64>()
                            .map(Bson::Int64)
                            .map_err(|error| format!("无效的 int64：{error}"))
                    });
            }
            "$numberInt" => {
                return value
                    .as_str()
                    .ok_or_else(|| "$numberInt 必须是字符串".to_string())
                    .and_then(|text| {
                        text.parse::<i32>()
                            .map(Bson::Int32)
                            .map_err(|error| format!("无效的 int32：{error}"))
                    });
            }
            "$numberDouble" => {
                let text = match value {
                    JsonValue::String(text) => text.as_str(),
                    JsonValue::Number(number) => {
                        return Ok(Bson::Double(number.as_f64().unwrap_or(0.0)));
                    }
                    _ => return Err("$numberDouble 必须是字符串".to_string()),
                };
                return text
                    .parse::<f64>()
                    .map(Bson::Double)
                    .map_err(|error| format!("无效的 double：{error}"));
            }
            "$binary" => return parse_extended_binary(value),
            "$uuid" => {
                return value
                    .as_str()
                    .ok_or_else(|| "$uuid 必须是字符串".to_string())
                    .and_then(parse_uuid);
            }
            "$timestamp" => return parse_extended_timestamp(value),
            "$regularExpression" => return parse_extended_regex(value),
            "$minKey" => return Ok(Bson::MinKey),
            "$maxKey" => return Ok(Bson::MaxKey),
            "$undefined" => return Ok(Bson::Undefined),
            "$symbol" => {
                return value
                    .as_str()
                    .map(|text| Bson::Symbol(text.to_string()))
                    .ok_or_else(|| "$symbol 必须是字符串".to_string());
            }
            "$code" => {
                return value
                    .as_str()
                    .map(|text| Bson::JavaScriptCode(text.to_string()))
                    .ok_or_else(|| "$code 必须是字符串".to_string());
            }
            _ => {}
        }
    }
    let mut document = Document::new();
    for (key, value) in object {
        document.insert(key.clone(), from_json(value)?);
    }
    Ok(Bson::Document(document))
}

/// Handle BSON field paths (`a.b.c`) when fetching a column value.
pub fn get_path<'a>(document: &'a Document, path: &str) -> Option<&'a Bson> {
    let mut parts = path.split('.');
    let first = parts.next()?;
    let mut current = document.get(first)?;
    for part in parts {
        match current {
            Bson::Document(inner) => current = inner.get(part)?,
            _ => return None,
        }
    }
    Some(current)
}

/// The JSON rendering used by `compact_json`: scalars stay native, special BSON types become
/// Extended JSON wrappers so an edit round-trips.
pub fn to_json(value: &Bson) -> JsonValue {
    match value {
        Bson::Double(value) => json!(value),
        Bson::String(value) => json!(value),
        Bson::Document(document) => {
            let mut object = JsonMap::new();
            for (key, value) in document {
                object.insert(key.clone(), to_json(value));
            }
            JsonValue::Object(object)
        }
        Bson::Array(values) => JsonValue::Array(values.iter().map(to_json).collect()),
        Bson::Boolean(value) => json!(value),
        Bson::Null => JsonValue::Null,
        Bson::Undefined => json!({ "$undefined": true }),
        Bson::RegularExpression(regex) => json!({
            "$regularExpression": {
                "pattern": regex.pattern.as_str(),
                "options": regex.options.as_str(),
            }
        }),
        Bson::JavaScriptCode(code) => json!({ "$code": code }),
        Bson::JavaScriptCodeWithScope(code) => json!({ "$code": code.code }),
        Bson::Int32(value) => json!({ "$numberInt": value.to_string() }),
        Bson::Int64(value) => json!({ "$numberLong": value.to_string() }),
        Bson::Timestamp(value) => {
            json!({ "$timestamp": { "t": value.time, "i": value.increment } })
        }
        Bson::Binary(binary) if binary.subtype == BinarySubtype::Uuid => {
            json!({ "$uuid": uuid_text(binary) })
        }
        Bson::Binary(binary) => json!({
            "$binary": {
                "base64": base64::engine::general_purpose::STANDARD.encode(&binary.bytes),
                "subType": format!("{:02x}", u8::from(binary.subtype)),
            }
        }),
        Bson::ObjectId(value) => json!({ "$oid": value.to_hex() }),
        Bson::DateTime(value) => json!({ "$date": date_text(*value) }),
        Bson::Symbol(value) => json!({ "$symbol": value }),
        Bson::Decimal128(value) => json!({ "$numberDecimal": value.to_string() }),
        Bson::MaxKey => json!({ "$maxKey": 1 }),
        Bson::MinKey => json!({ "$minKey": 1 }),
        Bson::DbPointer(pointer) => json!({ "$dbPointer": format!("{pointer:?}") }),
    }
}

fn date_text(value: mongodb::bson::DateTime) -> String {
    ChronoDateTime::<Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_else(|| value.timestamp_millis().to_string())
}

fn timestamp_text(value: Timestamp) -> String {
    format!("Timestamp({}, {})", value.time, value.increment)
}

fn uuid_text(binary: &Binary) -> String {
    uuid::Uuid::from_slice(&binary.bytes)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| base64::engine::general_purpose::STANDARD.encode(&binary.bytes))
}

fn parse_date(text: &str) -> Result<mongodb::bson::DateTime, String> {
    if let Ok(date) = ChronoDateTime::parse_from_rfc3339(text) {
        return Ok(mongodb::bson::DateTime::from_millis(
            date.with_timezone(&Utc).timestamp_millis(),
        ));
    }
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(value) = NaiveDateTime::parse_from_str(text, format) {
            return Ok(mongodb::bson::DateTime::from_millis(
                value.and_utc().timestamp_millis(),
            ));
        }
    }
    if let Ok(value) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let value = value
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| "无效的日期".to_string())?;
        return Ok(mongodb::bson::DateTime::from_millis(
            value.and_utc().timestamp_millis(),
        ));
    }
    if let Ok(millis) = text.parse::<i64>() {
        return Ok(mongodb::bson::DateTime::from_millis(millis));
    }
    Err(format!("无法解析日期：{text}"))
}

fn parse_ts_text(text: &str) -> Option<(u32, u32)> {
    let inner = text.strip_prefix("Timestamp(")?.strip_suffix(')')?;
    let (time, increment) = inner.split_once(',')?;
    Some((time.trim().parse().ok()?, increment.trim().parse().ok()?))
}

fn parse_timestamp(text: &str) -> Result<Bson, String> {
    if text.starts_with('{') {
        return parse_json(text);
    }
    if let Some((time, increment)) = parse_ts_text(text) {
        return Ok(Bson::Timestamp(Timestamp { time, increment }));
    }
    Err("时间戳格式应为 Timestamp(秒, 序号) 或 {\"$timestamp\":{\"t\":…,\"i\":…}}".to_string())
}

fn parse_binary(text: &str) -> Result<Bson, String> {
    if text.starts_with('{') {
        let value = parse_json(text)?;
        if matches!(value, Bson::Binary(_)) {
            return Ok(value);
        }
        return Err("不是二进制 JSON".to_string());
    }
    let hex: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    let bytes = decode_hex(&hex)?;
    Ok(Bson::Binary(Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }))
}

fn parse_uuid(text: &str) -> Result<Bson, String> {
    let value = uuid::Uuid::parse_str(text).map_err(|error| format!("无效的 UUID：{error}"))?;
    Ok(Bson::Binary(Binary {
        subtype: BinarySubtype::Uuid,
        bytes: value.as_bytes().to_vec(),
    }))
}

fn parse_regex(text: &str) -> Result<Bson, String> {
    if text.starts_with('{') {
        return parse_json(text);
    }
    let inner = text
        .strip_prefix('/')
        .and_then(|rest| rest.rsplit_once('/'))
        .ok_or_else(|| "正则格式应为 /pattern/flags".to_string())?;
    Ok(regex_value(inner.0, inner.1))
}

/// BSON 3 stores a regex pattern/options as C strings.
fn regex_value(pattern: &str, options: &str) -> Bson {
    use mongodb::bson::raw::CString as BsonCString;

    let empty = BsonCString::try_from("").expect("an empty C string is valid");
    Bson::RegularExpression(Regex {
        pattern: BsonCString::try_from(pattern).unwrap_or_else(|_| empty.clone()),
        options: BsonCString::try_from(options).unwrap_or(empty),
    })
}

fn parse_extended_date(value: &JsonValue) -> Result<mongodb::bson::DateTime, String> {
    match value {
        JsonValue::String(text) => parse_date(text),
        JsonValue::Object(object) => {
            if let Some(JsonValue::String(millis)) = object.get("$numberLong") {
                millis
                    .parse::<i64>()
                    .map(mongodb::bson::DateTime::from_millis)
                    .map_err(|error| format!("无效的日期毫秒：{error}"))
            } else {
                Err("$date 只支持字符串或 {\"$numberLong\": \"…\"}".to_string())
            }
        }
        _ => Err("无效的 $date".to_string()),
    }
}

fn parse_extended_binary(value: &JsonValue) -> Result<Bson, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "$binary 必须是对象".to_string())?;
    let data = object
        .get("base64")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "$binary.base64 缺失".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("无效的 base64：{error}"))?;
    let subtype = object
        .get("subType")
        .and_then(JsonValue::as_str)
        .and_then(|text| u8::from_str_radix(text, 16).ok())
        .map(BinarySubtype::from)
        .unwrap_or(BinarySubtype::Generic);
    Ok(Bson::Binary(Binary { subtype, bytes }))
}

fn parse_extended_timestamp(value: &JsonValue) -> Result<Bson, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "$timestamp 必须是对象".to_string())?;
    let time = object
        .get("t")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| "$timestamp.t 缺失".to_string())? as u32;
    let increment = object
        .get("i")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| "$timestamp.i 缺失".to_string())? as u32;
    Ok(Bson::Timestamp(Timestamp { time, increment }))
}

fn parse_extended_regex(value: &JsonValue) -> Result<Bson, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "$regularExpression 必须是对象".to_string())?;
    let pattern = object
        .get("pattern")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "$regularExpression.pattern 缺失".to_string())?;
    let options = object
        .get("options")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    Ok(regex_value(pattern, options))
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("十六进制字符串长度必须是偶数".to_string());
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let chars: Vec<char> = text.chars().collect();
    for pair in chars.chunks(2) {
        let byte = format!("{}{}", pair[0], pair[1]);
        bytes.push(
            u8::from_str_radix(&byte, 16).map_err(|error| format!("无效的十六进制：{error}"))?,
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_id_round_trips() {
        let value = Bson::ObjectId(ObjectId::parse_str("65f0c0ffee00000000000001").unwrap());
        assert_eq!(to_cell(&value).as_display(), "65f0c0ffee00000000000001");
        assert_eq!(
            from_text("objectId", "65f0c0ffee00000000000001").unwrap(),
            value
        );
    }

    #[test]
    fn embedded_document_round_trips_through_extended_json() {
        let value = Bson::Document(mongodb::bson::doc! {
            "name": "a",
            "n": 3,
            "at": Bson::DateTime(mongodb::bson::DateTime::from_millis(1_768_461_000_000)),
            "id": Bson::ObjectId(ObjectId::parse_str("65f0c0ffee00000000000001").unwrap()),
        });
        let text = compact_json(&value);
        assert_eq!(parse_json(&text).unwrap(), value);
    }

    #[test]
    fn decimal_and_bool_parse() {
        assert_eq!(
            from_text("decimal", "123456789.123456789").unwrap(),
            Bson::Decimal128("123456789.123456789".parse().unwrap())
        );
        assert_eq!(from_text("bool", "true").unwrap(), Bson::Boolean(true));
        assert_eq!(from_text("int", "42").unwrap(), Bson::Int32(42));
    }

    #[test]
    fn date_text_is_iso() {
        let date = Bson::DateTime(mongodb::bson::DateTime::from_millis(1_768_461_000_000));
        let text = match to_cell(&date) {
            CellValue::Text(text) => text,
            other => panic!("unexpected {other:?}"),
        };
        assert!(text.starts_with("2026-01-15T"));
        assert_eq!(from_text("date", &text).unwrap(), date);
    }
}

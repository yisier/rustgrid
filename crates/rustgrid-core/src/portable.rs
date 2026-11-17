//! Portable table-definition mapping between engines.
//!
//! A [`TableSchema`] is the designer's engine-agnostic model, but its column types are still the
//! source engine's native names (`varchar`, `nvarchar`, `NUMBER`, `jsonb`, ...). Data transfer
//! across engines needs a target definition whose types the target driver can render. This module
//! classifies a source column into a small logical type, then renders that logical type in the
//! target dialect. It is deliberately conservative: expressions that cannot be translated
//! (generated columns, check constraints, rules, partitioning, triggers) are dropped rather than
//! copied as invalid SQL.

use crate::dialect::DriverDialect;
use crate::model::{ColumnDef, ForeignKeyDef, IndexDef, TableOptions, TableSchema, UniqueKeyDef};

/// The logical type used between engines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalType {
    Bool,
    /// A signed integer with the given bit width (8, 16, 32 or 64).
    Int(u8),
    /// An unsigned integer with the given bit width; 64-bit unsigned does not fit a signed
    /// 64-bit target and is rendered as a wide decimal.
    UInt(u8),
    Decimal {
        precision: u32,
        scale: u32,
    },
    /// `true` for a double-precision float.
    Float(bool),
    /// `None` means an unbounded text type.
    Text(Option<u32>),
    /// `None` means an unbounded binary type.
    Binary(Option<u32>),
    Date,
    Time,
    DateTime {
        with_time_zone: bool,
    },
    Uuid,
    Json,
    Xml,
    /// A type this mapper does not understand; it falls back to the target's text type.
    Other(String),
}

/// Map a source table definition into a target dialect's definition.
///
/// Columns, primary keys, secondary indexes, unique keys and foreign keys are kept. Generated
/// expressions, checks, exclusion constraints, rules, partitioning and triggers are cleared
/// because their SQL is dialect-specific. The target driver still generates the final DDL, so a
/// target-specific default (e.g. `IDENTITY(1,1)`) comes from the mapped `ColumnDef` flags.
pub fn map_schema(
    schema: &TableSchema,
    source: DriverDialect,
    target: DriverDialect,
) -> TableSchema {
    let columns = schema
        .columns
        .iter()
        .map(|column| map_column(column, source, target))
        .collect();
    let indexes = schema
        .indexes
        .iter()
        .filter(|index| !index.primary)
        .map(|index| IndexDef {
            name: index.name.clone(),
            columns: index.columns.clone(),
            unique: index.unique,
            primary: false,
            index_type: String::new(),
            comment: String::new(),
        })
        .collect();
    let unique_keys = schema
        .unique_keys
        .iter()
        .map(|key| UniqueKeyDef {
            name: key.name.clone(),
            columns: key.columns.clone(),
        })
        .collect();
    let foreign_keys = schema
        .foreign_keys
        .iter()
        .map(|key| ForeignKeyDef {
            name: key.name.clone(),
            columns: key.columns.clone(),
            // The source schema does not exist in a different engine; let the target driver
            // qualify the reference with its own default schema.
            referenced_schema: String::new(),
            referenced_table: strip_schema(&key.referenced_table),
            referenced_columns: key.referenced_columns.clone(),
            on_delete: key.on_delete.clone(),
            on_update: key.on_update.clone(),
        })
        .collect();
    TableSchema {
        columns,
        indexes,
        unique_keys,
        checks: Vec::new(),
        excludes: Vec::new(),
        rules: Vec::new(),
        foreign_keys,
        triggers: Vec::new(),
        partitioning: String::new(),
        options: TableOptions {
            engine: if matches!(target, DriverDialect::Mysql) {
                schema.options.engine.clone()
            } else {
                String::new()
            },
            charset: if matches!(target, DriverDialect::Mysql) {
                schema.options.charset.clone()
            } else {
                String::new()
            },
            collation: if matches!(target, DriverDialect::Mysql) {
                schema.options.collation.clone()
            } else {
                String::new()
            },
            comment: schema.options.comment.clone(),
            auto_increment: String::new(),
        },
    }
}

/// The name a target engine should use for a source table.
///
/// Schema-less targets (MySQL/SQLite) receive the bare name; schema-aware targets keep a
/// schema-qualified name so the driver can resolve it.
pub fn map_table_name(name: &str, target: DriverDialect) -> String {
    if target_has_schemas(target) {
        name.to_string()
    } else {
        strip_schema(name).to_string()
    }
}

fn target_has_schemas(dialect: DriverDialect) -> bool {
    matches!(
        dialect,
        DriverDialect::Postgres | DriverDialect::SqlServer | DriverDialect::Oracle
    )
}

fn strip_schema(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, bare)| bare.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Strip ClickHouse's `Nullable(...)` / `LowCardinality(...)` wrappers so classification sees the
/// value's real type. `raw` is already lower-cased by [`classify`].
fn unwrap_clickhouse_type(raw: &str) -> String {
    let mut current = raw.trim();
    while let Some(inner) = current
        .strip_prefix("nullable(")
        .or_else(|| current.strip_prefix("lowcardinality("))
    {
        current = inner.strip_suffix(')').unwrap_or(inner);
    }
    current.to_string()
}

fn map_column(column: &ColumnDef, source: DriverDialect, target: DriverDialect) -> ColumnDef {
    let logical = classify(column, source);
    let mut mapped = column.clone();
    // Attributes that only make sense inside the source engine are cleared first; `render`
    // re-adds the target's own flags.
    mapped.charset = String::new();
    mapped.collation = String::new();
    mapped.extra = String::new();
    mapped.generated = String::new();
    mapped.stored = false;
    mapped.sparse = false;
    mapped.zerofill = false;
    mapped.unsigned = false;
    mapped.identity_start = String::new();
    mapped.identity_increment = String::new();
    mapped.identity = String::new();
    mapped.auto_increment = false;
    mapped.data_type = String::new();
    mapped.length = String::new();
    mapped.decimals = String::new();

    render(&logical, target, &mut mapped);

    // Identity/auto-increment is a target concept. Preserve the source's flag, but express it in
    // the target's own vocabulary.
    let source_identity = column.auto_increment
        || column.identity.eq_ignore_ascii_case("ALWAYS")
        || column.identity.eq_ignore_ascii_case("BY DEFAULT");
    if source_identity {
        // PostgreSQL and SQL Server identity columns must be integer types; an unsigned 64-bit
        // source became a wide decimal above, so narrow it back to bigint for the identity case.
        if matches!(target, DriverDialect::Postgres | DriverDialect::SqlServer)
            && (mapped.data_type.eq_ignore_ascii_case("numeric")
                || mapped.data_type.eq_ignore_ascii_case("decimal"))
        {
            mapped.data_type = "bigint".to_string();
            mapped.length = String::new();
            mapped.decimals = String::new();
        }
        match target {
            DriverDialect::Mysql => mapped.auto_increment = true,
            DriverDialect::Postgres => {
                mapped.auto_increment = true;
                mapped.identity = if column.identity.eq_ignore_ascii_case("ALWAYS") {
                    "ALWAYS".to_string()
                } else {
                    "BY DEFAULT".to_string()
                };
                mapped.identity_start = column.identity_start.clone();
                mapped.identity_increment = column.identity_increment.clone();
            }
            DriverDialect::SqlServer => mapped.auto_increment = true,
            DriverDialect::Oracle => {
                // Cross-engine transfers keep the source ids, so the target identity must accept
                // an explicit value; Oracle's `ALWAYS` rejects one and cannot be overridden.
                mapped.auto_increment = true;
                mapped.identity = "BY DEFAULT".to_string();
            }
            DriverDialect::Sqlite => {
                // SQLite only auto-increments an INTEGER PRIMARY KEY.
                mapped.auto_increment =
                    column.primary_key && mapped.data_type.eq_ignore_ascii_case("integer");
            }
            // ClickHouse has no identity columns (its defaults/mutations are not auto-increment).
            DriverDialect::ClickHouse => {}
            DriverDialect::Generic => {}
        }
    }

    // Defaults are copied only when they are literals or one of the portable current-time
    // spellings; arbitrary source expressions would produce invalid target SQL.
    mapped.default = map_default(column, target);
    mapped.comment = column.comment.clone();
    mapped.nullable = column.nullable;
    mapped.primary_key = column.primary_key;
    mapped
}

fn map_default(column: &ColumnDef, target: DriverDialect) -> String {
    let value = column.default.trim();
    if value.is_empty() {
        return String::new();
    }
    let lower = value.to_ascii_lowercase();
    let is_current_time = matches!(
        lower.as_str(),
        "current_timestamp"
            | "current_timestamp()"
            | "now()"
            | "getdate()"
            | "sysdatetime()"
            | "sysutcdatetime()"
            | "current_date"
            | "current_date()"
            | "curdate()"
            | "current_time"
            | "current_time()"
            | "curtime()"
            | "sysdate"
            | "systimestamp"
    );
    if is_current_time {
        return match target {
            DriverDialect::Mysql => {
                if lower.starts_with("current_date") || lower == "curdate()" {
                    "CURRENT_DATE".to_string()
                } else if lower.starts_with("current_time") || lower == "curtime()" {
                    "CURRENT_TIME".to_string()
                } else {
                    "CURRENT_TIMESTAMP".to_string()
                }
            }
            DriverDialect::Postgres => {
                if lower.starts_with("current_date") || lower == "curdate()" {
                    "CURRENT_DATE".to_string()
                } else if lower.starts_with("current_time") || lower == "curtime()" {
                    "CURRENT_TIME".to_string()
                } else {
                    "CURRENT_TIMESTAMP".to_string()
                }
            }
            DriverDialect::SqlServer => {
                if lower.starts_with("current_date") || lower == "curdate()" {
                    "CAST(GETDATE() AS date)".to_string()
                } else if lower.starts_with("current_time") || lower == "curtime()" {
                    "CAST(GETDATE() AS time)".to_string()
                } else {
                    "GETDATE()".to_string()
                }
            }
            DriverDialect::Oracle => {
                if lower.starts_with("current_date") || lower == "curdate()" {
                    "TRUNC(SYSDATE)".to_string()
                } else if lower.starts_with("current_time") || lower == "curtime()" {
                    "TO_CHAR(SYSDATE, 'HH24:MI:SS')".to_string()
                } else {
                    "SYSTIMESTAMP".to_string()
                }
            }
            DriverDialect::Sqlite => "CURRENT_TIMESTAMP".to_string(),
            DriverDialect::ClickHouse => {
                if lower.starts_with("current_date") || lower == "curdate()" {
                    "today()".to_string()
                } else if lower.starts_with("current_time") || lower == "curtime()" {
                    "formatDateTime(now(), '%H:%M:%S')".to_string()
                } else {
                    "now()".to_string()
                }
            }
            DriverDialect::Generic => value.to_string(),
        };
    }
    // Literal numbers, quoted strings, NULL and booleans are portable as written.
    if lower == "null"
        || lower == "true"
        || lower == "false"
        || value.parse::<f64>().is_ok()
        || (value.starts_with('\'') && value.ends_with('\''))
        || (value.starts_with('"') && value.ends_with('"'))
    {
        return value.to_string();
    }
    String::new()
}

fn render(logical: &LogicalType, target: DriverDialect, column: &mut ColumnDef) {
    match target {
        DriverDialect::Mysql => render_mysql(logical, column),
        DriverDialect::Postgres => render_postgres(logical, column),
        DriverDialect::SqlServer => render_sqlserver(logical, column),
        DriverDialect::Oracle => render_oracle(logical, column),
        DriverDialect::ClickHouse => render_clickhouse(logical, column),
        DriverDialect::Sqlite => render_sqlite(logical, column),
        DriverDialect::Generic => render_postgres(logical, column),
    }
}

fn render_mysql(logical: &LogicalType, column: &mut ColumnDef) {
    match logical {
        LogicalType::Bool => {
            column.data_type = "tinyint".to_string();
            column.length = "1".to_string();
        }
        LogicalType::Int(bits) => {
            column.data_type = match bits {
                0..=8 => "tinyint",
                9..=16 => "smallint",
                17..=32 => "int",
                _ => "bigint",
            }
            .to_string();
        }
        LogicalType::UInt(bits) => {
            // MySQL has a native unsigned bigint, so keep the identity-compatible integer type
            // instead of widening to decimal.
            render_mysql(
                &LogicalType::Int(if *bits >= 64 { 64 } else { *bits }),
                column,
            );
            column.unsigned = true;
        }
        LogicalType::Decimal { precision, scale } => {
            column.data_type = "decimal".to_string();
            column.length = (*precision).clamp(1, 65).to_string();
            column.decimals = (*scale).min(*precision).to_string();
        }
        LogicalType::Float(double) => {
            column.data_type = if *double { "double" } else { "float" }.to_string();
        }
        LogicalType::Text(length) => match length {
            Some(length) if *length <= 16_383 => {
                column.data_type = "varchar".to_string();
                column.length = (*length).max(1).to_string();
            }
            _ => column.data_type = "longtext".to_string(),
        },
        LogicalType::Binary(length) => match length {
            Some(length) if *length <= 65_535 => {
                column.data_type = "varbinary".to_string();
                column.length = (*length).max(1).to_string();
            }
            _ => column.data_type = "longblob".to_string(),
        },
        LogicalType::Date => column.data_type = "date".to_string(),
        LogicalType::Time => column.data_type = "time".to_string(),
        LogicalType::DateTime { with_time_zone } => {
            column.data_type = if *with_time_zone {
                "timestamp"
            } else {
                "datetime"
            }
            .to_string();
        }
        LogicalType::Uuid => {
            column.data_type = "char".to_string();
            column.length = "36".to_string();
        }
        LogicalType::Json => column.data_type = "json".to_string(),
        LogicalType::Xml => column.data_type = "longtext".to_string(),
        LogicalType::Other(_) => column.data_type = "longtext".to_string(),
    }
}

fn render_postgres(logical: &LogicalType, column: &mut ColumnDef) {
    match logical {
        LogicalType::Bool => column.data_type = "boolean".to_string(),
        LogicalType::Int(bits) => {
            column.data_type = match bits {
                0..=16 => "smallint",
                17..=32 => "integer",
                _ => "bigint",
            }
            .to_string();
        }
        LogicalType::UInt(bits) => {
            if *bits >= 64 {
                column.data_type = "numeric".to_string();
                column.length = "20".to_string();
                column.decimals = "0".to_string();
            } else {
                render_postgres(&LogicalType::Int(*bits), column);
            }
        }
        LogicalType::Decimal { precision, scale } => {
            column.data_type = "numeric".to_string();
            column.length = (*precision).clamp(1, 1000).to_string();
            column.decimals = (*scale).min(*precision).to_string();
        }
        LogicalType::Float(double) => {
            column.data_type = if *double { "double precision" } else { "real" }.to_string();
        }
        LogicalType::Text(length) => match length {
            Some(length) if *length <= 10_485_760 => {
                column.data_type = "varchar".to_string();
                column.length = (*length).max(1).to_string();
            }
            _ => column.data_type = "text".to_string(),
        },
        LogicalType::Binary(_) => column.data_type = "bytea".to_string(),
        LogicalType::Date => column.data_type = "date".to_string(),
        LogicalType::Time => column.data_type = "time".to_string(),
        LogicalType::DateTime { with_time_zone } => {
            column.data_type = if *with_time_zone {
                "timestamptz"
            } else {
                "timestamp"
            }
            .to_string();
        }
        LogicalType::Uuid => column.data_type = "uuid".to_string(),
        LogicalType::Json => column.data_type = "jsonb".to_string(),
        LogicalType::Xml => column.data_type = "xml".to_string(),
        LogicalType::Other(_) => column.data_type = "text".to_string(),
    }
}

fn render_sqlserver(logical: &LogicalType, column: &mut ColumnDef) {
    match logical {
        LogicalType::Bool => column.data_type = "bit".to_string(),
        LogicalType::Int(bits) => {
            column.data_type = match bits {
                0..=8 => "smallint",
                9..=16 => "smallint",
                17..=32 => "int",
                _ => "bigint",
            }
            .to_string();
        }
        LogicalType::UInt(bits) => {
            if *bits >= 64 {
                column.data_type = "decimal".to_string();
                column.length = "20".to_string();
                column.decimals = "0".to_string();
            } else {
                render_sqlserver(&LogicalType::Int(*bits), column);
            }
        }
        LogicalType::Decimal { precision, scale } => {
            column.data_type = "decimal".to_string();
            column.length = (*precision).clamp(1, 38).to_string();
            column.decimals = (*scale).min(*precision).to_string();
        }
        LogicalType::Float(double) => {
            column.data_type = if *double { "float" } else { "real" }.to_string();
        }
        LogicalType::Text(length) => match length {
            Some(length) if *length <= 4_000 => {
                column.data_type = "nvarchar".to_string();
                column.length = (*length).max(1).to_string();
            }
            _ => {
                column.data_type = "nvarchar".to_string();
                column.length = "max".to_string();
            }
        },
        LogicalType::Binary(length) => match length {
            Some(length) if *length <= 8_000 => {
                column.data_type = "varbinary".to_string();
                column.length = (*length).max(1).to_string();
            }
            _ => {
                column.data_type = "varbinary".to_string();
                column.length = "max".to_string();
            }
        },
        LogicalType::Date => column.data_type = "date".to_string(),
        LogicalType::Time => column.data_type = "time".to_string(),
        LogicalType::DateTime { with_time_zone } => {
            column.data_type = if *with_time_zone {
                "datetimeoffset"
            } else {
                "datetime2"
            }
            .to_string();
        }
        LogicalType::Uuid => column.data_type = "uniqueidentifier".to_string(),
        LogicalType::Json => {
            column.data_type = "nvarchar".to_string();
            column.length = "max".to_string();
        }
        LogicalType::Xml => column.data_type = "xml".to_string(),
        LogicalType::Other(_) => {
            column.data_type = "nvarchar".to_string();
            column.length = "max".to_string();
        }
    }
}

fn render_oracle(logical: &LogicalType, column: &mut ColumnDef) {
    match logical {
        LogicalType::Bool => {
            column.data_type = "NUMBER".to_string();
            column.length = "1".to_string();
        }
        LogicalType::Int(bits) => {
            column.data_type = "NUMBER".to_string();
            column.length = match bits {
                0..=8 => "3",
                9..=16 => "5",
                17..=32 => "10",
                _ => "19",
            }
            .to_string();
        }
        LogicalType::UInt(bits) => {
            column.data_type = "NUMBER".to_string();
            column.length = match bits {
                0..=8 => "3",
                9..=16 => "5",
                17..=32 => "10",
                _ => "20",
            }
            .to_string();
        }
        LogicalType::Decimal { precision, scale } => {
            column.data_type = "NUMBER".to_string();
            column.length = (*precision).clamp(1, 38).to_string();
            column.decimals = (*scale).min(*precision).to_string();
        }
        LogicalType::Float(double) => {
            column.data_type = if *double {
                "BINARY_DOUBLE"
            } else {
                "BINARY_FLOAT"
            }
            .to_string();
        }
        LogicalType::Text(length) => match length {
            Some(length) if *length <= 4_000 => {
                column.data_type = "VARCHAR2".to_string();
                column.length = (*length).max(1).to_string();
            }
            _ => column.data_type = "CLOB".to_string(),
        },
        LogicalType::Binary(length) => match length {
            Some(length) if *length <= 2_000 => {
                column.data_type = "RAW".to_string();
                column.length = (*length).max(1).to_string();
            }
            _ => column.data_type = "BLOB".to_string(),
        },
        LogicalType::Date => column.data_type = "DATE".to_string(),
        LogicalType::Time => {
            column.data_type = "VARCHAR2".to_string();
            column.length = "18".to_string();
        }
        LogicalType::DateTime { with_time_zone } => {
            column.data_type = if *with_time_zone {
                "TIMESTAMP WITH TIME ZONE"
            } else {
                "TIMESTAMP"
            }
            .to_string();
        }
        LogicalType::Uuid => {
            column.data_type = "VARCHAR2".to_string();
            column.length = "36".to_string();
        }
        LogicalType::Json => column.data_type = "CLOB".to_string(),
        LogicalType::Xml => column.data_type = "XMLTYPE".to_string(),
        LogicalType::Other(_) => column.data_type = "CLOB".to_string(),
    }
}

fn render_clickhouse(logical: &LogicalType, column: &mut ColumnDef) {
    match logical {
        LogicalType::Bool => column.data_type = "Bool".to_string(),
        LogicalType::Int(bits) => {
            column.data_type = match bits {
                0..=8 => "Int8",
                9..=16 => "Int16",
                17..=32 => "Int32",
                _ => "Int64",
            }
            .to_string();
        }
        LogicalType::UInt(bits) => {
            column.data_type = match bits {
                0..=8 => "UInt8",
                9..=16 => "UInt16",
                17..=32 => "UInt32",
                33..=64 => "UInt64",
                _ => "UInt128",
            }
            .to_string();
        }
        LogicalType::Decimal { precision, scale } => {
            // ClickHouse's Decimal(P, S) supports at most 76 digits; the 256-bit family covers 76.
            let precision = (*precision).clamp(1, 76);
            column.data_type = "Decimal".to_string();
            column.length = precision.to_string();
            column.decimals = (*scale).min(precision).to_string();
        }
        LogicalType::Float(double) => {
            column.data_type = if *double { "Float64" } else { "Float32" }.to_string();
        }
        // ClickHouse strings carry no declared length.
        LogicalType::Text(_) | LogicalType::Binary(_) => {
            column.data_type = "String".to_string();
        }
        LogicalType::Date => column.data_type = "Date".to_string(),
        // ClickHouse has no TIME type; a formatted string keeps the value readable.
        LogicalType::Time => column.data_type = "String".to_string(),
        LogicalType::DateTime { .. } => column.data_type = "DateTime64(3)".to_string(),
        LogicalType::Uuid => column.data_type = "UUID".to_string(),
        LogicalType::Json | LogicalType::Xml | LogicalType::Other(_) => {
            column.data_type = "String".to_string();
        }
    }
}

fn render_sqlite(logical: &LogicalType, column: &mut ColumnDef) {
    column.data_type = match logical {
        LogicalType::Bool | LogicalType::Int(_) | LogicalType::UInt(_) => "INTEGER",
        LogicalType::Decimal { .. } => "NUMERIC",
        LogicalType::Float(_) => "REAL",
        LogicalType::Text(_)
        | LogicalType::Date
        | LogicalType::Time
        | LogicalType::DateTime { .. } => "TEXT",
        LogicalType::Uuid | LogicalType::Json | LogicalType::Xml | LogicalType::Other(_) => "TEXT",
        LogicalType::Binary(_) => "BLOB",
    }
    .to_string();
    // SQLite's affinity does not need widths; keeping them would make the declared type look
    // like a different engine's.
    column.length = String::new();
    column.decimals = String::new();
}

fn classify(column: &ColumnDef, source: DriverDialect) -> LogicalType {
    let raw = column.data_type.trim().to_ascii_lowercase();
    let raw = if matches!(source, DriverDialect::ClickHouse) {
        unwrap_clickhouse_type(&raw)
    } else {
        raw
    };
    let data_type = raw.split('(').next().unwrap_or(&raw).trim().to_string();
    let length = column
        .length
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0);
    let precision = length.unwrap_or(0);
    let scale = column.decimals.trim().parse::<u32>().ok().unwrap_or(0);
    match data_type.as_str() {
        "bool" | "boolean" => LogicalType::Bool,
        "bit" => {
            if matches!(source, DriverDialect::Mysql) && column.length.trim() != "1" {
                LogicalType::Binary(length)
            } else {
                LogicalType::Bool
            }
        }
        "tinyint" => {
            if matches!(source, DriverDialect::Mysql) && column.length.trim() == "1" {
                LogicalType::Bool
            } else if column.unsigned || matches!(source, DriverDialect::SqlServer) {
                // SQL Server's tinyint is unsigned 0..255; a signed 8-bit target would lose half
                // its range.
                LogicalType::UInt(8)
            } else {
                LogicalType::Int(8)
            }
        }
        "smallint" | "int2" | "smallserial" | "serial2" => {
            if column.unsigned {
                LogicalType::UInt(16)
            } else {
                LogicalType::Int(16)
            }
        }
        "mediumint" => {
            if column.unsigned {
                LogicalType::UInt(24)
            } else {
                LogicalType::Int(24)
            }
        }
        "int" | "integer" | "int4" | "serial" | "serial4" => {
            if column.unsigned {
                LogicalType::UInt(32)
            } else {
                LogicalType::Int(32)
            }
        }
        // ClickHouse's sized integer types (`Int8`, `Int128`, `UInt256`, ...). They must come
        // before the SQL-standard aliases: PostgreSQL's `int8` is a 64-bit `bigint`, while
        // ClickHouse's `Int8` is one byte.
        "int8" | "int16" | "int32" | "int64" | "int128" | "int256"
            if matches!(source, DriverDialect::ClickHouse) =>
        {
            LogicalType::Int(data_type[3..].parse::<u8>().unwrap_or(64))
        }
        "uint8" | "uint16" | "uint32" | "uint64" | "uint128" | "uint256" => {
            LogicalType::UInt(data_type[4..].parse::<u8>().unwrap_or(64))
        }
        "float32" => LogicalType::Float(false),
        "float64" => LogicalType::Float(true),
        "fixedstring" => LogicalType::Text(length),
        "date32" => LogicalType::Date,
        "datetime64" => LogicalType::DateTime {
            with_time_zone: false,
        },
        "bigint" | "int8" | "bigserial" | "serial8" => {
            if column.unsigned {
                LogicalType::UInt(64)
            } else {
                LogicalType::Int(64)
            }
        }
        "number" => {
            // Oracle's bare NUMBER is a 38-digit decimal; a NUMBER with a zero scale and a
            // modest precision is an integer.
            if scale == 0 && precision <= 19 && precision > 0 {
                LogicalType::Int(if precision <= 3 {
                    8
                } else if precision <= 5 {
                    16
                } else if precision <= 10 {
                    32
                } else {
                    64
                })
            } else if scale == 0 && precision == 0 {
                LogicalType::Decimal {
                    precision: 38,
                    scale: 0,
                }
            } else {
                LogicalType::Decimal {
                    precision: precision.max(1),
                    scale,
                }
            }
        }
        "decimal" | "numeric" | "money" | "smallmoney" | "dec" => LogicalType::Decimal {
            precision: if precision == 0 { 38 } else { precision },
            scale,
        },
        "float" | "double" | "double precision" | "binary_double" | "binary_float" | "real"
        | "float4" | "float8" => LogicalType::Float(matches!(
            data_type.as_str(),
            "double" | "double precision" | "binary_double" | "float8"
        )),
        "char" | "character" | "varchar" | "character varying" | "nvarchar" | "nchar"
        | "varchar2" | "nvarchar2" | "text" | "tinytext" | "mediumtext" | "longtext" | "clob"
        | "nclob" | "ntext" | "string" | "long" => LogicalType::Text(length),
        "binary" | "varbinary" | "blob" | "tinyblob" | "mediumblob" | "longblob" | "bytea"
        | "raw" | "long raw" | "image" | "varbinary(max)" => LogicalType::Binary(length),
        "date" => {
            // Oracle's DATE always carries a time component, unlike the date-only type in the
            // other engines.
            if matches!(source, DriverDialect::Oracle) {
                LogicalType::DateTime {
                    with_time_zone: false,
                }
            } else {
                LogicalType::Date
            }
        }
        "time" | "time without time zone" | "time with time zone" => LogicalType::Time,
        "datetime"
        | "datetime2"
        | "smalldatetime"
        | "timestamp"
        | "timestamp without time zone"
        | "timestamp with time zone"
        | "timestamptz"
        | "datetimeoffset" => LogicalType::DateTime {
            with_time_zone: data_type.contains("with time zone")
                || data_type == "timestamptz"
                || data_type == "datetimeoffset",
        },
        "uuid" | "uniqueidentifier" => LogicalType::Uuid,
        "json" | "jsonb" => LogicalType::Json,
        "xml" | "xmltype" => LogicalType::Xml,
        "enum" | "set" => LogicalType::Text(length),
        _ => LogicalType::Other(data_type),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(data_type: &str, length: &str, decimals: &str) -> ColumnDef {
        ColumnDef {
            name: "value".to_string(),
            data_type: data_type.to_string(),
            length: length.to_string(),
            decimals: decimals.to_string(),
            nullable: true,
            ..Default::default()
        }
    }

    #[test]
    fn mysql_types_become_postgres_types() {
        let schema = TableSchema {
            columns: vec![
                column("bigint", "0", "0"),
                column("varchar", "255", "0"),
                column("tinyint", "1", "0"),
                column("decimal", "10", "2"),
                column("datetime", "0", "0"),
                column("longblob", "0", "0"),
            ],
            ..Default::default()
        };
        let mapped = map_schema(&schema, DriverDialect::Mysql, DriverDialect::Postgres);
        assert_eq!(mapped.columns[0].data_type, "bigint");
        assert_eq!(mapped.columns[1].data_type, "varchar");
        assert_eq!(mapped.columns[1].length, "255");
        assert_eq!(mapped.columns[2].data_type, "boolean");
        assert_eq!(mapped.columns[3].data_type, "numeric");
        assert_eq!(mapped.columns[4].data_type, "timestamp");
        assert_eq!(mapped.columns[5].data_type, "bytea");
    }

    #[test]
    fn unsigned_and_date_semantics_are_preserved() {
        let sqlserver_tinyint = ColumnDef {
            name: "small".to_string(),
            data_type: "tinyint".to_string(),
            ..Default::default()
        };
        let oracle = map_schema(
            &TableSchema {
                columns: vec![sqlserver_tinyint],
                ..Default::default()
            },
            DriverDialect::SqlServer,
            DriverDialect::Oracle,
        );
        // SQL Server's tinyint is 0..255; NUMBER(3) keeps that range.
        assert_eq!(oracle.columns[0].data_type, "NUMBER");
        assert_eq!(oracle.columns[0].length, "3");

        let oracle_date = ColumnDef {
            name: "created".to_string(),
            data_type: "DATE".to_string(),
            ..Default::default()
        };
        let postgres = map_schema(
            &TableSchema {
                columns: vec![oracle_date],
                ..Default::default()
            },
            DriverDialect::Oracle,
            DriverDialect::Postgres,
        );
        // Oracle DATE carries a time, so a date-only PostgreSQL type would lose data.
        assert_eq!(postgres.columns[0].data_type, "timestamp");
    }

    #[test]
    fn identity_is_expressed_in_the_target_vocabulary() {
        let schema = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                primary_key: true,
                auto_increment: true,
                nullable: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let pg = map_schema(&schema, DriverDialect::Mysql, DriverDialect::Postgres);
        assert!(pg.columns[0].auto_increment);
        assert_eq!(pg.columns[0].identity, "BY DEFAULT");
        let sqlserver = map_schema(&schema, DriverDialect::Mysql, DriverDialect::SqlServer);
        assert!(sqlserver.columns[0].auto_increment);
        assert_eq!(sqlserver.columns[0].data_type, "bigint");
        let sqlite = map_schema(&schema, DriverDialect::Mysql, DriverDialect::Sqlite);
        assert_eq!(sqlite.columns[0].data_type, "INTEGER");
        assert!(sqlite.columns[0].auto_increment);
    }

    #[test]
    fn mysql_types_become_clickhouse_types() {
        let schema = TableSchema {
            columns: vec![
                column("bigint", "0", "0"),
                column("varchar", "255", "0"),
                column("tinyint", "1", "0"),
                column("decimal", "20", "6"),
                column("datetime", "0", "0"),
                column("longblob", "0", "0"),
            ],
            ..Default::default()
        };
        let mapped = map_schema(&schema, DriverDialect::Mysql, DriverDialect::ClickHouse);
        assert_eq!(mapped.columns[0].data_type, "Int64");
        assert_eq!(mapped.columns[1].data_type, "String");
        assert_eq!(mapped.columns[2].data_type, "Bool");
        assert_eq!(mapped.columns[3].data_type, "Decimal");
        assert_eq!(mapped.columns[3].length, "20");
        assert_eq!(mapped.columns[3].decimals, "6");
        assert_eq!(mapped.columns[4].data_type, "DateTime64(3)");
        assert_eq!(mapped.columns[5].data_type, "String");
    }

    #[test]
    fn clickhouse_sized_types_classify_by_width() {
        let schema = TableSchema {
            columns: vec![
                column("Int8", "0", "0"),
                column("UInt64", "0", "0"),
                column("Nullable(String)", "0", "0"),
                column("LowCardinality(Nullable(Int32))", "0", "0"),
            ],
            ..Default::default()
        };
        let mapped = map_schema(&schema, DriverDialect::ClickHouse, DriverDialect::Mysql);
        assert_eq!(mapped.columns[0].data_type, "tinyint");
        assert_eq!(mapped.columns[1].data_type, "bigint");
        assert!(mapped.columns[1].unsigned);
        assert_eq!(mapped.columns[2].data_type, "longtext");
        assert_eq!(mapped.columns[3].data_type, "int");
    }

    #[test]
    fn dialect_specific_objects_are_dropped() {
        let schema = TableSchema {
            columns: vec![column("varchar", "10", "0")],
            checks: vec![crate::model::CheckDef {
                name: "ck".to_string(),
                expression: "value <> ''".to_string(),
            }],
            partitioning: "PARTITION BY HASH(id)".to_string(),
            triggers: vec![crate::model::TriggerDef {
                name: "trg".to_string(),
                timing: "BEFORE".to_string(),
                event: "INSERT".to_string(),
                statement: "SET @x = 1".to_string(),
            }],
            ..Default::default()
        };
        let mapped = map_schema(&schema, DriverDialect::Mysql, DriverDialect::Oracle);
        assert!(mapped.checks.is_empty());
        assert!(mapped.triggers.is_empty());
        assert!(mapped.partitioning.is_empty());
    }
}

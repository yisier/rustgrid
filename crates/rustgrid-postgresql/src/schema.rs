//! PostgreSQL table introspection and `CREATE`/`ALTER TABLE` generation for the table designer.

use std::collections::HashMap;

use rustgrid_core::{
    CheckDef, ColumnDef, ExcludeDef, ForeignKeyDef, IndexColumnDef, IndexDef, Result, RuleDef,
    TableOptions, TableSchema, TriggerDef, UniqueKeyDef,
};
use sqlx::{AssertSqlSafe, Row};

use crate::connection::{PostgresConnection, map_query_error};
use crate::helpers::{qualify, quote_identifier, quote_literal};

/// Split a `format_type` string (e.g. `character varying(50)`, `numeric(10,2)`,
/// `timestamp with time zone`) into a designer base type plus its length/decimals.
fn split_format_type(format_type: &str) -> (String, String, String) {
    let format_type = format_type.trim();
    if let Some(base) = format_type.strip_suffix("[]") {
        return (format!("{base}[]"), String::new(), String::new());
    }
    // Temporal types put their zone suffix *after* the precision (`timestamp(6) with time zone`),
    // unlike `character varying(50)`, so the suffix has to come off before the qualifier is read.
    let (head, zone) = if let Some(head) = format_type.strip_suffix(" with time zone") {
        (head.trim(), " with time zone")
    } else if let Some(head) = format_type.strip_suffix(" without time zone") {
        (head.trim(), " without time zone")
    } else {
        (format_type, "")
    };
    // Pull out an optional `(p[,s])` qualifier, now at the end of `head`.
    let (head, qualifier) = match head.split_once('(') {
        Some((head, rest)) => (head.trim(), rest.trim_end_matches(')').trim().to_string()),
        None => (head, String::new()),
    };
    let (length, decimals) = match qualifier.split_once(',') {
        Some((first, second)) => (first.trim().to_string(), second.trim().to_string()),
        None => (qualifier, String::new()),
    };
    let base = match (head, zone) {
        ("character varying", _) => "varchar",
        ("character", _) => "char",
        ("timestamp", " with time zone") => "timestamptz",
        ("time", " with time zone") => "timetz",
        (other, _) => other,
    };
    (base.to_string(), length, decimals)
}

/// The `GENERATED … AS IDENTITY` clause for a column, including its sequence options when the
/// designer supplied any. An invalid or empty start/increment is left to PostgreSQL's defaults.
fn identity_clause(column: &ColumnDef) -> String {
    let mut sql = format!("GENERATED {} AS IDENTITY", identity_generation(column));
    let mut options: Vec<String> = Vec::new();
    if let Some(start) = integer_option(&column.identity_start) {
        options.push(format!("START WITH {start}"));
    }
    if let Some(increment) = integer_option(&column.identity_increment) {
        options.push(format!("INCREMENT BY {increment}"));
    }
    if !options.is_empty() {
        sql.push_str(" (");
        sql.push_str(&options.join(" "));
        sql.push(')');
    }
    sql
}

/// A sequence option as a number, so only valid integers reach the DDL.
fn integer_option(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok()
}

/// The generation mode of an identity column, defaulting to PostgreSQL's `BY DEFAULT`.
fn identity_generation(column: &ColumnDef) -> &'static str {
    if column.identity.trim().eq_ignore_ascii_case("ALWAYS") {
        "ALWAYS"
    } else {
        "BY DEFAULT"
    }
}

/// The statements pre-10 `ALTER TABLE ... ALTER COLUMN ... ADD GENERATED ...` cannot express:
/// create an owned sequence and point the column's default at it, exactly what `serial` expands
/// to. The sequence name follows PostgreSQL's own `<table>_<column>_seq` convention.
fn legacy_serial_sql(schema_name: &str, table: &str, column: &ColumnDef) -> Vec<String> {
    let sequence_name = format!("{table}_{}_seq", column.name);
    let sequence = qualify(schema_name, &sequence_name);
    let qualified = qualify(schema_name, table);
    let column_name = quote_identifier(&column.name);
    let mut create = format!("CREATE SEQUENCE {sequence}");
    if let Some(start) = integer_option(&column.identity_start) {
        create.push_str(&format!(" START WITH {start}"));
    }
    if let Some(increment) = integer_option(&column.identity_increment) {
        create.push_str(&format!(" INCREMENT BY {increment}"));
    }
    vec![
        create,
        format!(
            "ALTER TABLE {qualified} ALTER COLUMN {column_name} SET DEFAULT nextval({}::regclass)",
            quote_literal(&sequence)
        ),
        format!("ALTER SEQUENCE {sequence} OWNED BY {qualified}.{column_name}"),
    ]
}

/// Append the sequence/default statements for pre-10 nullable auto-increment columns, which
/// cannot use `serial` (it implies `NOT NULL`).
fn append_legacy_serial_statements(
    statements: &mut Vec<String>,
    schema_name: &str,
    table: &str,
    columns: &[ColumnDef],
    supports_identity: bool,
) {
    if supports_identity {
        return;
    }
    for column in columns {
        if column.auto_increment
            && column.nullable
            && !is_serial_type(&column.data_type)
            && legacy_serial_type(&column.data_type).is_some()
        {
            statements.extend(legacy_serial_sql(schema_name, table, column));
        }
    }
}

/// A `serial` type for an integer column on a server without identity columns (before
/// PostgreSQL 10), so `CREATE TABLE`/`ADD COLUMN` still generates values with the engine's own
/// sequence machinery.
fn legacy_serial_type(data_type: &str) -> Option<&'static str> {
    match data_type.trim().to_ascii_lowercase().as_str() {
        "smallint" | "int2" | "smallserial" => Some("smallserial"),
        "integer" | "int" | "int4" | "serial" => Some("serial"),
        "bigint" | "int8" | "bigserial" => Some("bigserial"),
        _ => None,
    }
}

/// Whether the column type is already one of PostgreSQL's `serial` pseudo-types (which create
/// their own sequence and imply `NOT NULL`).
fn is_serial_type(data_type: &str) -> bool {
    data_type.trim().to_ascii_lowercase().ends_with("serial")
}

/// Render one column definition for `CREATE`/`ALTER TABLE`. `supports_identity` is false before
/// PostgreSQL 10, where an auto-increment column has to be declared `serial` instead.
fn column_sql(column: &ColumnDef, supports_identity: bool) -> String {
    let mut sql = quote_identifier(&column.name);
    let mut data_type = if column.data_type.trim().is_empty() {
        "text"
    } else {
        column.data_type.trim()
    };
    if !supports_identity
        && column.auto_increment
        && !column.nullable
        && let Some(serial) = legacy_serial_type(data_type)
    {
        data_type = serial;
    }
    sql.push(' ');
    sql.push_str(data_type);

    let serial = is_serial_type(data_type);
    let length = column.length.trim();
    let decimals = column.decimals.trim();
    if !length.is_empty() && length != "0" {
        sql.push('(');
        sql.push_str(length);
        if !decimals.is_empty() && decimals != "0" {
            sql.push(',');
            sql.push_str(decimals);
        }
        sql.push(')');
    }

    if !column.nullable {
        sql.push_str(" NOT NULL");
    }
    // An identity cannot carry a `DEFAULT`, and `serial` already implies its sequence, so an
    // auto-increment column never writes one. Before PostgreSQL 10 the caller appends a sequence
    // and default for a *nullable* auto-increment column, which `serial` would force NOT NULL.
    if column.auto_increment {
        if supports_identity && !serial {
            sql.push(' ');
            sql.push_str(&identity_clause(column));
        }
    } else if !column.default.trim().is_empty() {
        sql.push_str(" DEFAULT ");
        sql.push_str(column.default.trim());
    }
    if !column.comment.trim().is_empty() {
        // Comments are emitted as separate `COMMENT ON COLUMN` statements by the caller.
    }
    sql
}

fn index_sql(schema: &str, table: &str, index: &IndexDef) -> String {
    let columns = index
        .columns
        .iter()
        .map(|column| {
            let mut sql = quote_identifier(&column.name);
            if column.descending {
                sql.push_str(" DESC");
            }
            sql
        })
        .collect::<Vec<_>>()
        .join(", ");
    // PostgreSQL's index method is its `USING` clause; an empty type means the server's default
    // (`btree`), which the driver omits rather than spelling out.
    let method = index.index_type.trim();
    let using = if method.is_empty() {
        String::new()
    } else {
        format!(" USING {}", quote_identifier(&method.to_ascii_lowercase()))
    };
    format!(
        "CREATE {}INDEX {} ON {}{using} ({columns})",
        if index.unique { "UNIQUE " } else { "" },
        quote_identifier(&index.name),
        qualify(schema, table)
    )
}

/// `COMMENT ON INDEX` for an index whose designer 注释 is non-empty.
fn index_comment_sql(schema: &str, index: &IndexDef) -> Option<String> {
    (!index.comment.trim().is_empty()).then(|| {
        format!(
            "COMMENT ON INDEX {} IS {}",
            qualify(schema, &index.name),
            quote_literal(index.comment.trim())
        )
    })
}

fn foreign_key_sql(foreign_key: &ForeignKeyDef) -> String {
    let columns = foreign_key
        .columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let referenced = foreign_key
        .referenced_columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let referenced_table = if !foreign_key.referenced_schema.trim().is_empty() {
        qualify(
            foreign_key.referenced_schema.trim(),
            &foreign_key.referenced_table,
        )
    } else if foreign_key.referenced_table.contains('.') {
        // Compatibility with a schema loaded before `referenced_schema` existed: the qualified
        // name used to be stored in `referenced_table` alone.
        let (schema, name) = foreign_key
            .referenced_table
            .split_once('.')
            .expect("checked contains dot");
        qualify(schema, name)
    } else {
        quote_identifier(&foreign_key.referenced_table)
    };
    let constraint = if foreign_key.name.trim().is_empty() {
        String::new()
    } else {
        format!("CONSTRAINT {} ", quote_identifier(&foreign_key.name))
    };
    let mut sql =
        format!("{constraint}FOREIGN KEY ({columns}) REFERENCES {referenced_table} ({referenced})");
    if !foreign_key.on_delete.trim().is_empty() {
        sql.push_str(" ON DELETE ");
        sql.push_str(foreign_key.on_delete.trim());
    }
    if !foreign_key.on_update.trim().is_empty() {
        sql.push_str(" ON UPDATE ");
        sql.push_str(foreign_key.on_update.trim());
    }
    sql
}

fn unique_key_sql(key: &UniqueKeyDef) -> String {
    let body = format!(
        "UNIQUE ({})",
        key.columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if key.name.trim().is_empty() {
        body
    } else {
        format!("CONSTRAINT {} {body}", quote_identifier(&key.name))
    }
}

fn check_sql(check: &CheckDef) -> String {
    let body = format!("CHECK ({})", check.expression.trim());
    if check.name.trim().is_empty() {
        body
    } else {
        format!("CONSTRAINT {} {body}", quote_identifier(&check.name))
    }
}

fn exclude_sql(exclude: &ExcludeDef) -> String {
    let body = exclude.definition.trim();
    if exclude.name.trim().is_empty() {
        body.to_string()
    } else {
        format!("CONSTRAINT {} {body}", quote_identifier(&exclude.name))
    }
}

fn exclude_comment_sql(schema_name: &str, table: &str, exclude: &ExcludeDef) -> Option<String> {
    (!exclude.comment.trim().is_empty()).then(|| {
        format!(
            "COMMENT ON CONSTRAINT {} ON {} IS {}",
            quote_identifier(&exclude.name),
            qualify(schema_name, table),
            quote_literal(exclude.comment.trim())
        )
    })
}

fn rule_statement(rule: &RuleDef) -> Option<String> {
    let statement = rule.definition.trim().trim_end_matches(';').trim_end();
    (!statement.is_empty()).then(|| statement.to_string())
}

pub(crate) async fn table_schema(
    connection: &PostgresConnection,
    database: &str,
    table: &str,
) -> Result<TableSchema> {
    let (schema_name, bare) = connection.resolve_object(database, table).await?;
    let pool = connection.pool_for(database).await?;
    let features = connection.catalog_features();

    // `attidentity` and `pg_sequence` arrived in PostgreSQL 10. Older servers expose serial
    // columns only through their `nextval(...)` default, so the identity columns read as empty
    // and the sequence start/increment stay unset (the server's own defaults apply).
    let identity = if features.identity {
        "a.attidentity::text"
    } else {
        "''::text"
    };
    let sequence_columns = if features.sequences {
        "seq.seqstart, seq.seqincrement"
    } else {
        "NULL::bigint AS seqstart, NULL::bigint AS seqincrement"
    };
    let sequence_join = if features.sequences {
        "LEFT JOIN LATERAL ( \
             SELECT s.seqstart, s.seqincrement \
             FROM pg_sequence s \
             WHERE s.seqrelid = to_regclass( \
                 pg_get_serial_sequence(format('%I.%I', n.nspname, c.relname), a.attname)) \
         ) seq ON true"
    } else {
        ""
    };

    let column_rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT a.attname, \
                format_type(a.atttypid, a.atttypmod) AS data_type, \
                NOT a.attnotnull AS nullable, \
                pg_get_expr(d.adbin, d.adrelid) AS default_expr, \
                col_description(a.attrelid, a.attnum) AS comment, \
                {identity}, \
                EXISTS (SELECT 1 FROM pg_index i \
                        WHERE i.indrelid = a.attrelid AND i.indisprimary \
                          AND a.attnum = ANY(i.indkey)) AS primary_key, \
                {sequence_columns} \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         {sequence_join} \
         WHERE n.nspname = $1 AND c.relname = $2 \
           AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum"
    )))
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;

    let mut columns = Vec::with_capacity(column_rows.len());
    for row in &column_rows {
        let name: String = row.try_get(0).unwrap_or_default();
        let format_type: String = row.try_get(1).unwrap_or_default();
        let nullable: bool = row.try_get(2).unwrap_or(true);
        let default_expr: Option<String> = row.try_get(3).unwrap_or(None);
        let comment: String = row.try_get(4).unwrap_or(None).unwrap_or_default();
        let identity_code: String = row.try_get(5).unwrap_or_default();
        let primary_key: bool = row.try_get(6).unwrap_or(false);
        let identity_start: Option<i64> = row.try_get(7).unwrap_or(None);
        let identity_increment: Option<i64> = row.try_get(8).unwrap_or(None);

        let (data_type, length, decimals) = split_format_type(&format_type);
        let default = default_expr.unwrap_or_default();
        let is_serial_default = default.to_ascii_lowercase().starts_with("nextval(");
        let identity = match identity_code.as_str() {
            "a" => "ALWAYS".to_string(),
            "d" => "BY DEFAULT".to_string(),
            _ => String::new(),
        };
        let auto_increment = !identity.is_empty() || is_serial_default;
        columns.push(ColumnDef {
            name,
            data_type,
            length,
            decimals,
            nullable,
            default: if is_serial_default {
                String::new()
            } else {
                default
            },
            primary_key,
            auto_increment,
            identity,
            identity_start: if auto_increment {
                identity_start
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            identity_increment: if auto_increment {
                identity_increment
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            unsigned: false,
            zerofill: false,
            sparse: false,
            comment,
            charset: String::new(),
            collation: String::new(),
            extra: String::new(),
            generated: String::new(),
            stored: false,
        });
    }

    let index_rows = sqlx::query(
        "SELECT i.relname, ix.indisunique, ix.indisprimary, a.attname, \
                (ix.indoption[k.ord - 1] & 1) = 1 AS descending, am.amname, \
                obj_description(i.oid, 'pg_class') AS comment \
         FROM pg_index ix \
         JOIN pg_class i ON i.oid = ix.indexrelid \
         JOIN pg_class t ON t.oid = ix.indrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         LEFT JOIN pg_am am ON am.oid = i.relam \
         CROSS JOIN generate_series(1, ix.indnatts) AS k(ord) \
         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ix.indkey[k.ord - 1] \
         WHERE n.nspname = $1 AND t.relname = $2 \
           AND NOT EXISTS (SELECT 1 FROM pg_constraint u \
                           WHERE u.conindid = ix.indexrelid AND u.contype = 'u') \
         ORDER BY i.relname, k.ord",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    let mut indexes: Vec<IndexDef> = Vec::new();
    for row in &index_rows {
        let name: String = row.try_get(0).unwrap_or_default();
        let unique: bool = row.try_get(1).unwrap_or(false);
        let primary: bool = row.try_get(2).unwrap_or(false);
        let column: String = row.try_get(3).unwrap_or_default();
        // PostgreSQL indexes have no prefix length (子部分 is MySQL's).
        let descending: bool = row.try_get(4).unwrap_or(false);
        let method: String = row.try_get(5).unwrap_or_default();
        let comment: String = row.try_get(6).unwrap_or(None).unwrap_or_default();
        let index_column = IndexColumnDef {
            name: column,
            descending,
            prefix_length: String::new(),
        };
        match indexes.iter_mut().find(|index| index.name == name) {
            Some(existing) => existing.columns.push(index_column),
            None => indexes.push(IndexDef {
                name,
                columns: vec![index_column],
                unique,
                primary,
                index_type: method,
                comment,
            }),
        }
    }

    let fk_rows = sqlx::query(
        "SELECT con.conname, \
                (SELECT array_agg(a.attname ORDER BY k.ord) \
                 FROM generate_series(1, array_length(con.conkey, 1)) AS k(ord) \
                 JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = con.conkey[k.ord]) AS columns, \
                rn.nspname, rc.relname, \
                (SELECT array_agg(a.attname ORDER BY k.ord) \
                 FROM generate_series(1, array_length(con.confkey, 1)) AS k(ord) \
                 JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = con.confkey[k.ord]) AS ref_columns, \
                con.confupdtype::text, con.confdeltype::text \
         FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_class rc ON rc.oid = con.confrelid \
         JOIN pg_namespace rn ON rn.oid = rc.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND con.contype = 'f' \
         ORDER BY con.conname",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    let mut foreign_keys = Vec::with_capacity(fk_rows.len());
    for row in &fk_rows {
        let name: String = row.try_get(0).unwrap_or_default();
        let columns: Vec<String> = row.try_get(1).unwrap_or_default();
        let ref_schema: String = row.try_get(2).unwrap_or_default();
        let ref_table: String = row.try_get(3).unwrap_or_default();
        let ref_columns: Vec<String> = row.try_get(4).unwrap_or_default();
        let update_type: String = row.try_get(5).unwrap_or_default();
        let delete_type: String = row.try_get(6).unwrap_or_default();
        foreign_keys.push(ForeignKeyDef {
            name,
            columns,
            referenced_schema: ref_schema,
            referenced_table: ref_table,
            referenced_columns: ref_columns,
            on_delete: normalize_action(&delete_type),
            on_update: normalize_action(&update_type),
        });
    }

    let unique_rows = sqlx::query(
        "SELECT con.conname, \
                (SELECT array_agg(a.attname ORDER BY k.ord) \
                 FROM generate_series(1, array_length(con.conkey, 1)) AS k(ord) \
                 JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = con.conkey[k.ord]) AS columns \
         FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND con.contype = 'u' \
         ORDER BY con.conname",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    let unique_keys = unique_rows
        .iter()
        .map(|row| UniqueKeyDef {
            name: row.try_get(0).unwrap_or_default(),
            columns: row.try_get(1).unwrap_or_default(),
        })
        .collect();

    let check_rows = sqlx::query(
        "SELECT con.conname, pg_get_constraintdef(con.oid, true) \
         FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND con.contype = 'c' \
         ORDER BY con.conname",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    let checks = check_rows
        .iter()
        .map(|row| CheckDef {
            name: row.try_get(0).unwrap_or_default(),
            expression: check_expression(&row.try_get::<String, _>(1).unwrap_or_default()),
        })
        .collect();

    let exclude_rows = sqlx::query(
        "SELECT con.conname, pg_get_constraintdef(con.oid, true), \
                obj_description(con.oid, 'pg_constraint') \
         FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND con.contype = 'x' \
         ORDER BY con.conname",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    let excludes = exclude_rows
        .iter()
        .map(|row| ExcludeDef {
            name: row.try_get(0).unwrap_or_default(),
            definition: row.try_get(1).unwrap_or_default(),
            comment: row
                .try_get::<Option<String>, _>(2)
                .ok()
                .flatten()
                .unwrap_or_default(),
        })
        .collect();

    let rule_rows = sqlx::query(
        "SELECT r.rulename, pg_get_ruledef(r.oid, true) \
         FROM pg_rewrite r \
         JOIN pg_class c ON c.oid = r.ev_class \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND r.rulename <> '_RETURN' \
         ORDER BY r.rulename",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    let rules = rule_rows
        .iter()
        .map(|row| RuleDef {
            name: row.try_get(0).unwrap_or_default(),
            definition: row.try_get(1).unwrap_or_default(),
        })
        .collect();

    let trigger_rows = sqlx::query(
        "SELECT t.tgname, pg_get_triggerdef(t.oid) \
         FROM pg_trigger t \
         JOIN pg_class c ON c.oid = t.tgrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND NOT t.tgisinternal \
         ORDER BY t.tgname",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    let triggers = trigger_rows
        .iter()
        .map(|row| TriggerDef {
            name: row.try_get(0).unwrap_or_default(),
            timing: String::new(),
            event: String::new(),
            statement: row.try_get(1).unwrap_or_default(),
        })
        .collect();

    let comment = sqlx::query(
        "SELECT obj_description(c.oid, 'pg_class') \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2",
    )
    .bind(&schema_name)
    .bind(&bare)
    .fetch_optional(&pool)
    .await
    .map_err(map_query_error)?
    .and_then(|row| row.try_get::<String, _>(0).ok())
    .unwrap_or_default();

    Ok(TableSchema {
        columns,
        indexes,
        unique_keys,
        checks,
        excludes,
        rules,
        foreign_keys,
        triggers,
        partitioning: String::new(),
        options: TableOptions {
            comment,
            ..Default::default()
        },
    })
}

/// Strip the `CHECK (...)` wrapper and one *matching* outer parenthesis pair. PostgreSQL returns
/// `CHECK (((a > 0) AND (b > 0)))` for a compound check; a blind `[1..len-1]` would corrupt the
/// inner `(a > 0)` and `(b > 0)`, so the pair is removed only when its opening paren closes at the
/// very end.
fn check_expression(definition: &str) -> String {
    let definition = definition.trim();
    let body = definition
        .strip_prefix("CHECK (")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(definition)
        .trim();
    strip_matching_outer_parens(body).to_string()
}

fn strip_matching_outer_parens(text: &str) -> &str {
    let text = text.trim();
    if !text.starts_with('(') || !text.ends_with(')') {
        return text;
    }
    let mut depth = 0usize;
    let mut quoted = false;
    for (position, character) in text.char_indices() {
        match character {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return if position + character.len_utf8() == text.len() {
                        text[1..text.len() - 1].trim()
                    } else {
                        text
                    };
                }
            }
            _ => {}
        }
    }
    text
}

fn normalize_action(code: &str) -> String {
    match code {
        "c" => "CASCADE".to_string(),
        "n" => "SET NULL".to_string(),
        "d" => "SET DEFAULT".to_string(),
        "r" => "RESTRICT".to_string(),
        _ => String::new(),
    }
}

fn primary_columns(schema: &TableSchema) -> Vec<String> {
    schema
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect()
}

fn primary_index_name(schema: &TableSchema) -> Option<String> {
    schema
        .indexes
        .iter()
        .find(|index| index.primary)
        .map(|index| index.name.clone())
}

fn comment_statements(schema_name: &str, table: &str, schema: &TableSchema) -> Vec<String> {
    let mut statements = Vec::new();
    if !schema.options.comment.trim().is_empty() {
        statements.push(format!(
            "COMMENT ON TABLE {} IS {}",
            qualify(schema_name, table),
            quote_literal(schema.options.comment.trim())
        ));
    }
    for column in &schema.columns {
        if !column.comment.trim().is_empty() {
            statements.push(format!(
                "COMMENT ON COLUMN {}.{} IS {}",
                qualify(schema_name, table),
                quote_identifier(&column.name),
                quote_literal(column.comment.trim())
            ));
        }
    }
    for exclude in &schema.excludes {
        if let Some(statement) = exclude_comment_sql(schema_name, table, exclude) {
            statements.push(statement);
        }
    }
    statements
}

/// `DROP`/`CREATE TRIGGER` statements for PostgreSQL's complete trigger definitions.
pub(crate) fn trigger_sql(
    schema_name: &str,
    table: &str,
    original: Option<&TableSchema>,
    modified: &TableSchema,
) -> Vec<String> {
    let mut statements = Vec::new();
    let original_triggers = original
        .map(|schema| schema.triggers.as_slice())
        .unwrap_or(&[]);
    for trigger in original_triggers {
        if trigger.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified
            .triggers
            .iter()
            .find(|candidate| candidate.name == trigger.name)
            .is_some_and(|candidate| candidate == trigger);
        if !unchanged {
            statements.push(format!(
                "DROP TRIGGER IF EXISTS {} ON {}",
                quote_identifier(&trigger.name),
                qualify(schema_name, table)
            ));
        }
    }
    for trigger in &modified.triggers {
        // A new full definition has no name until the designer reloads; the statement carries it.
        if trigger.statement.trim().is_empty() {
            continue;
        }
        let unchanged = original
            .map(|schema| schema.triggers.as_slice())
            .is_some_and(|triggers| {
                triggers
                    .iter()
                    .find(|candidate| candidate.name == trigger.name)
                    .is_some_and(|candidate| candidate == trigger)
            });
        if !unchanged {
            statements.push(
                trigger
                    .statement
                    .trim()
                    .trim_end_matches(';')
                    .trim_end()
                    .to_string(),
            );
        }
    }
    statements
}

pub(crate) fn table_schema_sql(
    schema_name: &str,
    table: &str,
    original: Option<&TableSchema>,
    modified: &TableSchema,
    supports_identity: bool,
) -> String {
    let qualified = qualify(schema_name, table);

    let Some(original) = original else {
        let mut definitions: Vec<String> = modified
            .columns
            .iter()
            .map(|column| column_sql(column, supports_identity))
            .collect();
        let primary = primary_columns(modified);
        if !primary.is_empty() {
            definitions.push(format!(
                "PRIMARY KEY ({})",
                primary
                    .iter()
                    .map(|column| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for key in &modified.unique_keys {
            if !key.columns.is_empty() {
                definitions.push(unique_key_sql(key));
            }
        }
        for check in &modified.checks {
            if !check.expression.trim().is_empty() {
                definitions.push(check_sql(check));
            }
        }
        for exclude in &modified.excludes {
            if !exclude.definition.trim().is_empty() {
                definitions.push(exclude_sql(exclude));
            }
        }
        for foreign_key in &modified.foreign_keys {
            definitions.push(foreign_key_sql(foreign_key));
        }
        let mut statements = vec![format!(
            "CREATE TABLE {qualified} (\n    {}\n)",
            definitions.join(",\n    ")
        )];
        append_legacy_serial_statements(
            &mut statements,
            schema_name,
            table,
            &modified.columns,
            supports_identity,
        );
        for index in modified.indexes.iter().filter(|index| !index.primary) {
            statements.push(index_sql(schema_name, table, index));
            if let Some(comment) = index_comment_sql(schema_name, index) {
                statements.push(comment);
            }
        }
        statements.extend(comment_statements(schema_name, table, modified));
        statements.extend(modified.rules.iter().filter_map(rule_statement));
        return statements.join(";\n");
    };

    let mut statements: Vec<String> = Vec::new();
    let original_columns: HashMap<&str, &ColumnDef> = original
        .columns
        .iter()
        .map(|column| (column.name.as_str(), column))
        .collect();
    let modified_columns: HashMap<&str, &ColumnDef> = modified
        .columns
        .iter()
        .map(|column| (column.name.as_str(), column))
        .collect();

    let original_indexes: HashMap<&str, &IndexDef> = original
        .indexes
        .iter()
        .map(|index| (index.name.as_str(), index))
        .collect();
    let modified_indexes: HashMap<&str, &IndexDef> = modified
        .indexes
        .iter()
        .map(|index| (index.name.as_str(), index))
        .collect();
    let original_fks: HashMap<&str, &ForeignKeyDef> = original
        .foreign_keys
        .iter()
        .filter(|foreign_key| !foreign_key.name.trim().is_empty())
        .map(|foreign_key| (foreign_key.name.as_str(), foreign_key))
        .collect();
    let modified_fks: HashMap<&str, &ForeignKeyDef> = modified
        .foreign_keys
        .iter()
        .filter(|foreign_key| !foreign_key.name.trim().is_empty())
        .map(|foreign_key| (foreign_key.name.as_str(), foreign_key))
        .collect();
    let original_uniques: HashMap<&str, &UniqueKeyDef> = original
        .unique_keys
        .iter()
        .filter(|key| !key.name.trim().is_empty())
        .map(|key| (key.name.as_str(), key))
        .collect();
    let modified_uniques: HashMap<&str, &UniqueKeyDef> = modified
        .unique_keys
        .iter()
        .filter(|key| !key.name.trim().is_empty())
        .map(|key| (key.name.as_str(), key))
        .collect();
    let original_checks: HashMap<&str, &CheckDef> = original
        .checks
        .iter()
        .filter(|check| !check.name.trim().is_empty())
        .map(|check| (check.name.as_str(), check))
        .collect();
    let modified_checks: HashMap<&str, &CheckDef> = modified
        .checks
        .iter()
        .filter(|check| !check.name.trim().is_empty())
        .map(|check| (check.name.as_str(), check))
        .collect();
    let original_excludes: HashMap<&str, &ExcludeDef> = original
        .excludes
        .iter()
        .filter(|exclude| !exclude.name.trim().is_empty())
        .map(|exclude| (exclude.name.as_str(), exclude))
        .collect();
    let modified_excludes: HashMap<&str, &ExcludeDef> = modified
        .excludes
        .iter()
        .filter(|exclude| !exclude.name.trim().is_empty())
        .map(|exclude| (exclude.name.as_str(), exclude))
        .collect();
    let original_rules: HashMap<&str, &RuleDef> = original
        .rules
        .iter()
        .filter(|rule| !rule.name.trim().is_empty())
        .map(|rule| (rule.name.as_str(), rule))
        .collect();
    let modified_rules: HashMap<&str, &RuleDef> = modified
        .rules
        .iter()
        .filter(|rule| !rule.name.trim().is_empty())
        .map(|rule| (rule.name.as_str(), rule))
        .collect();

    // Constraints and indexes are dropped before columns and re-added at the end: PostgreSQL
    // refuses to drop a column a constraint still uses, and `DROP COLUMN` would already remove an
    // index, making the later explicit `DROP INDEX` fail.
    for foreign_key in &original.foreign_keys {
        if foreign_key.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified_fks
            .get(foreign_key.name.as_str())
            .is_some_and(|candidate| *candidate == foreign_key);
        if !unchanged {
            statements.push(format!(
                "ALTER TABLE {qualified} DROP CONSTRAINT {}",
                quote_identifier(&foreign_key.name)
            ));
        }
    }
    for key in &original.unique_keys {
        if key.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified_uniques
            .get(key.name.as_str())
            .is_some_and(|candidate| *candidate == key);
        if !unchanged {
            statements.push(format!(
                "ALTER TABLE {qualified} DROP CONSTRAINT {}",
                quote_identifier(&key.name)
            ));
        }
    }
    for check in &original.checks {
        if check.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified_checks
            .get(check.name.as_str())
            .is_some_and(|candidate| *candidate == check);
        if !unchanged {
            statements.push(format!(
                "ALTER TABLE {qualified} DROP CONSTRAINT {}",
                quote_identifier(&check.name)
            ));
        }
    }
    for exclude in &original.excludes {
        if exclude.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified_excludes
            .get(exclude.name.as_str())
            .is_some_and(|candidate| *candidate == exclude);
        if !unchanged {
            statements.push(format!(
                "ALTER TABLE {qualified} DROP CONSTRAINT {}",
                quote_identifier(&exclude.name)
            ));
        }
    }
    for rule in &original.rules {
        if rule.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified_rules
            .get(rule.name.as_str())
            .is_some_and(|candidate| *candidate == rule);
        if !unchanged {
            statements.push(format!(
                "DROP RULE IF EXISTS {} ON {qualified}",
                quote_identifier(&rule.name)
            ));
        }
    }

    let original_primary = primary_columns(original);
    let modified_primary = primary_columns(modified);
    if original_primary != modified_primary
        && let Some(name) = primary_index_name(original)
    {
        statements.push(format!(
            "ALTER TABLE {qualified} DROP CONSTRAINT {}",
            quote_identifier(&name)
        ));
    }
    for index in original.indexes.iter().filter(|index| !index.primary) {
        let unchanged = modified_indexes
            .get(index.name.as_str())
            .is_some_and(|candidate| *candidate == index);
        if !unchanged {
            statements.push(format!("DROP INDEX {}", qualify(schema_name, &index.name)));
        }
    }

    for column in &modified.columns {
        match original_columns.get(column.name.as_str()) {
            None => {
                statements.push(format!(
                    "ALTER TABLE {qualified} ADD COLUMN {}",
                    column_sql(column, supports_identity)
                ));
                append_legacy_serial_statements(
                    &mut statements,
                    schema_name,
                    table,
                    std::slice::from_ref(column),
                    supports_identity,
                );
            }
            Some(previous) => {
                let type_changed = previous.data_type != column.data_type
                    || previous.length != column.length
                    || previous.decimals != column.decimals;
                if type_changed {
                    statements.push(format!(
                        "ALTER TABLE {qualified} ALTER COLUMN {} TYPE {} USING {}::{}",
                        quote_identifier(&column.name),
                        column_type_sql(column),
                        quote_identifier(&column.name),
                        column_type_sql(column)
                    ));
                }
                if previous.nullable != column.nullable {
                    statements.push(format!(
                        "ALTER TABLE {qualified} ALTER COLUMN {} {} NOT NULL",
                        quote_identifier(&column.name),
                        if column.nullable { "DROP" } else { "SET" }
                    ));
                }
                // Identity changes come before the default comparison: a column cannot carry both
                // an identity and a `DEFAULT`, so a default has to be dropped before an identity is
                // added and an identity has to be dropped before a default is set.
                let was_identity = previous.auto_increment && !previous.identity.trim().is_empty();
                let is_identity = column.auto_increment && !column.identity.trim().is_empty();
                if !previous.auto_increment && column.auto_increment {
                    if !previous.default.trim().is_empty() {
                        statements.push(format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {} DROP DEFAULT",
                            quote_identifier(&column.name)
                        ));
                    }
                    if supports_identity {
                        statements.push(format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {} ADD {}",
                            quote_identifier(&column.name),
                            identity_clause(column)
                        ));
                    } else if !is_serial_type(&column.data_type)
                        && legacy_serial_type(&column.data_type).is_some()
                    {
                        statements.extend(legacy_serial_sql(schema_name, table, column));
                    } else {
                        statements.push(format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {} ADD {}",
                            quote_identifier(&column.name),
                            identity_clause(column)
                        ));
                    }
                } else if previous.auto_increment && !column.auto_increment {
                    // A serial column keeps its `nextval(...)` default outside the model, so
                    // dropping the identity is not enough to stop it generating values.
                    let clause = if was_identity {
                        "DROP IDENTITY IF EXISTS"
                    } else {
                        "DROP DEFAULT"
                    };
                    statements.push(format!(
                        "ALTER TABLE {qualified} ALTER COLUMN {} {clause}",
                        quote_identifier(&column.name)
                    ));
                } else if previous.auto_increment
                    && column.auto_increment
                    && !was_identity
                    && is_identity
                {
                    // A serial column converted to a real identity: its `nextval(...)` default
                    // must go before the identity can be added.
                    statements.push(format!(
                        "ALTER TABLE {qualified} ALTER COLUMN {} DROP DEFAULT",
                        quote_identifier(&column.name)
                    ));
                    statements.push(format!(
                        "ALTER TABLE {qualified} ALTER COLUMN {} ADD {}",
                        quote_identifier(&column.name),
                        identity_clause(column)
                    ));
                } else if previous.auto_increment && column.auto_increment && was_identity {
                    if identity_generation(previous) != identity_generation(column) {
                        statements.push(format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {} SET GENERATED {}",
                            quote_identifier(&column.name),
                            identity_generation(column)
                        ));
                    }
                    if previous.identity_increment.trim() != column.identity_increment.trim()
                        && let Some(increment) = integer_option(&column.identity_increment)
                    {
                        statements.push(format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {} SET INCREMENT BY {increment}",
                            quote_identifier(&column.name)
                        ));
                    }
                    if previous.identity_start.trim() != column.identity_start.trim()
                        && let Some(start) = integer_option(&column.identity_start)
                    {
                        // `SET START WITH` records the value a later restart returns to, while
                        // `RESTART WITH` makes the very next generated value honour it.
                        statements.push(format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {} SET START WITH {start}",
                            quote_identifier(&column.name)
                        ));
                        statements.push(format!(
                            "ALTER TABLE {qualified} ALTER COLUMN {} RESTART WITH {start}",
                            quote_identifier(&column.name)
                        ));
                    }
                }
                if !column.auto_increment && previous.default != column.default {
                    let clause = if column.default.trim().is_empty() {
                        "DROP DEFAULT".to_string()
                    } else {
                        format!("SET DEFAULT {}", column.default.trim())
                    };
                    statements.push(format!(
                        "ALTER TABLE {qualified} ALTER COLUMN {} {clause}",
                        quote_identifier(&column.name)
                    ));
                }
                if previous.comment != column.comment {
                    statements.push(format!(
                        "COMMENT ON COLUMN {}.{} IS {}",
                        qualified,
                        quote_identifier(&column.name),
                        if column.comment.trim().is_empty() {
                            "NULL".to_string()
                        } else {
                            quote_literal(column.comment.trim())
                        }
                    ));
                }
            }
        }
    }
    for column in &original.columns {
        if !modified_columns.contains_key(column.name.as_str()) {
            statements.push(format!(
                "ALTER TABLE {qualified} DROP COLUMN {}",
                quote_identifier(&column.name)
            ));
        }
    }

    if original_primary != modified_primary && !modified_primary.is_empty() {
        statements.push(format!(
            "ALTER TABLE {qualified} ADD PRIMARY KEY ({})",
            modified_primary
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    for index in modified.indexes.iter().filter(|index| !index.primary) {
        let unchanged = original_indexes
            .get(index.name.as_str())
            .is_some_and(|candidate| *candidate == index);
        if !unchanged {
            statements.push(index_sql(schema_name, table, index));
            if let Some(comment) = index_comment_sql(schema_name, index) {
                statements.push(comment);
            }
        }
    }

    for key in &modified.unique_keys {
        if key.name.trim().is_empty() || key.columns.is_empty() {
            continue;
        }
        let unchanged = original_uniques
            .get(key.name.as_str())
            .is_some_and(|candidate| *candidate == key);
        if !unchanged {
            statements.push(format!(
                "ALTER TABLE {qualified} ADD {}",
                unique_key_sql(key)
            ));
        }
    }

    for check in &modified.checks {
        if check.name.trim().is_empty() || check.expression.trim().is_empty() {
            continue;
        }
        let unchanged = original_checks
            .get(check.name.as_str())
            .is_some_and(|candidate| *candidate == check);
        if !unchanged {
            statements.push(format!("ALTER TABLE {qualified} ADD {}", check_sql(check)));
        }
    }

    for exclude in &modified.excludes {
        if exclude.name.trim().is_empty() || exclude.definition.trim().is_empty() {
            continue;
        }
        let unchanged = original_excludes
            .get(exclude.name.as_str())
            .is_some_and(|candidate| *candidate == exclude);
        if !unchanged {
            statements.push(format!(
                "ALTER TABLE {qualified} ADD {}",
                exclude_sql(exclude)
            ));
        }
        let comment_changed = !unchanged
            || original_excludes
                .get(exclude.name.as_str())
                .is_none_or(|previous| previous.comment != exclude.comment);
        if comment_changed {
            statements.push(if exclude.comment.trim().is_empty() {
                format!(
                    "COMMENT ON CONSTRAINT {} ON {qualified} IS NULL",
                    quote_identifier(&exclude.name)
                )
            } else {
                exclude_comment_sql(schema_name, table, exclude).unwrap_or_default()
            });
        }
    }

    for foreign_key in &modified.foreign_keys {
        if foreign_key.name.trim().is_empty() {
            continue;
        }
        let unchanged = original_fks
            .get(foreign_key.name.as_str())
            .is_some_and(|candidate| *candidate == foreign_key);
        if !unchanged {
            statements.push(format!(
                "ALTER TABLE {qualified} ADD {}",
                foreign_key_sql(foreign_key)
            ));
        }
    }

    for rule in &modified.rules {
        if rule.definition.trim().is_empty() {
            continue;
        }
        let unchanged = original_rules
            .get(rule.name.as_str())
            .is_some_and(|candidate| *candidate == rule);
        if !unchanged && let Some(statement) = rule_statement(rule) {
            statements.push(statement);
        }
    }

    if original.options.comment != modified.options.comment {
        statements.push(format!(
            "COMMENT ON TABLE {qualified} IS {}",
            if modified.options.comment.trim().is_empty() {
                "NULL".to_string()
            } else {
                quote_literal(modified.options.comment.trim())
            }
        ));
    }

    statements.join(";\n")
}

/// The SQL type of a column (base type plus its qualifier), for `ALTER COLUMN ... TYPE`.
fn column_type_sql(column: &ColumnDef) -> String {
    let mut sql = column.data_type.trim().to_string();
    let length = column.length.trim();
    let decimals = column.decimals.trim();
    if !length.is_empty() && length != "0" {
        sql.push('(');
        sql.push_str(length);
        if !decimals.is_empty() && decimals != "0" {
            sql.push(',');
            sql.push_str(decimals);
        }
        sql.push(')');
    }
    sql
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_format_types() {
        assert_eq!(
            split_format_type("character varying(50)"),
            ("varchar".to_string(), "50".to_string(), String::new())
        );
        assert_eq!(
            split_format_type("numeric(10,2)"),
            ("numeric".to_string(), "10".to_string(), "2".to_string())
        );
        assert_eq!(
            split_format_type("timestamp with time zone"),
            ("timestamptz".to_string(), String::new(), String::new())
        );
        assert_eq!(
            split_format_type("integer[]"),
            ("integer[]".to_string(), String::new(), String::new())
        );
        // The precision sits before the zone suffix, so it belongs in `length`, not in the type.
        assert_eq!(
            split_format_type("timestamp(6) with time zone"),
            ("timestamptz".to_string(), "6".to_string(), String::new())
        );
        assert_eq!(
            split_format_type("time(3) without time zone"),
            ("time".to_string(), "3".to_string(), String::new())
        );
    }

    #[test]
    fn creates_a_table_mapped_from_mysql() {
        use rustgrid_core::{ColumnDef, DriverDialect, TableSchema, map_schema};

        let source = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    data_type: "bigint".to_string(),
                    nullable: false,
                    primary_key: true,
                    auto_increment: true,
                    ..Default::default()
                },
                ColumnDef {
                    name: "email".to_string(),
                    data_type: "varchar".to_string(),
                    length: "255".to_string(),
                    ..Default::default()
                },
                ColumnDef {
                    name: "payload".to_string(),
                    data_type: "longblob".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mapped = map_schema(&source, DriverDialect::Mysql, DriverDialect::Postgres);
        let sql = table_schema_sql("public", "users", None, &mapped, true);
        assert!(sql.contains("\"id\" bigint"), "{sql}");
        assert!(sql.contains("\"email\" varchar(255)"), "{sql}");
        assert!(sql.contains("\"payload\" bytea"), "{sql}");
        assert!(sql.contains("GENERATED BY DEFAULT AS IDENTITY"), "{sql}");
    }

    #[test]
    fn builds_index_method_and_comment() {
        let schema = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
                ..Default::default()
            }],
            indexes: vec![IndexDef {
                name: "idx_id".to_string(),
                columns: vec![IndexColumnDef {
                    name: "id".to_string(),
                    descending: true,
                    prefix_length: String::new(),
                }],
                unique: true,
                index_type: "hash".to_string(),
                comment: "lookup".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let sql = table_schema_sql("public", "t", None, &schema, true);
        assert!(
            sql.contains(
                "CREATE UNIQUE INDEX \"idx_id\" ON \"public\".\"t\" USING \"hash\" (\"id\" DESC)"
            ),
            "{sql}"
        );
        assert!(
            sql.contains("COMMENT ON INDEX \"public\".\"idx_id\" IS 'lookup'"),
            "{sql}"
        );
    }

    #[test]
    fn builds_full_trigger_diff() {
        let original = TableSchema {
            triggers: vec![TriggerDef {
                name: "trg".to_string(),
                statement:
                    "CREATE TRIGGER trg AFTER INSERT ON public.t FOR EACH ROW EXECUTE FUNCTION f()"
                        .to_string(),
                ..TriggerDef::default()
            }],
            ..TableSchema::default()
        };
        let mut modified = original.clone();
        modified.triggers[0].statement =
            "CREATE TRIGGER trg AFTER UPDATE ON public.t FOR EACH ROW EXECUTE FUNCTION f()"
                .to_string();

        let sql = trigger_sql("public", "t", Some(&original), &modified);
        assert_eq!(sql[0], "DROP TRIGGER IF EXISTS \"trg\" ON \"public\".\"t\"");
        assert!(sql[1].contains("AFTER UPDATE"), "{sql:?}");
    }

    #[test]
    fn builds_exclusion_constraints_and_rules() {
        let mut schema = TableSchema {
            columns: vec![ColumnDef {
                name: "room".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        schema.excludes.push(ExcludeDef {
            name: "ex_room".to_string(),
            definition: "EXCLUDE USING gist (room WITH =)".to_string(),
            comment: "one booking per room".to_string(),
        });
        schema.rules.push(RuleDef {
            name: "r_ignore".to_string(),
            definition: "CREATE RULE r_ignore AS ON INSERT TO public.t DO INSTEAD NOTHING;"
                .to_string(),
        });

        let sql = table_schema_sql("public", "t", None, &schema, true);
        assert!(
            sql.contains("CONSTRAINT \"ex_room\" EXCLUDE USING gist (room WITH =)"),
            "{sql}"
        );
        assert!(
            sql.contains(
                "COMMENT ON CONSTRAINT \"ex_room\" ON \"public\".\"t\" IS 'one booking per room'"
            ),
            "{sql}"
        );
        assert!(
            sql.contains("CREATE RULE r_ignore AS ON INSERT TO public.t DO INSTEAD NOTHING"),
            "{sql}"
        );

        let original = schema.clone();
        let mut modified = original.clone();
        modified.excludes[0].definition =
            "EXCLUDE USING gist (room WITH =, during WITH &&)".to_string();
        modified.rules.clear();
        let sql = table_schema_sql("public", "t", Some(&original), &modified, true);
        assert!(
            sql.contains("ALTER TABLE \"public\".\"t\" DROP CONSTRAINT \"ex_room\""),
            "{sql}"
        );
        assert!(
            sql.contains(
                "ADD CONSTRAINT \"ex_room\" EXCLUDE USING gist (room WITH =, during WITH &&)"
            ),
            "{sql}"
        );
        // Dropping and recreating the constraint loses its comment, so the comment is re-applied.
        assert!(
            sql.contains(
                "COMMENT ON CONSTRAINT \"ex_room\" ON \"public\".\"t\" IS 'one booking per room'"
            ),
            "{sql}"
        );
        assert!(
            sql.contains("DROP RULE IF EXISTS \"r_ignore\" ON \"public\".\"t\""),
            "{sql}"
        );
    }

    #[test]
    fn builds_unique_and_check_constraints() {
        let mut schema = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    data_type: "integer".to_string(),
                    nullable: false,
                    ..Default::default()
                },
                ColumnDef {
                    name: "code".to_string(),
                    data_type: "text".to_string(),
                    nullable: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        schema.unique_keys.push(UniqueKeyDef {
            name: "uq_code".to_string(),
            columns: vec!["code".to_string()],
        });
        schema.checks.push(CheckDef {
            name: "ck_id".to_string(),
            expression: "\"id\" > 0".to_string(),
        });

        let sql = table_schema_sql("public", "t", None, &schema, true);
        assert!(
            sql.contains("CONSTRAINT \"uq_code\" UNIQUE (\"code\")"),
            "{sql}"
        );
        assert!(
            sql.contains("CONSTRAINT \"ck_id\" CHECK (\"id\" > 0)"),
            "{sql}"
        );

        let mut modified = schema.clone();
        modified.unique_keys[0].columns.push("id".to_string());
        modified.checks[0].expression = "\"id\" >= 0".to_string();
        let sql = table_schema_sql("public", "t", Some(&schema), &modified, true);
        assert!(
            sql.contains("ALTER TABLE \"public\".\"t\" DROP CONSTRAINT \"uq_code\""),
            "{sql}"
        );
        assert!(
            sql.contains(
                "ALTER TABLE \"public\".\"t\" ADD CONSTRAINT \"uq_code\" UNIQUE (\"code\", \"id\")"
            ),
            "{sql}"
        );
        assert!(
            sql.contains(
                "ALTER TABLE \"public\".\"t\" ADD CONSTRAINT \"ck_id\" CHECK (\"id\" >= 0)"
            ),
            "{sql}"
        );
    }

    #[test]
    fn strips_the_check_wrapper_without_corrupting_inner_groups() {
        assert_eq!(check_expression("CHECK ((age > 0))"), "age > 0".to_string());
        assert_eq!(
            check_expression("CHECK (((age > 0) AND (age < 100)))"),
            "(age > 0) AND (age < 100)".to_string()
        );
        assert_eq!(
            check_expression("CHECK ((a > 0) AND (b > 0))"),
            "(a > 0) AND (b > 0)".to_string()
        );
    }

    #[test]
    fn builds_create_table() {
        let schema = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
                primary_key: true,
                auto_increment: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let sql = table_schema_sql("public", "t", None, &schema, true);
        assert!(sql.starts_with("CREATE TABLE \"public\".\"t\""));
        assert!(sql.contains("\"id\" integer NOT NULL GENERATED BY DEFAULT AS IDENTITY"));
        assert!(sql.contains("PRIMARY KEY (\"id\")"));
    }

    #[test]
    fn builds_identity_sequence_options() {
        let schema = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                nullable: false,
                auto_increment: true,
                identity: "ALWAYS".to_string(),
                identity_start: "1000".to_string(),
                identity_increment: "2".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let sql = table_schema_sql("public", "t", None, &schema, true);
        assert!(sql.contains(
            "\"id\" bigint NOT NULL GENERATED ALWAYS AS IDENTITY (START WITH 1000 INCREMENT BY 2)"
        ));

        // Altering an existing identity changes the generation mode and the sequence itself.
        let original = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                nullable: false,
                auto_increment: true,
                identity: "BY DEFAULT".to_string(),
                identity_start: "1".to_string(),
                identity_increment: "1".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let modified = TableSchema {
            columns: vec![ColumnDef {
                identity: "ALWAYS".to_string(),
                identity_start: "1000".to_string(),
                identity_increment: "2".to_string(),
                ..original.columns[0].clone()
            }],
            ..Default::default()
        };
        let alter = table_schema_sql("public", "t", Some(&original), &modified, true);
        assert!(alter.contains("ALTER COLUMN \"id\" SET GENERATED ALWAYS"));
        assert!(alter.contains("ALTER COLUMN \"id\" SET INCREMENT BY 2"));
        assert!(alter.contains("ALTER COLUMN \"id\" SET START WITH 1000"));
        assert!(alter.contains("ALTER COLUMN \"id\" RESTART WITH 1000"));
    }

    #[test]
    fn builds_serial_columns_before_postgresql_10() {
        let schema = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    data_type: "bigint".to_string(),
                    nullable: false,
                    primary_key: true,
                    auto_increment: true,
                    ..Default::default()
                },
                ColumnDef {
                    name: "seq".to_string(),
                    data_type: "integer".to_string(),
                    nullable: true,
                    auto_increment: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let sql = table_schema_sql("public", "t", None, &schema, false);
        assert!(sql.contains("\"id\" bigserial NOT NULL"), "{sql}");
        // `serial` implies NOT NULL, so a nullable auto-increment column gets an explicit
        // sequence/default instead.
        assert!(sql.contains("\"seq\" integer,"), "{sql}");
        assert!(
            sql.contains("CREATE SEQUENCE \"public\".\"t_seq_seq\""),
            "{sql}"
        );
        assert!(
            sql.contains(
                "ALTER TABLE \"public\".\"t\" ALTER COLUMN \"seq\" SET DEFAULT nextval('\"public\".\"t_seq_seq\"'::regclass)"
            ),
            "{sql}"
        );
        assert!(
            sql.contains(
                "ALTER SEQUENCE \"public\".\"t_seq_seq\" OWNED BY \"public\".\"t\".\"seq\""
            ),
            "{sql}"
        );
        assert!(!sql.contains("GENERATED"), "{sql}");

        // A modern server keeps identity columns.
        let modern = table_schema_sql("public", "t", None, &schema, true);
        assert!(modern.contains("\"id\" bigint NOT NULL GENERATED BY DEFAULT AS IDENTITY"));
        assert!(!modern.contains("CREATE SEQUENCE"), "{modern}");
    }

    #[test]
    fn alters_auto_increment_before_postgresql_10() {
        let original = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    data_type: "bigint".to_string(),
                    nullable: false,
                    primary_key: true,
                    auto_increment: true,
                    ..Default::default()
                },
                ColumnDef {
                    name: "amount".to_string(),
                    data_type: "bigint".to_string(),
                    nullable: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut modified = original.clone();
        modified.columns.push(ColumnDef {
            name: "seq".to_string(),
            data_type: "integer".to_string(),
            nullable: true,
            auto_increment: true,
            ..Default::default()
        });
        modified.columns[1].auto_increment = true;

        let sql = table_schema_sql("public", "t", Some(&original), &modified, false);
        assert!(
            sql.contains("ALTER TABLE \"public\".\"t\" ADD COLUMN \"seq\" integer"),
            "{sql}"
        );
        assert!(
            sql.contains("CREATE SEQUENCE \"public\".\"t_seq_seq\""),
            "{sql}"
        );
        assert!(
            sql.contains("CREATE SEQUENCE \"public\".\"t_amount_seq\""),
            "{sql}"
        );
        assert!(
            sql.contains("ALTER COLUMN \"amount\" SET DEFAULT nextval"),
            "{sql}"
        );
        assert!(!sql.contains("GENERATED"), "{sql}");
    }
}

//! The ClickHouse [`Connection`] implementation.
//!
//! ClickHouse is reached through its **HTTP interface** (port 8123, or 8443 with TLS). Each request
//! is independent, so the connection holds no pool and no mutable session state: `clickhouse::Client`
//! is `Clone + Send + Sync` and safe to share. Result sets are read in
//! `JSONCompactEachRowWithNamesAndTypes`, which reports the column names and ClickHouse types along
//! with the rows.
//!
//! The data grid is a read-only projection (see [`DriverCapability::ReadOnlyGrid`]): ClickHouse has
//! no unique constraint, nothing identifies a single row, and `ALTER TABLE … UPDATE/DELETE` is an
//! asynchronous mutation. Inserts are still supported for the import wizard; the query editor is
//! the place for every other write.
//!
//! [`DriverCapability::ReadOnlyGrid`]: rustgrid_core::DriverCapability::ReadOnlyGrid

use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clickhouse::{Client, error::Error as ClickHouseError};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rustgrid_core::{
    BackupObjectKind, ColumnInfo, Connection, ConnectionOptions, DatabaseInfo, DatabaseOptions,
    DriverId, Error, FilterNode, ObjectDump, ObjectKind, PageRequest, QueryResult, Result,
    RowInsert, RowUpdate, ScriptSession, TableInfo, TablePage, TableSchema, TableStatus, TlsMode,
};
use rustgrid_tunnel::Tunnel;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::helpers;

/// How many rows one `INSERT` statement carries.
const INSERT_BATCH_ROWS: usize = helpers::INSERT_BATCH_ROWS;

/// The ClickHouse driver's live connection.
pub(crate) struct ClickHouseConnection {
    client: Client,
    default_database: String,
    read_only: bool,
    /// Keeps the SSH/HTTP/SOCKS forwarder alive for the connection's lifetime.
    _tunnel: Option<Tunnel>,
}

impl ClickHouseConnection {
    /// Open a connection from a profile. The default database is part of the HTTP endpoint, which
    /// also validates it: a missing database fails the probe instead of the first page load.
    pub(crate) async fn open(
        host: &str,
        port: u16,
        username: &str,
        password: Option<&str>,
        database: &str,
        options: &ConnectionOptions,
    ) -> Result<Self> {
        let mut target_host = if host.trim().is_empty() {
            "127.0.0.1".to_string()
        } else {
            host.trim().to_string()
        };
        let mut target_port = if port == 0 { 8123 } else { port };
        let tunnel = if options.tunnel.is_empty() {
            None
        } else {
            let tunnel = Tunnel::start(options.tunnel.clone(), target_host.clone(), target_port)
                .await
                .map_err(Error::Connection)?;
            target_host = "127.0.0.1".to_string();
            target_port = tunnel.local_port();
            Some(tunnel)
        };

        let default_database = if database.trim().is_empty() {
            "default".to_string()
        } else {
            database.trim().to_string()
        };

        let build = |scheme: &str| -> Result<Client> {
            build_client(
                scheme,
                &target_host,
                target_port,
                username,
                password,
                &default_database,
                options,
            )
        };

        // With TLS preferred, try HTTPS first and fall back to plain HTTP: ClickHouse serves one or
        // the other on a given port, and the profile does not know which.
        let preferred = options.tls.mode == TlsMode::Preferred;
        let client = if options.tls.mode == TlsMode::Disabled {
            build("http")?
        } else {
            let secure = build("https")?;
            if preferred {
                match probe(&secure).await {
                    Ok(_) => secure,
                    // A rejected password is not a reason to try plain HTTP: report it so the app
                    // opens its password prompt.
                    Err(error) if is_auth_error(&error.to_string()) => {
                        return Err(connect_error(error, &target_host, target_port));
                    }
                    Err(_) => build("http")?,
                }
            } else {
                secure
            }
        };

        probe(&client)
            .await
            .map_err(|error| connect_error(error, &target_host, target_port))?;

        Ok(Self {
            client,
            default_database,
            read_only: options.read_only,
            _tunnel: tunnel,
        })
    }

    fn client_for(&self, database: Option<&str>) -> Client {
        match database.filter(|name| !name.trim().is_empty()) {
            Some(database) => self.client.clone().with_database(database.trim()),
            None => self.client.clone(),
        }
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.read_only {
            return Err(Error::Query(
                "This connection is read-only; enable writes in the connection settings.".into(),
            ));
        }
        Ok(())
    }

    async fn fetch_rows(&self, sql: &str) -> Result<helpers::DecodedResult> {
        let client = self.client.clone();
        let query = helpers::ensure_row_format(sql);
        let mut cursor = client
            .query_raw(&query)
            .fetch_bytes(helpers::ROW_FORMAT)
            .map_err(map_query_error)?;
        let body = cursor.collect().await.map_err(map_query_error)?;
        helpers::decode_body(&body)
    }

    async fn execute_sql(&self, sql: &str) -> Result<()> {
        self.client
            .query_raw(sql)
            .execute()
            .await
            .map_err(map_query_error)
    }

    /// Every table of a database with its `system.tables` overview, keyed by name.
    async fn read_table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        let sql = format!(
            "SELECT name, engine, total_rows, total_bytes, metadata_modification_time, comment, \
             sorting_key, partition_key FROM system.tables WHERE database = {} AND is_temporary = \
             0",
            helpers::quote_literal(database)
        );
        let decoded = self.fetch_rows(&sql).await?;
        let mut statuses = Vec::with_capacity(decoded.rows.len());
        for row in &decoded.rows {
            let name = text_at(&decoded.columns, row, "name").unwrap_or_default();
            let mut create_options = Vec::new();
            if let Some(partition) = text_at(&decoded.columns, row, "partition_key")
                && !partition.is_empty()
            {
                create_options.push(format!("PARTITION BY {partition}"));
            }
            if let Some(sorting) = text_at(&decoded.columns, row, "sorting_key")
                && !sorting.is_empty()
            {
                create_options.push(format!("ORDER BY {sorting}"));
            }
            statuses.push((
                name,
                TableStatus {
                    engine: text_at(&decoded.columns, row, "engine"),
                    rows: u64_at(&decoded.columns, row, "total_rows"),
                    data_length: u64_at(&decoded.columns, row, "total_bytes"),
                    updated: text_at(&decoded.columns, row, "metadata_modification_time"),
                    comment: text_at(&decoded.columns, row, "comment"),
                    create_options: (!create_options.is_empty()).then(|| create_options.join(" ")),
                    ..TableStatus::default()
                },
            ));
        }
        statuses.sort_by_key(|(name, _)| name.to_ascii_lowercase());
        Ok(statuses)
    }
}

#[async_trait]
impl Connection for ClickHouseConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("clickhouse")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        let decoded = self
            .fetch_rows("SELECT name FROM system.databases ORDER BY name")
            .await?;
        let mut databases: Vec<String> = decoded
            .rows
            .iter()
            .filter_map(|row| text_at(&decoded.columns, row, "name"))
            .filter(|name| {
                // The tree shows the user's databases; the system catalogue is reachable from the
                // query editor instead.
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "system" | "information_schema" | "_temporary_and_external_tables"
                )
            })
            .collect();
        databases.sort_by_key(|name| name.to_ascii_lowercase());
        Ok(databases
            .into_iter()
            .map(|name| DatabaseInfo { name })
            .collect())
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        let sql = format!(
            "SELECT name, engine FROM system.tables WHERE database = {} AND is_temporary = 0 AND \
             name NOT LIKE '.inner%' ORDER BY name",
            helpers::quote_literal(database)
        );
        let decoded = self.fetch_rows(&sql).await?;
        let mut tables: Vec<TableInfo> = decoded
            .rows
            .iter()
            .filter_map(|row| {
                let name = text_at(&decoded.columns, row, "name")?;
                let engine = text_at(&decoded.columns, row, "engine").unwrap_or_default();
                let kind = if matches!(
                    engine.as_str(),
                    "View" | "MaterializedView" | "LiveView" | "WindowView"
                ) {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                };
                Some(TableInfo {
                    name,
                    kind,
                    updatable: false,
                })
            })
            .collect();
        tables.sort_by_key(|table| table.name.to_ascii_lowercase());
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let sql = format!(
            "SELECT name, type, default_kind, comment FROM system.columns WHERE database = {} AND \
             table = {} ORDER BY position",
            helpers::quote_literal(database),
            helpers::quote_literal(table)
        );
        let decoded = self.fetch_rows(&sql).await?;
        let mut columns = Vec::with_capacity(decoded.rows.len());
        for row in &decoded.rows {
            let Some(name) = text_at(&decoded.columns, row, "name") else {
                continue;
            };
            let data_type = text_at(&decoded.columns, row, "type").unwrap_or_default();
            let default_kind = text_at(&decoded.columns, row, "default_kind").unwrap_or_default();
            columns.push(ColumnInfo {
                name,
                nullable: helpers::is_nullable(&data_type),
                data_type,
                primary_key: false,
                // ClickHouse has no identity/auto-increment columns.
                auto_increment: false,
                // MATERIALIZED / ALIAS / EPHEMERAL values are computed by the engine.
                generated: matches!(
                    default_kind.to_ascii_uppercase().as_str(),
                    "MATERIALIZED" | "ALIAS" | "EPHEMERAL"
                ),
                comment: text_at(&decoded.columns, row, "comment").unwrap_or_default(),
            });
        }
        Ok(columns)
    }

    async fn fetch_page(
        &self,
        database: &str,
        table: &str,
        page: PageRequest,
    ) -> Result<TablePage> {
        let columns = self.columns(database, table).await?;
        let page_size = page.page_size.max(1);
        let sql = helpers::page_sql(database, table, &columns, &page);
        let decoded = self.fetch_rows(&sql).await?;
        let total_rows = if page.skip_total {
            None
        } else {
            self.count_rows(database, table, &page.filter)
                .await
                .ok()
                .flatten()
        };
        Ok(TablePage {
            columns,
            rows: decoded.rows,
            page: page.page,
            page_size,
            total_rows,
        })
    }

    async fn count_rows(
        &self,
        database: &str,
        table: &str,
        filter: &[FilterNode],
    ) -> Result<Option<u64>> {
        let columns = self.columns(database, table).await?;
        let sql = helpers::count_sql(database, table, filter, &columns);
        let decoded = self.fetch_rows(&sql).await?;
        let total = decoded
            .rows
            .first()
            .and_then(|row| u64_at(&decoded.columns, row, "total"));
        Ok(total)
    }

    /// The grid never stages edits for a ClickHouse page (the driver advertises
    /// `DriverCapability::ReadOnlyGrid`); a write that still arrives is refused rather than applied
    /// as a mutation.
    async fn update_rows(
        &self,
        _database: &str,
        _table: &str,
        _updates: &[RowUpdate],
    ) -> Result<()> {
        Err(Error::Query(
            "ClickHouse tables are read-only in the grid; use the SQL editor to change rows."
                .into(),
        ))
    }

    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        self.ensure_writable()?;
        if rows.is_empty() {
            return Ok(());
        }
        let columns = self.columns(database, table).await?;
        for chunk in rows.chunks(INSERT_BATCH_ROWS) {
            let sql = helpers::insert_sql(database, table, &columns, chunk);
            if !sql.is_empty() {
                self.execute_sql(&sql).await?;
            }
        }
        Ok(())
    }

    async fn import_rows(
        &self,
        database: &str,
        table: &str,
        rows: &[RowInsert],
    ) -> Result<(usize, usize)> {
        self.insert_rows(database, table, rows).await?;
        Ok((rows.len(), 0))
    }

    /// See [`Connection::update_rows`]: rows have no identity to delete by.
    async fn delete_rows(
        &self,
        _database: &str,
        _table: &str,
        _keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()> {
        Err(Error::Query(
            "ClickHouse tables are read-only in the grid; use the SQL editor to delete rows."
                .into(),
        ))
    }

    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        run_statement(&self.client_for(database), sql).await
    }

    async fn execute_query_many(
        &self,
        database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let client = self.client_for(database);
        let mut results = Vec::new();
        for statement in rustgrid_core::split::split_statements(sql) {
            if statement.trim().is_empty() {
                continue;
            }
            let result = run_statement(&client, &statement).await?;
            if result.has_result_set {
                results.push(result);
            }
        }
        Ok(results)
    }

    async fn script_session(
        &self,
        database: Option<&str>,
    ) -> Result<Option<Box<dyn ScriptSession>>> {
        // A stable session id makes `SET` survive between the statements of one run; the current
        // database is tracked on the client side so `USE` works across statements too.
        let session_id = session_id();
        let client = self
            .client
            .clone()
            .with_setting("session_id", session_id)
            .with_setting("session_timeout", "3600");
        let database = database
            .filter(|name| !name.trim().is_empty())
            .map(|name| name.trim().to_string())
            .or_else(|| Some(self.default_database.clone()));
        Ok(Some(Box::new(ClickHouseSession { client, database })))
    }

    async fn create_database(&self, name: &str, options: &DatabaseOptions) -> Result<()> {
        self.ensure_writable()?;
        self.execute_sql(&self.create_database_sql(name, options))
            .await
    }

    fn create_database_sql(&self, name: &str, _options: &DatabaseOptions) -> String {
        format!("CREATE DATABASE {}", helpers::quote_identifier(name))
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        self.ensure_writable()?;
        self.execute_sql(&format!(
            "DROP DATABASE {}",
            helpers::quote_identifier(name)
        ))
        .await
    }

    async fn drop_table(&self, database: &str, table: &str) -> Result<()> {
        self.ensure_writable()?;
        self.execute_sql(&format!(
            "DROP TABLE {}",
            helpers::table_ref(database, table)
        ))
        .await
    }

    /// ClickHouse has no `DELETE FROM`: `TRUNCATE` is its equivalent, and the two app actions
    /// (清空表 / 截断表) map to the same statement.
    async fn empty_table(&self, database: &str, table: &str) -> Result<()> {
        self.truncate_table(database, table).await
    }

    async fn truncate_table(&self, database: &str, table: &str) -> Result<()> {
        self.ensure_writable()?;
        self.execute_sql(&format!(
            "TRUNCATE TABLE {}",
            helpers::table_ref(database, table)
        ))
        .await
    }

    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()> {
        self.ensure_writable()?;
        self.execute_sql(&format!(
            "RENAME TABLE {} TO {}",
            helpers::table_ref(database, table),
            helpers::table_ref(database, new_name)
        ))
        .await
    }

    /// ClickHouse databases have no charset, collation or owner to edit; the dialog only asks for
    /// the name, so there is nothing to load or diff.
    async fn database_options(&self, _name: &str) -> Result<DatabaseOptions> {
        Ok(DatabaseOptions::default())
    }

    async fn server_version(&self) -> Result<String> {
        self.client
            .query("SELECT version()")
            .fetch_one::<String>()
            .await
            .map_err(map_query_error)
    }

    async fn session_count(&self) -> Result<u64> {
        self.client
            .query("SELECT count() FROM system.processes")
            .fetch_one::<u64>()
            .await
            .map_err(map_query_error)
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        Ok(self
            .read_table_statuses(database)
            .await?
            .into_iter()
            .find(|(name, _)| name == table)
            .map(|(_, status)| status)
            .unwrap_or_default())
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        self.read_table_statuses(database).await
    }

    /// `system.tables.create_table_query` is the full `CREATE` statement, views included.
    async fn object_ddl(
        &self,
        database: &str,
        name: &str,
        _is_view: bool,
    ) -> Result<Option<String>> {
        let sql = format!(
            "SELECT create_table_query FROM system.tables WHERE database = {} AND name = {}",
            helpers::quote_literal(database),
            helpers::quote_literal(name)
        );
        let decoded = self.fetch_rows(&sql).await?;
        Ok(decoded
            .rows
            .first()
            .and_then(|row| text_at(&decoded.columns, row, "create_table_query"))
            .filter(|ddl| !ddl.trim().is_empty()))
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn collations(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn alter_database_options(
        &self,
        _name: &str,
        _original: &DatabaseOptions,
        _modified: &DatabaseOptions,
    ) -> Result<()> {
        Ok(())
    }

    fn alter_database_sql(
        &self,
        _name: &str,
        _original: &DatabaseOptions,
        _modified: &DatabaseOptions,
    ) -> String {
        String::new()
    }

    fn column_types(&self) -> Vec<&'static str> {
        vec![
            "Int8",
            "Int16",
            "Int32",
            "Int64",
            "Int128",
            "Int256",
            "UInt8",
            "UInt16",
            "UInt32",
            "UInt64",
            "UInt128",
            "UInt256",
            "Float32",
            "Float64",
            "Decimal(18, 4)",
            "Bool",
            "String",
            "FixedString(16)",
            "Date",
            "Date32",
            "DateTime",
            "DateTime64(3)",
            "UUID",
            "IPv4",
            "IPv6",
            "Nullable(String)",
            "LowCardinality(String)",
            "Array(String)",
            "Map(String, String)",
            "Tuple(String, Int64)",
            "Enum8('a' = 1, 'b' = 2)",
        ]
    }

    /// ClickHouse has no stored procedures or scheduled events; its UDFs are plain SQL functions
    /// created through the query editor.
    async fn list_routines(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn list_events(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn backup_object_metadata(
        &self,
        _database: &str,
        _kind: BackupObjectKind,
        _name: &str,
    ) -> Result<ObjectDump> {
        Err(Error::Query(
            "ClickHouse connections do not support .rgbak backups.".into(),
        ))
    }

    async fn stream_table_rows(
        &self,
        _database: &str,
        _table: &str,
        _on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        Err(Error::Query(
            "ClickHouse connections do not support .rgbak backups.".into(),
        ))
    }

    async fn restore_object(&self, _database: &str, _object: &ObjectDump) -> Result<()> {
        Err(Error::Query(
            "ClickHouse connections do not support .rgbak backups.".into(),
        ))
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        vec![
            "MergeTree",
            "ReplacingMergeTree",
            "SummingMergeTree",
            "AggregatingMergeTree",
            "CollapsingMergeTree",
            "VersionedCollapsingMergeTree",
            "ReplicatedMergeTree",
            "ReplicatedReplacingMergeTree",
            "ReplicatedSummingMergeTree",
            "ReplicatedAggregatingMergeTree",
            "Memory",
            "Log",
            "TinyLog",
            "StripeLog",
            "Buffer",
            "Distributed",
            "Merge",
            "Null",
            "URL",
            "File",
            "S3",
            "Kafka",
            "PostgreSQL",
            "MySQL",
        ]
    }

    /// The table designer is not offered for ClickHouse (MergeTree needs engine-specific clauses),
    /// so its introspection path is not used.
    async fn table_schema(&self, _database: &str, _table: &str) -> Result<TableSchema> {
        Err(Error::Query(
            "ClickHouse tables are managed through their CREATE statement in the SQL editor."
                .into(),
        ))
    }

    /// The import wizard creates a table with a plain `MergeTree`; see
    /// [`helpers::create_table_sql`].
    fn table_schema_sql(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        if original.is_some() {
            // The designer is unavailable, so only `CREATE` is ever produced.
            return String::new();
        }
        helpers::create_table_sql(database, table, modified)
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// One query-editor run's session: pins a ClickHouse `session_id` so `SET` carries across
/// statements, and tracks the current database itself so `USE` does too (every HTTP request names
/// its database, which would otherwise override the session's).
struct ClickHouseSession {
    client: Client,
    database: Option<String>,
}

impl ClickHouseSession {
    fn client_for_current_database(&self) -> Client {
        match &self.database {
            Some(database) if !database.trim().is_empty() => {
                self.client.clone().with_database(database.trim())
            }
            _ => self.client.clone(),
        }
    }
}

#[async_trait]
impl ScriptSession for ClickHouseSession {
    async fn execute(&mut self, sql: &str) -> Result<QueryResult> {
        if let Some(database) = use_database(sql) {
            self.database = Some(database);
            return Ok(QueryResult {
                statement: sql.trim().to_string(),
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: 0,
                has_result_set: false,
                last_insert_id: None,
            });
        }
        run_statement(&self.client_for_current_database(), sql).await
    }

    async fn execute_many(&mut self, sql: &str) -> Result<Vec<QueryResult>> {
        let mut results = Vec::new();
        for statement in rustgrid_core::split::split_statements(sql) {
            if statement.trim().is_empty() {
                continue;
            }
            let result = self.execute(&statement).await?;
            if result.has_result_set {
                results.push(result);
            }
        }
        Ok(results)
    }
}

/// Run one statement and decode its result set (or report that it had none).
async fn run_statement(client: &Client, sql: &str) -> Result<QueryResult> {
    let statement = sql.trim().trim_end_matches(';').trim().to_string();
    if statement.is_empty() {
        return Ok(QueryResult {
            statement,
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: 0,
            has_result_set: false,
            last_insert_id: None,
        });
    }
    if helpers::returns_result_set(&statement) {
        if helpers::has_format_clause(&statement) {
            return Err(Error::Query(
                "Remove the FORMAT clause: the query editor reads ClickHouse's JSON format itself."
                    .into(),
            ));
        }
        let query = helpers::ensure_row_format(&statement);
        let mut cursor = client
            .query_raw(&query)
            .fetch_bytes(helpers::ROW_FORMAT)
            .map_err(map_query_error)?;
        let body = cursor.collect().await.map_err(map_query_error)?;
        let decoded = helpers::decode_body(&body)?;
        Ok(QueryResult {
            statement,
            columns: decoded.columns,
            rows: decoded.rows,
            rows_affected: 0,
            has_result_set: true,
            last_insert_id: None,
        })
    } else {
        client
            .query_raw(&statement)
            .execute()
            .await
            .map_err(map_query_error)?;
        Ok(QueryResult {
            statement,
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: 0,
            has_result_set: false,
            last_insert_id: None,
        })
    }
}

/// `USE database` (optionally quoted), which the session applies to its following requests.
fn use_database(sql: &str) -> Option<String> {
    let rest = sql.trim_start();
    if !rest.get(..3)?.eq_ignore_ascii_case("use") {
        return None;
    }
    let after_keyword = &rest[3..];
    // `USER` and `USERS` start with the same letters; a statement separator must follow the word.
    if after_keyword.len() == after_keyword.trim_start().len() {
        return None;
    }
    let name = after_keyword.trim().trim_end_matches(';').trim();
    let name = name
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .or_else(|| {
            name.strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .unwrap_or(name)
        .trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("rustgrid-{nanos:x}")
}

async fn probe(client: &Client) -> std::result::Result<String, ClickHouseError> {
    client.query("SELECT version()").fetch_one::<String>().await
}

fn connect_error(error: ClickHouseError, host: &str, port: u16) -> Error {
    if is_auth_error(&error.to_string()) {
        Error::Authentication(error.to_string())
    } else {
        Error::Connection(format!("{host}:{port}: {error}"))
    }
}

/// Map a client error: authentication stays a distinct variant so the app opens its password
/// prompt, transport failures are connection errors, and server messages keep ClickHouse's own
/// text (which names the error code and the failing object).
pub(crate) fn map_query_error(error: ClickHouseError) -> Error {
    match &error {
        ClickHouseError::Network(_) => Error::Connection(error.to_string()),
        ClickHouseError::BadResponse(body) => {
            let body = body.trim();
            let body = body.chars().take(4_000).collect::<String>();
            if is_auth_error(&body) {
                Error::Authentication(body)
            } else {
                Error::Query(body)
            }
        }
        _ => Error::Query(error.to_string()),
    }
}

fn is_auth_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("authentication failed")
        || lower.contains("code: 516")
        || lower.contains("code: 194")
        || lower.contains("code: 192")
        || lower.contains("unauthorized")
}

/// Which ClickHouse column value a row carries, by name (the overview queries alias their columns).
fn cell_at<'a>(
    columns: &[ColumnInfo],
    row: &'a [rustgrid_core::CellValue],
    name: &str,
) -> Option<&'a rustgrid_core::CellValue> {
    columns
        .iter()
        .position(|column| column.name == name)
        .and_then(|index| row.get(index))
}

fn text_at(columns: &[ColumnInfo], row: &[rustgrid_core::CellValue], name: &str) -> Option<String> {
    match cell_at(columns, row, name)? {
        rustgrid_core::CellValue::Null => None,
        value => Some(value.as_display()),
    }
}

fn u64_at(columns: &[ColumnInfo], row: &[rustgrid_core::CellValue], name: &str) -> Option<u64> {
    match cell_at(columns, row, name)? {
        rustgrid_core::CellValue::Uint(value) => Some(*value),
        rustgrid_core::CellValue::Int(value) => u64::try_from(*value).ok(),
        rustgrid_core::CellValue::Float(value) => Some(*value as u64),
        rustgrid_core::CellValue::Text(text) => text.trim().parse().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// HTTP client and TLS
// ---------------------------------------------------------------------------

/// Build the ClickHouse client for one scheme. A fresh hyper client per client keeps the plain and
/// TLS variants independent; `clickhouse::Client` clones the transport imperatively.
fn build_client(
    scheme: &str,
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    database: &str,
    options: &ConnectionOptions,
) -> Result<Client> {
    let connector = https_connector(&options.tls, options.connect_timeout)?;
    let idle = options
        .keepalive
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(30));
    let hyper = HyperClient::builder(TokioExecutor::new())
        .pool_idle_timeout(idle)
        .build(connector);
    let url = format!("{scheme}://{}:{port}", bracket_host(host));
    let client = clickhouse::Client::with_http_client(hyper)
        .with_url(url)
        .with_database(database);
    Ok(apply_user(
        apply_settings(client, options),
        username,
        password,
    ))
}

fn bracket_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn apply_user(client: Client, username: &str, password: Option<&str>) -> Client {
    let username = if username.trim().is_empty() {
        "default"
    } else {
        username.trim()
    };
    client
        .with_user(username.to_string())
        .with_password(password.unwrap_or_default().to_string())
}

fn apply_settings(client: Client, options: &ConnectionOptions) -> Client {
    // Decimals and wide integers are quoted in JSON so their exact text survives the trip (a JSON
    // number would go through `f64`). Denormals are quoted rather than silently becoming `null`.
    let mut client = client
        .with_setting("output_format_json_quote_decimals", "1")
        .with_setting("output_format_json_quote_64bit_integers", "1")
        .with_setting("output_format_json_quote_denormals", "1");
    if let Some(seconds) = options.query_timeout.filter(|seconds| *seconds > 0) {
        client = client.with_setting("max_execution_time", seconds.to_string());
    }
    if options.read_only {
        // `readonly = 2` still rejects writes but lets the client send its own settings (`= 1`
        // forbids changing any setting, including the ones this driver just set).
        client = client.with_setting("readonly", "2");
    }
    client
}

/// The HTTPS connector, with the profile's CA, client certificate/key and verification mode. It
/// also accepts plain `http://` URLs, so one connector serves both schemes.
fn https_connector(
    tls: &rustgrid_core::TlsOptions,
    connect_timeout: Option<u64>,
) -> Result<HttpsConnector<HttpConnector>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| Error::Connection(error.to_string()))?;
    let builder = if tls.mode == TlsMode::Required {
        // Encrypt without verifying: `dangerous()` is only available before the verification
        // choice, so no root store is built on this path.
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerification(provider)))
    } else {
        let mut roots = rustls::RootCertStore::empty();
        if tls.ca.trim().is_empty() {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        } else {
            for certificate in load_certificates(tls.ca.trim())? {
                roots.add(certificate).map_err(|error| {
                    Error::Connection(format!("invalid CA certificate: {error}"))
                })?;
            }
        }
        builder.with_root_certificates(roots)
    };
    // `VerifyCa` verifies the CA chain; rustls always checks the host name as well, so the two
    // verifying modes coincide here (the stricter behaviour is the safe one).
    let config = if !tls.cert.trim().is_empty() {
        let certificates = load_certificates(tls.cert.trim())?;
        let key = load_private_key(tls.key.trim())?;
        builder
            .with_client_auth_cert(certificates, key)
            .map_err(|error| Error::Connection(format!("invalid client certificate: {error}")))?
    } else {
        builder.with_no_client_auth()
    };

    let mut http = HttpConnector::new();
    http.enforce_http(false);
    if let Some(seconds) = connect_timeout.filter(|seconds| *seconds > 0) {
        http.set_connect_timeout(Some(Duration::from_secs(seconds)));
    }
    http.set_keepalive(Some(Duration::from_secs(30)));
    Ok(hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_or_http()
        .enable_http1()
        .wrap_connector(http))
}

fn load_certificates(path: &str) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path)
        .map_err(|error| Error::Connection(format!("cannot open certificate {path}: {error}")))?;
    rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| Error::Connection(format!("cannot read certificate {path}: {error}")))
}

fn load_private_key(path: &str) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path)
        .map_err(|error| Error::Connection(format!("cannot open private key {path}: {error}")))?;
    rustls_pemfile::private_key(&mut BufReader::new(file))
        .map_err(|error| Error::Connection(format!("cannot read private key {path}: {error}")))?
        .ok_or_else(|| Error::Connection(format!("no private key found in {path}")))
}

/// Accept any server certificate: `TlsMode::Required` encrypts without verifying, matching the
/// other drivers' "require, do not verify" mode.
struct NoVerification(Arc<rustls::crypto::CryptoProvider>);

impl std::fmt::Debug for NoVerification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NoVerification")
    }
}

impl rustls::client::danger::ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signed,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            certificate,
            signed,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_statement_names_are_tracked() {
        assert_eq!(use_database("USE analytics"), Some("analytics".to_string()));
        assert_eq!(use_database("use `my db`;"), Some("my db".to_string()));
        assert_eq!(use_database("SELECT 1"), None);
        assert_eq!(use_database("USER"), None);
    }

    #[test]
    fn auth_errors_are_recognized() {
        assert!(is_auth_error(
            "Code: 516. DB::Exception: default: Authentication failed: password is incorrect"
        ));
        assert!(!is_auth_error(
            "Code: 60. DB::Exception: Table default.x doesn't exist"
        ));
    }

    #[test]
    fn ipv6_hosts_are_bracketed() {
        assert_eq!(bracket_host("::1"), "[::1]");
        assert_eq!(bracket_host("[::1]"), "[::1]");
        assert_eq!(bracket_host("localhost"), "localhost");
    }
}

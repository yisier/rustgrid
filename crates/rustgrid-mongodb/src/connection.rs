//! The MongoDB [`Connection`] implementation.
//!
//! A collection is presented as the app's "table": its columns are the union of the top-level
//! fields of a small document sample (BSON types included), one document is one grid row, and
//! `_id` is the primary key every edit and delete is keyed by. Documents keep their full structure
//! in the row form's JSON editor because embedded documents and arrays are rendered as Extended
//! JSON.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::TryStreamExt;
use mongodb::{
    Client, Collection, Database,
    bson::{Bson, Document, doc},
    error::{Error as MongoError, ErrorKind as MongoErrorKind},
    options::{ClientOptions, Tls, TlsOptions},
};
use rustgrid_core::{
    BackupObjectKind, CellValue, ColumnInfo, Connection, DatabaseInfo, DatabaseOptions, DriverId,
    Error, ObjectDump, ObjectKind, PageRequest, QueryResult, Result, RowInsert, RowUpdate,
    TableInfo, TablePage, TableSchema, TableStatus,
};
use rustgrid_tunnel::Tunnel;

use crate::{filter, value};

/// How many documents are sampled to infer a collection's field set.
const SCHEMA_SAMPLE: i64 = 50;
/// The largest page a grid may request (a guard against an accidental full-collection scan).
const MAX_PAGE_SIZE: u64 = 5_000;
/// How many rows a query-tab command may return before it is cut off.
const RESULT_LIMIT: usize = 5_000;
/// How many collections `table_statuses` gathers stats for in one pass.
const STATUS_LIMIT: usize = 500;

/// How the connection authenticates and which topology it expects (parsed once from the profile
/// options so every URI rebuild uses the same values).
#[derive(Debug, Clone, Default)]
struct MongoSettings {
    auth_source: String,
    auth_mechanism: String,
    replica_set: String,
    read_preference: String,
}

pub struct MongoConnection {
    client: Client,
    /// The profile's database, used as the default for query tabs that have none.
    default_database: Option<String>,
    /// Field sets inferred from samples, keyed by `database\0collection`.
    schemas: Mutex<HashMap<String, Arc<Vec<ColumnInfo>>>>,
    read_only: bool,
    /// Keeps the SSH/HTTP/SOCKS forwarder alive for the connection's lifetime.
    _tunnel: Option<Tunnel>,
}

impl MongoConnection {
    /// Open a client from a profile. The URI is assembled here (rather than stored) so credentials
    /// are percent-encoded exactly once and can never leak into a saved profile.
    pub(crate) async fn open(
        host: &str,
        port: u16,
        username: &str,
        password: Option<&str>,
        database: &str,
        options: &rustgrid_core::options::ConnectionOptions,
        profile_options: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self> {
        let settings = MongoSettings {
            auth_source: profile_options
                .get(crate::driver::OPT_AUTH_SOURCE)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "admin".to_string()),
            auth_mechanism: profile_options
                .get(crate::driver::OPT_AUTH_MECHANISM)
                .cloned()
                .unwrap_or_default(),
            replica_set: profile_options
                .get(crate::driver::OPT_REPLICA_SET)
                .map(|value| value.trim().to_string())
                .unwrap_or_default(),
            read_preference: profile_options
                .get(crate::driver::OPT_READ_PREFERENCE)
                .cloned()
                .unwrap_or_default(),
        };

        let mut target_host = if host.trim().is_empty() {
            "127.0.0.1".to_string()
        } else {
            host.trim().to_string()
        };
        let mut target_port = if port == 0 { 27017 } else { port };
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

        let uri = build_uri(
            &target_host,
            target_port,
            username,
            password,
            database,
            &settings,
        );
        let mut client_options = ClientOptions::parse(&uri)
            .await
            .map_err(|error| connect_error(error, &target_host, target_port))?;
        client_options.app_name = Some("RustGrid".to_string());
        if let Some(seconds) = options.connect_timeout {
            client_options.connect_timeout = Some(Duration::from_secs(seconds.max(1)));
        }
        client_options.tls = tls_options(&options.tls, tunnel.is_some())?;

        let client = Client::with_options(client_options)
            .map_err(|error| connect_error(error, &target_host, target_port))?;
        // Verify the handshake now, so a wrong password surfaces as `Error::Authentication` (the
        // app then prompts) instead of on the first catalog call.
        client
            .database(&settings.auth_source)
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|error| connect_error(error, &target_host, target_port))?;

        Ok(Self {
            client,
            default_database: (!database.trim().is_empty()).then(|| database.trim().to_string()),
            schemas: Mutex::new(HashMap::new()),
            read_only: options.read_only,
            _tunnel: tunnel,
        })
    }

    fn database(&self, name: &str) -> Database {
        self.client.database(name)
    }

    fn collection(&self, database: &str, collection: &str) -> Collection<Document> {
        self.database(database).collection(collection)
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.read_only {
            return Err(Error::Query(
                "This connection is read-only; enable writes in the connection settings.".into(),
            ));
        }
        Ok(())
    }

    fn cache_key(database: &str, collection: &str) -> String {
        format!("{database}\u{0}{collection}")
    }

    fn cached(&self, key: &str) -> Option<Arc<Vec<ColumnInfo>>> {
        self.schemas
            .lock()
            .ok()
            .and_then(|schemas| schemas.get(key).cloned())
    }

    fn store(&self, key: String, columns: Arc<Vec<ColumnInfo>>) {
        if let Ok(mut schemas) = self.schemas.lock() {
            schemas.insert(key, columns);
        }
    }

    fn forget(&self, database: &str, collection: &str) {
        if let Ok(mut schemas) = self.schemas.lock() {
            schemas.remove(&Self::cache_key(database, collection));
        }
    }

    /// Infer a collection's field set from a sample of documents.
    async fn infer_columns(&self, database: &str, collection: &str) -> Result<Vec<ColumnInfo>> {
        let mut cursor = self
            .collection(database, collection)
            .find(doc! {})
            .limit(SCHEMA_SAMPLE)
            .await
            .map_err(query_error)?;
        let mut documents = Vec::new();
        while let Some(document) = cursor.try_next().await.map_err(query_error)? {
            documents.push(document);
        }

        let mut columns: Vec<ColumnInfo> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for document in &documents {
            for (name, item) in document {
                match index.get(name) {
                    Some(&position) => {
                        if matches!(item, Bson::Null | Bson::Undefined) {
                            columns[position].nullable = true;
                        }
                    }
                    None => {
                        index.insert(name.clone(), columns.len());
                        columns.push(ColumnInfo {
                            name: name.clone(),
                            data_type: value::type_name(item).to_string(),
                            nullable: matches!(item, Bson::Null | Bson::Undefined),
                            primary_key: name == "_id",
                            auto_increment: false,
                            generated: false,
                            comment: String::new(),
                        });
                    }
                }
            }
            // Every field that existed before this document but is absent from it is nullable.
            for column in &mut columns {
                if !document.contains_key(&column.name) {
                    column.nullable = true;
                }
            }
        }

        if let Some(position) = columns.iter().position(|column| column.name == "_id") {
            let id = columns.remove(position);
            columns.insert(0, id);
        } else {
            // An empty collection (or one whose sample lacked `_id`) still gets the key column, so
            // the grid has something to select and the insert path knows where `_id` goes.
            columns.insert(
                0,
                ColumnInfo {
                    name: "_id".to_string(),
                    data_type: "objectId".to_string(),
                    nullable: false,
                    primary_key: true,
                    auto_increment: false,
                    generated: false,
                    comment: String::new(),
                },
            );
        }

        Ok(columns)
    }

    async fn schema(&self, database: &str, collection: &str) -> Result<Arc<Vec<ColumnInfo>>> {
        let key = Self::cache_key(database, collection);
        if let Some(columns) = self.cached(&key) {
            return Ok(columns);
        }
        let columns = Arc::new(self.infer_columns(database, collection).await?);
        self.store(key, columns.clone());
        Ok(columns)
    }

    /// The BSON value for one edited cell, using the column's inferred type.
    fn parse_for(columns: &[ColumnInfo], field: &str, text: &str) -> Bson {
        let column_type = columns
            .iter()
            .find(|column| column.name == field)
            .map(|column| column.data_type.as_str())
            .unwrap_or("");
        value::from_text(column_type, text)
            .or_else(|_| value::from_text("", text))
            .unwrap_or_else(|_| Bson::String(text.to_string()))
    }

    fn key_document(keys: &[(String, Option<String>)], columns: &[ColumnInfo]) -> Document {
        let mut document = Document::new();
        for (field, value) in keys {
            let bson = match value {
                Some(text) => Self::parse_for(columns, field, text),
                None => Bson::Null,
            };
            document.insert(field.clone(), bson);
        }
        document
    }

    /// Run one JSON command from the query tab.
    async fn run_json(&self, database: Option<&str>, text: &str) -> Result<QueryResult> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(Error::Query("请输入一条 JSON 命令。".into()));
        }
        let parsed = value::parse_json(trimmed).map_err(Error::Query)?;
        let command = match parsed {
            Bson::Document(document) => document,
            _ => {
                return Err(Error::Query(
                    "MongoDB 查询需要一条 JSON 命令对象，例如 {\"find\": \"users\", \"limit\": 20}。"
                        .into(),
                ));
            }
        };
        let name = command
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| Error::Query("命令对象不能为空。".into()))?;
        let database = database
            .map(str::to_string)
            .or_else(|| self.default_database.clone())
            .ok_or_else(|| Error::Query("请先选择一个数据库。".into()))?;
        let db = self.database(&database);

        if self.read_only
            && !matches!(
                name.as_str(),
                "find"
                    | "aggregate"
                    | "count"
                    | "distinct"
                    | "listCollections"
                    | "listDatabases"
                    | "listIndexes"
                    | "collStats"
                    | "dbStats"
                    | "explain"
                    | "ping"
                    | "buildInfo"
                    | "serverStatus"
                    | "getParameter"
            )
        {
            return Err(Error::Query(
                "This connection is read-only; only read commands are allowed.".into(),
            ));
        }

        match name.as_str() {
            "find" => {
                let collection_name = command
                    .get_str("find")
                    .map_err(|error| Error::Query(format!("find 需要一个集合名：{error}")))?;
                let filter = command.get_document("filter").cloned().unwrap_or_default();
                let collection = self.collection(&database, collection_name);
                let mut find = collection.find(filter);
                if let Some(skip) = command_i64(&command, "skip") {
                    find = find.skip(skip.max(0) as u64);
                }
                if let Some(limit) = command_i64(&command, "limit") {
                    find = find.limit(limit.clamp(1, RESULT_LIMIT as i64));
                } else {
                    find = find.limit(RESULT_LIMIT as i64);
                }
                if let Ok(sort) = command.get_document("sort") {
                    find = find.sort(sort.clone());
                }
                if let Ok(projection) = command.get_document("projection") {
                    find = find.projection(projection.clone());
                }
                let cursor = find.await.map_err(query_error)?;
                let documents = collect(cursor, RESULT_LIMIT).await?;
                Ok(result_from_documents(trimmed, &documents))
            }
            "aggregate" => {
                let collection_name = command
                    .get_str("aggregate")
                    .map_err(|error| Error::Query(format!("aggregate 需要一个集合名：{error}")))?;
                let pipeline: Vec<Document> = command
                    .get_array("pipeline")
                    .map_err(|error| {
                        Error::Query(format!("aggregate 需要 pipeline 数组：{error}"))
                    })?
                    .iter()
                    .filter_map(|stage| stage.as_document().cloned())
                    .collect();
                if self.read_only
                    && pipeline
                        .iter()
                        .any(|stage| stage.contains_key("$out") || stage.contains_key("$merge"))
                {
                    return Err(Error::Query(
                        "This connection is read-only; $out/$merge cannot run.".into(),
                    ));
                }
                let cursor = self
                    .collection(&database, collection_name)
                    .aggregate(pipeline)
                    .await
                    .map_err(query_error)?;
                let documents = collect(cursor, RESULT_LIMIT).await?;
                Ok(result_from_documents(trimmed, &documents))
            }
            "count" => {
                let collection_name = command
                    .get_str("count")
                    .map_err(|error| Error::Query(format!("count 需要一个集合名：{error}")))?;
                let filter = command
                    .get_document("query")
                    .or_else(|_| command.get_document("filter"))
                    .cloned()
                    .unwrap_or_default();
                let total = self
                    .collection(&database, collection_name)
                    .count_documents(filter)
                    .await
                    .map_err(query_error)?;
                Ok(QueryResult {
                    statement: trimmed.to_string(),
                    columns: vec![column("n", "long")],
                    rows: vec![vec![CellValue::Int(total as i64)]],
                    rows_affected: 0,
                    has_result_set: true,
                    last_insert_id: None,
                })
            }
            "distinct" => {
                let collection_name = command
                    .get_str("distinct")
                    .map_err(|error| Error::Query(format!("distinct 需要一个集合名：{error}")))?;
                let field = command
                    .get_str("key")
                    .map_err(|error| Error::Query(format!("distinct 需要 key：{error}")))?;
                let filter = command.get_document("query").cloned().unwrap_or_default();
                let items = self
                    .collection(&database, collection_name)
                    .distinct(field, filter)
                    .await
                    .map_err(query_error)?;
                let rows = items
                    .iter()
                    .map(|item| vec![value::to_cell(item)])
                    .collect();
                Ok(QueryResult {
                    statement: trimmed.to_string(),
                    columns: vec![column(field, "auto")],
                    rows,
                    rows_affected: 0,
                    has_result_set: true,
                    last_insert_id: None,
                })
            }
            "listCollections" => {
                let mut specifications = db.list_collections().await.map_err(query_error)?;
                let mut documents: Vec<Document> = Vec::new();
                while let Some(specification) =
                    specifications.try_next().await.map_err(query_error)?
                {
                    if let Ok(document) = mongodb::bson::serialize_to_document(&specification) {
                        documents.push(document);
                    }
                }
                Ok(result_from_documents(trimmed, &documents))
            }
            "listDatabases" => {
                let names = self
                    .client
                    .list_database_names()
                    .await
                    .map_err(query_error)?;
                let rows = names
                    .into_iter()
                    .map(|name| vec![CellValue::Text(name)])
                    .collect();
                Ok(QueryResult {
                    statement: trimmed.to_string(),
                    columns: vec![column("name", "string")],
                    rows,
                    rows_affected: 0,
                    has_result_set: true,
                    last_insert_id: None,
                })
            }
            _ => {
                let reply = db.run_command(command.clone()).await.map_err(query_error)?;
                let affected = command_i64(&reply, "n")
                    .or_else(|| command_i64(&reply, "nModified"))
                    .map(|value| value.max(0) as u64)
                    .unwrap_or(0);
                let write = matches!(
                    name.as_str(),
                    "insert"
                        | "update"
                        | "delete"
                        | "findAndModify"
                        | "create"
                        | "drop"
                        | "collMod"
                        | "createIndexes"
                        | "dropIndexes"
                        | "renameCollection"
                        | "createUser"
                        | "updateUser"
                        | "dropUser"
                        | "createRole"
                        | "updateRole"
                        | "dropRole"
                        | "grantRolesToUser"
                        | "revokeRolesFromUser"
                        | "grantPrivilegesToRole"
                        | "revokePrivilegesFromRole"
                );
                if write {
                    return Ok(QueryResult {
                        statement: trimmed.to_string(),
                        columns: Vec::new(),
                        rows: Vec::new(),
                        rows_affected: affected,
                        has_result_set: false,
                        last_insert_id: None,
                    });
                }
                let columns = columns_for_documents(std::slice::from_ref(&reply));
                let rows = vec![row_for_document(&reply, &columns)];
                Ok(QueryResult {
                    statement: trimmed.to_string(),
                    columns,
                    rows,
                    rows_affected: affected,
                    has_result_set: true,
                    last_insert_id: None,
                })
            }
        }
    }

    async fn status_for(&self, database: &str, collection: &str, view: bool) -> TableStatus {
        let stats = self
            .database(database)
            .run_command(doc! { "collStats": collection, "scale": 1 })
            .await;
        match stats {
            Ok(stats) => TableStatus {
                engine: Some(if view { "view" } else { "collection" }.to_string()),
                rows: command_i64(&stats, "count").map(|value| value as u64),
                data_length: command_i64(&stats, "storageSize")
                    .or_else(|| command_i64(&stats, "size"))
                    .map(|value| value as u64),
                index_length: command_i64(&stats, "totalIndexSize").map(|value| value as u64),
                row_format: stats
                    .get_bool("capped")
                    .ok()
                    .filter(|capped| *capped)
                    .map(|_| "capped".to_string()),
                ..TableStatus::default()
            },
            Err(_) => TableStatus {
                engine: Some(if view { "view" } else { "collection" }.to_string()),
                ..TableStatus::default()
            },
        }
    }
}

#[async_trait]
impl Connection for MongoConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("mongodb")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        let mut names = self
            .client
            .list_database_names()
            .await
            .map_err(query_error)?;
        names.retain(|name| !matches!(name.as_str(), "admin" | "config" | "local"));
        names.sort_by_key(|name| name.to_ascii_lowercase());
        Ok(names
            .into_iter()
            .map(|name| DatabaseInfo { name })
            .collect())
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        let mut cursor = self
            .database(database)
            .list_collections()
            .await
            .map_err(query_error)?;
        let mut specifications = Vec::new();
        while let Some(specification) = cursor.try_next().await.map_err(query_error)? {
            specifications.push(specification);
        }
        let mut tables: Vec<TableInfo> = specifications
            .into_iter()
            .filter(|specification| !specification.name.starts_with("system."))
            .map(|specification| TableInfo {
                name: specification.name,
                kind: match specification.collection_type {
                    mongodb::results::CollectionType::View => ObjectKind::View,
                    _ => ObjectKind::Table,
                },
                updatable: false,
            })
            .collect();
        tables.sort_by_key(|table| table.name.to_ascii_lowercase());
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        Ok(self.schema(database, table).await?.as_ref().clone())
    }

    async fn fetch_page(
        &self,
        database: &str,
        table: &str,
        page: PageRequest,
    ) -> Result<TablePage> {
        let columns = self.schema(database, table).await?;
        let query = filter::build(&page.filter, &columns);
        let page_index = page.page;
        let page_size = page.page_size.clamp(1, MAX_PAGE_SIZE);

        let mut sort = Document::new();
        for column in &page.order_by {
            sort.insert(
                column.column.clone(),
                if column.descending {
                    Bson::Int32(-1)
                } else {
                    Bson::Int32(1)
                },
            );
        }
        // A stable tie-breaker keeps pagination coherent when the sort column repeats.
        if !sort.contains_key("_id") {
            sort.insert("_id", Bson::Int32(1));
        }

        let collection = self.collection(database, table);
        let find = collection
            .find(query.clone())
            .sort(sort)
            .skip(page_index.saturating_mul(page_size))
            .limit(page_size as i64);
        let cursor = find.await.map_err(query_error)?;
        let documents = collect(cursor, page_size as usize).await?;

        let rows = documents
            .iter()
            .map(|document| {
                columns
                    .iter()
                    .map(|column| {
                        value::get_path(document, &column.name)
                            .map(value::to_cell)
                            .unwrap_or(CellValue::Null)
                    })
                    .collect()
            })
            .collect();

        let total_rows = if page.skip_total {
            None
        } else {
            self.collection(database, table)
                .count_documents(query)
                .await
                .ok()
        };

        Ok(TablePage {
            columns: columns.as_ref().clone(),
            rows,
            page: page_index,
            page_size,
            total_rows,
        })
    }

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        self.ensure_writable()?;
        let columns = self.schema(database, table).await?;
        let collection = self.collection(database, table);
        for update in updates {
            let keys = Self::key_document(&update.keys, &columns);
            if keys.is_empty() {
                return Err(Error::Query("缺少定位文档的 _id。".into()));
            }
            let mut set = Document::new();
            for (field, text) in &update.set {
                if field == "_id" {
                    // `_id` is immutable in MongoDB; a grid edit to it is ignored rather than
                    // failing the whole save.
                    continue;
                }
                let bson = match text {
                    Some(text) => Self::parse_for(&columns, field, text),
                    None => Bson::Null,
                };
                set.insert(field.clone(), bson);
            }
            if set.is_empty() {
                continue;
            }
            collection
                .update_one(keys, doc! { "$set": set })
                .await
                .map_err(query_error)?;
        }
        Ok(())
    }

    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        self.ensure_writable()?;
        let columns = self.schema(database, table).await?;
        let collection = self.collection(database, table);
        for row in rows {
            let mut document = Document::new();
            for (field, text) in &row.values {
                // An untouched `_id` cell arrives as an empty string; leave it out so the server
                // generates the key.
                if field == "_id" && text.as_deref().unwrap_or("").trim().is_empty() {
                    continue;
                }
                let bson = match text {
                    Some(text) => Self::parse_for(&columns, field, text),
                    None => Bson::Null,
                };
                document.insert(field.clone(), bson);
            }
            collection.insert_one(document).await.map_err(query_error)?;
        }
        // A new document may add fields; drop the sampled field set so the next reload shows them.
        self.forget(database, table);
        Ok(())
    }

    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()> {
        self.ensure_writable()?;
        let columns = self.schema(database, table).await?;
        let collection = self.collection(database, table);
        for key in keys {
            let document = Self::key_document(key, &columns);
            if document.is_empty() {
                return Err(Error::Query("缺少定位文档的 _id。".into()));
            }
            collection.delete_one(document).await.map_err(query_error)?;
        }
        Ok(())
    }

    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        self.run_json(database, sql).await
    }

    /// A MongoDB "statement" is one JSON command; never split it on semicolons (they appear in
    /// strings and are not statement separators).
    fn split_statements(&self, sql: &str) -> Vec<String> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![trimmed.to_string()]
        }
    }

    async fn create_database(&self, _name: &str, _options: &DatabaseOptions) -> Result<()> {
        Err(Error::Query(
            "MongoDB 不单独创建空数据库：它在第一个集合写入时出现，请用 {\"create\": \"集合名\"} 新建集合。"
                .into(),
        ))
    }

    fn create_database_sql(&self, _name: &str, _options: &DatabaseOptions) -> String {
        String::new()
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        self.ensure_writable()?;
        self.database(name).drop().await.map_err(query_error)
    }

    async fn drop_table(&self, database: &str, table: &str) -> Result<()> {
        self.ensure_writable()?;
        self.collection(database, table)
            .drop()
            .await
            .map_err(query_error)?;
        self.forget(database, table);
        Ok(())
    }

    async fn empty_table(&self, database: &str, table: &str) -> Result<()> {
        self.ensure_writable()?;
        self.collection(database, table)
            .delete_many(doc! {})
            .await
            .map_err(query_error)?;
        Ok(())
    }

    async fn truncate_table(&self, database: &str, table: &str) -> Result<()> {
        self.empty_table(database, table).await
    }

    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()> {
        self.ensure_writable()?;
        self.client
            .database("admin")
            .run_command(doc! {
                "renameCollection": format!("{database}.{table}"),
                "to": format!("{database}.{new_name}"),
                "dropTarget": false,
            })
            .await
            .map_err(query_error)?;
        self.forget(database, table);
        Ok(())
    }

    async fn database_options(&self, _name: &str) -> Result<DatabaseOptions> {
        Ok(DatabaseOptions::default())
    }

    async fn server_version(&self) -> Result<String> {
        let reply = self
            .client
            .database("admin")
            .run_command(doc! { "buildInfo": 1 })
            .await
            .map_err(query_error)?;
        reply
            .get_str("version")
            .map(str::to_string)
            .map_err(|error| Error::Query(format!("buildInfo 缺少 version：{error}")))
    }

    async fn session_count(&self) -> Result<u64> {
        // `serverStatus` needs a privileged account; a limited user still gets a working
        // connection, just without a session count.
        let reply = match self
            .client
            .database("admin")
            .run_command(doc! { "serverStatus": 1 })
            .await
        {
            Ok(reply) => reply,
            Err(_) => return Ok(0),
        };
        Ok(reply
            .get_document("connections")
            .ok()
            .and_then(|connections| command_i64(connections, "current"))
            .unwrap_or(0)
            .max(0) as u64)
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        let view = self
            .list_tables(database)
            .await
            .ok()
            .and_then(|tables| {
                tables
                    .into_iter()
                    .find(|info| info.name == table)
                    .map(|info| info.kind == ObjectKind::View)
            })
            .unwrap_or(false);
        Ok(self.status_for(database, table, view).await)
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        let tables = self.list_tables(database).await?;
        let mut statuses = Vec::new();
        for table in tables.iter().take(STATUS_LIMIT) {
            let status = self
                .status_for(database, &table.name, table.kind == ObjectKind::View)
                .await;
            statuses.push((table.name.clone(), status));
        }
        Ok(statuses)
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
        Err(Error::Query(
            "MongoDB 没有可编辑的数据库字符集/排序规则选项。".into(),
        ))
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
            "string",
            "int",
            "long",
            "double",
            "decimal",
            "bool",
            "date",
            "timestamp",
            "objectId",
            "object",
            "array",
            "binData",
            "uuid",
            "regex",
            "null",
        ]
    }

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
        Err(Error::Query("MongoDB 连接不支持 .rgbak 备份。".into()))
    }

    async fn stream_table_rows(
        &self,
        _database: &str,
        _table: &str,
        _on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        Err(Error::Query("MongoDB 连接不支持 .rgbak 备份。".into()))
    }

    async fn restore_object(&self, _database: &str, _object: &ObjectDump) -> Result<()> {
        Err(Error::Query("MongoDB 连接不支持 .rgbak 备份。".into()))
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn table_schema(&self, _database: &str, _table: &str) -> Result<TableSchema> {
        Err(Error::Query(
            "MongoDB 集合没有关系型表结构，请在查询页用 JSON 命令管理集合。".into(),
        ))
    }

    fn table_schema_sql(
        &self,
        _database: &str,
        _table: &str,
        _original: Option<&TableSchema>,
        _modified: &TableSchema,
    ) -> String {
        String::new()
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// URI and TLS
// ---------------------------------------------------------------------------

fn build_uri(
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    database: &str,
    settings: &MongoSettings,
) -> String {
    let mut uri = String::from("mongodb://");
    let anonymous = settings.auth_mechanism.eq_ignore_ascii_case("none");
    let username = username.trim();
    if !username.is_empty() && !anonymous {
        uri.push_str(&percent_encode(username));
        if let Some(password) = password {
            uri.push(':');
            uri.push_str(&percent_encode(password));
        }
        uri.push('@');
    }
    uri.push_str(host);
    uri.push(':');
    uri.push_str(&port.to_string());
    uri.push('/');
    if !database.trim().is_empty() {
        uri.push_str(&percent_encode(database.trim()));
    }
    let mut parameters: Vec<String> = Vec::new();
    if !username.is_empty() && !anonymous {
        parameters.push(format!(
            "authSource={}",
            percent_encode(&settings.auth_source)
        ));
    }
    if !settings.replica_set.is_empty() {
        parameters.push(format!(
            "replicaSet={}",
            percent_encode(&settings.replica_set)
        ));
    }
    if !settings.read_preference.is_empty() {
        parameters.push(format!(
            "readPreference={}",
            percent_encode(&settings.read_preference)
        ));
    }
    parameters.push("retryWrites=true".to_string());
    parameters.push("appName=RustGrid".to_string());
    uri.push('?');
    uri.push_str(&parameters.join("&"));
    uri
}

/// Percent-encode the characters that are reserved inside a MongoDB connection string.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn tls_options(
    options: &rustgrid_core::options::TlsOptions,
    through_tunnel: bool,
) -> Result<Option<Tls>> {
    use rustgrid_core::options::TlsMode;

    if options.mode == TlsMode::Disabled && options.ca.is_empty() && options.cert.is_empty() {
        return Ok(None);
    }
    let verifying = matches!(options.mode, TlsMode::VerifyCa | TlsMode::VerifyIdentity)
        || (options.mode == TlsMode::Disabled && !options.ca.trim().is_empty());
    if through_tunnel && verifying {
        return Err(Error::Config(
            "通过 SSH/HTTP/SOCKS 隧道连接 MongoDB 时无法校验证书主机名，请改用“需要 TLS（不校验证书）”或关闭校验。"
                .into(),
        ));
    }
    let mut tls = TlsOptions::builder()
        .allow_invalid_certificates(!verifying)
        .build();
    if !options.ca.trim().is_empty() {
        tls.ca_file_path = Some(options.ca.trim().into());
    }
    if !options.cert.trim().is_empty() {
        tls.cert_key_file_path = Some(options.cert.trim().into());
    }
    Ok(Some(Tls::Enabled(tls)))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn connect_error(error: MongoError, host: &str, port: u16) -> Error {
    let text = error.to_string();
    if is_auth_error(&error) || looks_like_auth(&text) {
        Error::Authentication(text)
    } else {
        Error::Connection(format!("无法连接到 MongoDB {host}:{port}：{text}"))
    }
}

fn query_error(error: MongoError) -> Error {
    if is_auth_error(&error) || looks_like_auth(&error.to_string()) {
        Error::Authentication(error.to_string())
    } else {
        Error::Query(error.to_string())
    }
}

fn is_auth_error(error: &MongoError) -> bool {
    match error.kind.as_ref() {
        MongoErrorKind::Authentication { .. } => true,
        MongoErrorKind::Command(command) => matches!(command.code, 13 | 18),
        _ => false,
    }
}

fn looks_like_auth(message: &str) -> bool {
    let message = message.to_ascii_uppercase();
    message.contains("AUTHENTICATION FAILED")
        || message.contains("AUTHENTICATIONFAILED")
        || message.contains("SCRAM")
        || message.contains("REQUIRES AUTHENTICATION")
        || message.contains("UNAUTHORIZED")
}

// ---------------------------------------------------------------------------
// Result-set helpers
// ---------------------------------------------------------------------------

async fn collect(mut cursor: mongodb::Cursor<Document>, limit: usize) -> Result<Vec<Document>> {
    let mut documents = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(query_error)? {
        documents.push(document);
        if documents.len() >= limit {
            break;
        }
    }
    Ok(documents)
}

/// The union of the top-level fields of every row, `_id` first.
fn columns_for_documents(documents: &[Document]) -> Vec<ColumnInfo> {
    let mut names: Vec<String> = Vec::new();
    for document in documents {
        for name in document.keys() {
            if !names.iter().any(|existing| existing == name) {
                names.push(name.clone());
            }
        }
    }
    if let Some(position) = names.iter().position(|name| name == "_id") {
        let id = names.remove(position);
        names.insert(0, id);
    }
    names
        .into_iter()
        .map(|name| {
            let data_type = documents
                .iter()
                .find_map(|document| document.get(&name).map(value::type_name))
                .unwrap_or("string");
            ColumnInfo {
                primary_key: name == "_id",
                data_type: data_type.to_string(),
                nullable: true,
                auto_increment: false,
                generated: false,
                comment: String::new(),
                name,
            }
        })
        .collect()
}

fn row_for_document(document: &Document, columns: &[ColumnInfo]) -> Vec<CellValue> {
    columns
        .iter()
        .map(|column| {
            value::get_path(document, &column.name)
                .map(value::to_cell)
                .unwrap_or(CellValue::Null)
        })
        .collect()
}

fn result_from_documents(statement: &str, documents: &[Document]) -> QueryResult {
    let columns = columns_for_documents(documents);
    let rows = documents
        .iter()
        .map(|document| row_for_document(document, &columns))
        .collect();
    QueryResult {
        statement: statement.to_string(),
        columns,
        rows,
        rows_affected: 0,
        has_result_set: true,
        last_insert_id: None,
    }
}

fn column(name: &str, data_type: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        data_type: data_type.to_string(),
        nullable: true,
        primary_key: false,
        auto_increment: false,
        generated: false,
        comment: String::new(),
    }
}

/// Read an integer that may be encoded as int32, int64 or double.
fn command_i64(document: &Document, key: &str) -> Option<i64> {
    match document.get(key)? {
        Bson::Int32(value) => Some(i64::from(*value)),
        Bson::Int64(value) => Some(*value),
        Bson::Double(value) => Some(*value as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_encodes_credentials_and_options() {
        let settings = MongoSettings {
            auth_source: "admin".into(),
            auth_mechanism: String::new(),
            replica_set: "rs0".into(),
            read_preference: "secondaryPreferred".into(),
        };
        let uri = build_uri(
            "127.0.0.1",
            27017,
            "root",
            Some("P@ss:1"),
            "test",
            &settings,
        );
        assert!(uri.starts_with("mongodb://root:P%40ss%3A1@127.0.0.1:27017/test?"));
        assert!(uri.contains("authSource=admin"));
        assert!(uri.contains("replicaSet=rs0"));
        assert!(uri.contains("readPreference=secondaryPreferred"));
    }

    #[test]
    fn auth_mechanism_none_omits_credentials() {
        let settings = MongoSettings {
            auth_mechanism: "None".into(),
            ..MongoSettings::default()
        };
        let uri = build_uri("db.local", 27017, "root", Some("secret"), "", &settings);
        assert_eq!(
            uri,
            "mongodb://db.local:27017/?retryWrites=true&appName=RustGrid"
        );
    }

    #[test]
    fn document_columns_put_id_first() {
        let documents = vec![
            doc! { "name": "a", "_id": 1i32, "age": 3i32 },
            doc! { "name": "b", "_id": 2i32, "active": true },
        ];
        let columns = columns_for_documents(&documents);
        assert_eq!(columns[0].name, "_id");
        assert_eq!(columns[0].data_type, "int");
        assert!(columns.iter().any(|column| column.name == "active"));
    }
}

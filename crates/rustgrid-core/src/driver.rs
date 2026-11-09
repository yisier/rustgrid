use async_trait::async_trait;

use crate::capability::DriverCapabilities;
use crate::descriptor::{ConnectionFormSpec, DriverDescriptor, DriverIconStyle, TableDesignerSpec};
use crate::dialect::DriverDialect;
use crate::error::{Error, Result};
use crate::model::{
    BackupObjectKind, CollectionDefinition, ColumnInfo, ConnectionConfig, ConnectionProfile,
    DatabaseInfo, DatabaseOptions, DriverId, ObjectCreate, ObjectDump, ObjectNamespace,
    PageRequest, QueryResult, RowInsert, RowUpdate, TableInfo, TablePage, TableSchema, TableStatus,
};
use crate::routine::{RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};
use crate::user::{
    AccountKind, ObjectPrivilegeRow, PrivilegeCatalog, ServerSecurableGrant, UserAccount,
    UserDetails, UserEdit, UserEditSection, UserMapping,
};
use crate::view::{ViewDetails, ViewEdit};

#[async_trait]
pub trait Driver: Send + Sync {
    fn id(&self) -> DriverId;

    fn display_name(&self) -> String;

    fn default_port(&self) -> u16;

    /// The username pre-filled by a New Connection form for this engine, or `""` when the engine
    /// has no conventional default account (the field then starts empty). It is only a starting
    /// value: every engine accepts any user name.
    fn default_username(&self) -> &'static str {
        ""
    }

    /// Presentation/capability metadata for this driver, used by the New Connection menu, the
    /// connection-tree icon and the UI's capability checks.
    ///
    /// The default reproduces the historical trait methods, so a driver gains the descriptor
    /// without changing behavior; engines override it as they are migrated.
    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: self.id(),
            display_name: self.display_name(),
            default_port: self.default_port(),
            is_file_based: self.is_file_based(),
            icon: "icons/connection.svg",
            icon_style: DriverIconStyle::Plain,
            capabilities: DriverCapabilities::from_flags(
                self.supports_database_management(),
                self.supports_users(),
                self.supports_routines(),
                self.supports_schemas(),
            ),
            database_editor: self.database_editor(),
            connection_form: ConnectionFormSpec::default(),
            table_designer: TableDesignerSpec::default(),
            order: 1000,
        }
    }

    /// Whether this engine connects to a database file rather than a network server. File-based
    /// engines ignore host/port/username/password and take the file path as the profile's
    /// `database`.
    fn is_file_based(&self) -> bool {
        false
    }

    /// The capabilities of one saved connection.
    ///
    /// `descriptor()` describes the driver as a whole. A driver whose server software can run in
    /// more than one mode (the MySQL driver's StarRocks compatibility mode) overrides this so the
    /// UI hides the features that mode cannot provide. The default is the descriptor's static set.
    fn capabilities_for(&self, _profile: &ConnectionProfile) -> DriverCapabilities {
        self.descriptor().capabilities
    }

    /// Whether this connection may open the table designer. A compatibility mode whose server
    /// cannot execute the engine's table DDL (MySQL's StarRocks mode) returns `false`, so the UI
    /// hides the designer and its 新建表 action instead of opening a page whose save would fail.
    fn supports_table_designer(&self, _profile: &ConnectionProfile) -> bool {
        true
    }

    /// Whether this connection may use RustGrid's `.rgbak` backup/restore. A server that cannot
    /// replay the engine's table DDL (MySQL's StarRocks mode) returns `false`, so the Backup tab
    /// and the tree's 备份 category stay unavailable.
    fn supports_backup(&self, _profile: &ConnectionProfile) -> bool {
        true
    }

    /// The SQL dialect the engine speaks, used by the UI to pick engine-specific data (today the
    /// built-in function list offered by completion) without matching on the engine id.
    fn dialect(&self) -> DriverDialect {
        DriverDialect::Generic
    }

    /// The dialect of one saved connection. A driver whose single descriptor covers several engines
    /// (the generic ODBC driver) overrides this so completion, the dump writer and the paging hints
    /// follow the engine the connection actually reaches. The default is the static dialect.
    fn dialect_for(&self, _profile: &ConnectionProfile) -> DriverDialect {
        self.dialect()
    }

    /// The system drivers offered by the connection form's driver dropdown (ODBC). Engines without
    /// such a list return an empty one, and the form falls back to a free-text field.
    fn connection_drivers(&self) -> Vec<String> {
        Vec::new()
    }

    /// Whether the engine can create/alter/drop databases. SQLite cannot (a database is a file),
    /// so the app hides the database-management actions for it.
    fn supports_database_management(&self) -> bool {
        true
    }

    /// Whether the engine has server accounts and privileges. SQLite does not, so the Users tab is
    /// disabled for it.
    fn supports_users(&self) -> bool {
        true
    }

    /// Whether the engine has stored routines (functions/procedures). SQLite does not, so the
    /// Functions tab and tree category are hidden for it.
    fn supports_routines(&self) -> bool {
        true
    }

    /// Whether the engine has schemas as a first-class object (SQL Server). When true the tree
    /// nests databases under their schemas and offers 新建模式 / 删除模式.
    fn supports_schemas(&self) -> bool {
        false
    }

    /// Which fields and tabs the New/Edit Database dialog shows for this engine. The default is
    /// MySQL's charset + collation.
    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec {
            charset: true,
            collation: true,
            ..Default::default()
        }
    }

    /// Which fields and sections the New/Edit User window shows for this engine, and the privilege
    /// sets it offers. The default is MySQL's account model (user@host, an authentication plugin,
    /// resource limits and the full privilege set); engines without those concepts hide them.
    fn user_editor(&self) -> UserEditorSpec {
        UserEditorSpec::mysql()
    }

    /// The recovery models offered by the database dialog (SQL Server). Empty hides the field.
    fn database_recovery_models(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// The compatibility levels offered by the database dialog (SQL Server). Empty hides the field.
    fn database_compatibility_levels(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>>;
}

/// Which fields the New/Edit Database dialog shows for an engine, and which extra (currently
/// informational) tabs sit between 常规 and SQL 预览. Keeping this on the driver is what lets the
/// dialog stay engine-agnostic while SQL Server shows its owner/recovery/compatibility options.
#[derive(Debug, Clone, Default)]
pub struct DatabaseEditorSpec {
    /// Whether the charset dropdown is shown (MySQL/MariaDB).
    pub charset: bool,
    /// Whether the collation dropdown is shown.
    pub collation: bool,
    /// Whether the owner dropdown is shown (SQL Server).
    pub owner: bool,
    /// Whether the recovery-model dropdown is shown (SQL Server).
    pub recovery_model: bool,
    /// Whether the compatibility-level dropdown is shown (SQL Server).
    pub compatibility_level: bool,
    /// Whether the New Database dialog also asks for the first collection (document stores:
    /// MongoDB creates a database together with its first collection). Only shown while creating.
    pub first_collection: bool,
    /// Extra informational tabs shown in order after 常规.
    pub extra_tabs: Vec<DatabaseEditorTab>,
}

/// An informational tab of the Edit Database dialog that has no editable state yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseEditorTab {
    Filegroups,
    Files,
    Advanced,
    Comment,
}

/// Which fields and sections the New/Edit User window shows for an engine. The default
/// ([`UserEditorSpec::mysql`]) is MySQL's account model; other engines turn off the parts their
/// server has no equivalent for, so the editor never shows a control that cannot do anything.
///
/// The privileges themselves come from the driver's [`PrivilegeCatalog`]
/// ([`Connection::privilege_catalog`]), not from this spec.
#[derive(Debug, Clone)]
pub struct UserEditorSpec {
    /// The account kinds the editor offers, in 类型 dropdown order. Engines that do not separate
    /// users from roles (and engines with no accounts at all) leave the single-entry default: the
    /// 常规 page then shows no 类型 control and the Users list no 类型 column.
    pub kinds: Vec<AccountKind>,
    /// The user@host identity row and its host quick chips (MySQL/MariaDB).
    pub host: bool,
    /// The authentication-plugin dropdown.
    pub authentication_plugin: bool,
    /// The password-expiry policy row.
    pub password_expiry: bool,
    /// The password-valid-until row (PostgreSQL's `VALID UNTIL`).
    pub password_valid_until: bool,
    /// The 锁定该账号 checkbox.
    pub account_lock: bool,
    /// When set, the account-lock checkbox is labelled 已启用 and reads `!account_locked`
    /// (SQL Server's login `is_disabled`, which is the inverse of a lock).
    pub account_enabled: bool,
    /// Whether the 高级 section (MySQL's `REQUIRE`/SSL type, cipher, issuer, subject) is shown.
    pub ssl: bool,
    /// The 最大问题数 resource limit (MySQL).
    pub max_questions: bool,
    /// The 最大更新数 resource limit (MySQL).
    pub max_updates: bool,
    /// The 最大连接数 resource limit (MySQL, PostgreSQL).
    pub max_connections: bool,
    /// The 最大用户连接数 resource limit (MySQL).
    pub max_user_connections: bool,
    /// The Oracle PROFILE row.
    pub profile: bool,
    /// The Oracle DEFAULT TABLESPACE row.
    pub default_tablespace: bool,
    /// The Oracle tablespace-quota row.
    pub tablespace_quota: bool,
    /// Whether the 超级用户 column is meaningful in the Users account list.
    pub list_super_user: bool,
    /// Whether the 服务器权限 section is shown.
    pub server_privileges: bool,
    /// Whether the 权限 (object-grant) section is shown.
    pub object_privileges: bool,
    /// Whether the 默认权限 (default-privileges) section is shown (PostgreSQL's
    /// `ALTER DEFAULT PRIVILEGES`).
    pub default_privileges: bool,
    /// Whether the 对象权限管理器 toolbar item is offered. It may be `true` while
    /// `object_privileges` is `false` (SQL Server manages object grants only through the manager).
    pub object_privilege_manager: bool,
    /// Whether a save must finish with `FLUSH PRIVILEGES` (MySQL/MariaDB reload the grant tables;
    /// other engines apply GRANT/REVOKE immediately).
    pub flush_privileges: bool,
    /// Whether the 角色 section is shown.
    pub roles: bool,
    /// Whether the 用户映射 section is shown (SQL Server's login → database user mappings and
    /// database roles).
    pub user_mapping: bool,
    /// Whether the 登录信息 section shows the verification type (SQL Server's Windows / SQL Server
    /// authentication).
    pub verification_type: bool,
    /// Whether the 终端节点权限 section is shown (SQL Server's endpoint permissions).
    pub endpoint_permissions: bool,
    /// Whether the 登录权限 section is shown (SQL Server's per-login securable permissions).
    pub login_permissions: bool,
}

impl UserEditorSpec {
    /// MySQL/MariaDB: every field. Accounts can be users or roles (MySQL 8 / MariaDB 10.0.5+), so
    /// the 常规 page offers a 类型 choice.
    pub fn mysql() -> Self {
        Self {
            kinds: vec![AccountKind::User, AccountKind::Role],
            host: true,
            authentication_plugin: true,
            password_expiry: true,
            password_valid_until: false,
            account_lock: true,
            account_enabled: false,
            ssl: true,
            max_questions: true,
            max_updates: true,
            max_connections: true,
            max_user_connections: true,
            profile: false,
            default_tablespace: false,
            tablespace_quota: false,
            list_super_user: true,
            server_privileges: true,
            object_privileges: true,
            default_privileges: false,
            object_privilege_manager: true,
            flush_privileges: true,
            roles: true,
            user_mapping: false,
            verification_type: false,
            endpoint_permissions: false,
            login_permissions: false,
        }
    }
}

/// One pinned database session for running a SQL script statement by statement.
///
/// [`Connection::execute_query`] may borrow a different pooled connection on each call, so a
/// session-scoped setting made by an earlier statement (MySQL's `SET FOREIGN_KEY_CHECKS` or
/// `USE db`, a `BEGIN`, PostgreSQL's `SET search_path`, SQL Server's `SET IDENTITY_INSERT`, …)
/// would be lost before the next one runs. A script session keeps one connection checked out so
/// that state carries across the whole script.
#[async_trait]
pub trait ScriptSession: Send {
    /// Run one statement on the pinned connection and report its outcome.
    async fn execute(&mut self, sql: &str) -> Result<QueryResult>;

    /// Run one statement and return every result set it produced (a stored procedure's `CALL`
    /// can return several). The default returns [`ScriptSession::execute`]'s single result.
    async fn execute_many(&mut self, sql: &str) -> Result<Vec<QueryResult>> {
        Ok(vec![self.execute(sql).await?])
    }
}

/// What a live stream from the server should watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchSpec {
    /// Every command the server executes (`MONITOR`).
    Monitor,
    /// The listed pub/sub channels and patterns.
    Subscribe {
        channels: Vec<String>,
        patterns: Vec<String>,
    },
}

/// One item of a live server stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    /// The pub/sub channel (or the matched pattern) the message arrived on; `"monitor"` for a
    /// `MONITOR` line.
    pub channel: String,
    /// The message payload or the monitor line.
    pub text: String,
}

/// A live stream from the server (pub/sub messages, `MONITOR` lines), read item by item.
#[async_trait]
pub trait Watch: Send {
    /// The next event, or `None` when the server closed the stream.
    async fn next(&mut self) -> Result<Option<WatchEvent>>;
}

#[async_trait]
pub trait Connection: Send + Sync {
    fn driver_id(&self) -> DriverId;

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>>;

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>>;

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>>;

    /// Fetch one page of a table. `page.order_by` is engine-agnostic: implementations must
    /// translate it to their own ordering syntax (e.g. SQL `ORDER BY`) and must keep row order
    /// stable across pages so pagination stays coherent.
    async fn fetch_page(&self, database: &str, table: &str, page: PageRequest)
    -> Result<TablePage>;

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()>;

    /// Insert new rows into `table`. Each row carries the columns to set; columns not listed take
    /// their default (or stay unset, e.g. for `AUTO_INCREMENT`). A `None` value is an explicit
    /// `NULL`. Implementations should insert atomically.
    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()>;

    /// Insert rows for the Import Wizard, returning `(inserted, updated)`.
    ///
    /// A document store keys rows by a user-visible `_id`, so re-importing a collection's own export
    /// would fail every row with a duplicate-key error. MongoDB updates a document whose `_id`
    /// already exists (setting the mapped fields, so unmapped fields survive) and counts it as
    /// updated. Every other engine inserts, and the default reports all rows as inserted.
    async fn import_rows(
        &self,
        database: &str,
        table: &str,
        rows: &[RowInsert],
    ) -> Result<(usize, usize)> {
        self.insert_rows(database, table, rows).await?;
        Ok((rows.len(), 0))
    }

    /// Delete the rows identified by `keys`: each entry is one row's key columns
    /// (`(column, value)` pairs, `None` meaning SQL `NULL`). Implementations should delete
    /// atomically.
    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()>;

    /// The full document behind one row, as text, for a **document-store** engine (MongoDB).
    ///
    /// The grid's columns are only a projection of the fields a sample found; this is the
    /// authoritative row, which the row form shows in its JSON view and lets the user edit as a
    /// whole (including fields the sample never saw). `keys` uses the same shape as
    /// [`Connection::delete_rows`]. Engines whose rows are not documents keep the default `None`
    /// and the form never offers that view.
    async fn row_document(
        &self,
        _database: &str,
        _table: &str,
        _keys: &[(String, Option<String>)],
    ) -> Result<Option<String>> {
        Ok(None)
    }

    /// Replace the document identified by `keys` with `document` (text in the same shape
    /// [`Connection::row_document`] returned). The identity columns may not change and a document
    /// may add or remove other fields. Only document-store engines implement it.
    async fn replace_row_document(
        &self,
        _database: &str,
        _table: &str,
        _keys: &[(String, Option<String>)],
        _document: &str,
    ) -> Result<()> {
        Err(Error::Query(
            "This engine cannot replace a row from a JSON document.".into(),
        ))
    }

    /// Run an arbitrary SQL statement (or script) in the context of `database`, if given.
    ///
    /// Implementations must use the text protocol so that DDL and other statements that
    /// cannot be prepared still execute.
    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult>;

    /// Run an arbitrary SQL script, returning one [`QueryResult`] per statement that produced a
    /// result set (in order). A script with a single statement yields one entry; a `SELECT` with
    /// zero rows still yields an entry so the grid can show its columns. The default
    /// implementation runs the whole string as one statement.
    async fn execute_query_many(
        &self,
        database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        Ok(vec![self.execute_query(database, sql).await?])
    }

    /// Split a SQL script into individual statements, for the "Run SQL File" feature, so the app
    /// can run (and report on) one statement at a time. The default cuts on top-level semicolons
    /// while respecting string literals, quoted identifiers and comments; engines with a
    /// block-structured dialect override it with their own scanner.
    fn split_statements(&self, sql: &str) -> Vec<String> {
        crate::split::split_statements(sql)
    }

    /// Open a pinned session for running a SQL script statement by statement (see
    /// [`ScriptSession`]), so session-scoped state carries across statements. Drivers that cannot
    /// pin a connection return `None`; the caller then falls back to per-statement
    /// [`Connection::execute_query`].
    async fn script_session(
        &self,
        _database: Option<&str>,
    ) -> Result<Option<Box<dyn ScriptSession>>> {
        Ok(None)
    }

    /// Create a database with the given engine-agnostic options. Fields the engine does not use
    /// (empty strings) are omitted, letting the engine pick its defaults.
    async fn create_database(&self, name: &str, options: &DatabaseOptions) -> Result<()>;

    /// The `CREATE DATABASE` statement [`Connection::create_database`] runs, for the dialog's SQL
    /// preview.
    fn create_database_sql(&self, name: &str, options: &DatabaseOptions) -> String;

    async fn drop_database(&self, name: &str) -> Result<()>;

    /// Drop a table.
    async fn drop_table(&self, database: &str, table: &str) -> Result<()>;

    /// Delete every row of a table (DML, so it is transactional/logged and can be rolled back).
    async fn empty_table(&self, database: &str, table: &str) -> Result<()>;

    /// Truncate a table (DDL): much faster than [`Connection::empty_table`] but cannot be rolled
    /// back on most engines.
    async fn truncate_table(&self, database: &str, table: &str) -> Result<()>;

    /// Rename a table within its database.
    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()>;

    /// Load a database's current editable options (charset/collation/owner/recovery/…), so the
    /// Edit Database dialog can show them and diff the user's changes.
    async fn database_options(&self, name: &str) -> Result<DatabaseOptions>;

    /// The SQL Server server logins offered as database owners. Engines without an owner concept
    /// return an empty list.
    async fn database_owners(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// The server's version string (e.g. `8.0.31`), for the connection info pane.
    async fn server_version(&self) -> Result<String>;

    /// The number of client sessions currently connected to the server, for the connection info
    /// pane. Engines without a session table may return `0`.
    async fn session_count(&self) -> Result<u64>;

    /// A `SHOW TABLE STATUS`-style summary of one table, for the table info pane. Engines without
    /// an equivalent should return [`TableStatus::default`].
    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus>;

    /// A `SHOW TABLE STATUS`-style overview of every table in a database, keyed by name, for the
    /// object list's 详细列表. Engines without an equivalent return an empty list.
    async fn table_statuses(&self, _database: &str) -> Result<Vec<(String, TableStatus)>> {
        Ok(Vec::new())
    }

    /// The `CREATE`/DDL script for one table or view, for the info pane's DDL view. `is_view`
    /// distinguishes a view from a table where the engine stores both in the same catalog. Engines
    /// that cannot reproduce a script return `None`.
    async fn object_ddl(
        &self,
        _database: &str,
        _name: &str,
        _is_view: bool,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    // ----- Key-value engines ---------------------------------------------------------------------
    //
    // Engines whose objects are keys (Redis) expose a paged listing, namespaces, key creation, TTL
    // and live server streams here. The defaults keep the SQL engines unchanged.

    /// List one page of objects, with an opaque `cursor` for the follow-up page.
    ///
    /// `pattern` narrows the listing (a namespace filter); `None` lists everything. The default
    /// ignores paging and returns [`Connection::list_tables`] with no cursor.
    async fn list_objects_page(
        &self,
        database: &str,
        _cursor: Option<&str>,
        _pattern: Option<&str>,
    ) -> Result<(Vec<TableInfo>, Option<String>)> {
        Ok((self.list_tables(database).await?, None))
    }

    /// The namespaces (key prefixes) of a key-value database, with their object counts.
    async fn list_namespaces(&self, _database: &str) -> Result<Vec<ObjectNamespace>> {
        Ok(Vec::new())
    }

    /// Create an object (a key). Engines without key objects keep the default error.
    async fn create_object(&self, _database: &str, name: &str, _spec: &ObjectCreate) -> Result<()> {
        Err(Error::Query(format!(
            "this driver cannot create {name}: it has no key objects"
        )))
    }

    /// The object's expiry in seconds: `Some(-1)` when it never expires, `None` when the engine has
    /// no TTL concept.
    async fn object_ttl(&self, _database: &str, _name: &str) -> Result<Option<i64>> {
        Ok(None)
    }

    /// Set (or, with `None`, remove) an object's expiry.
    async fn set_object_ttl(&self, _database: &str, name: &str, _ttl: Option<i64>) -> Result<()> {
        Err(Error::Query(format!(
            "this driver cannot set a TTL on {name}"
        )))
    }

    /// Open a live server stream (pub/sub messages or `MONITOR` lines). `None` when the driver has
    /// no live streams.
    async fn watch(
        &self,
        _database: Option<&str>,
        _spec: &WatchSpec,
    ) -> Result<Option<Box<dyn Watch>>> {
        Ok(None)
    }

    async fn character_sets(&self) -> Result<Vec<String>>;

    async fn collations(&self) -> Result<Vec<String>>;

    /// Apply the options that changed between `original` and `modified`. The driver decides which
    /// fields it supports and emits only the necessary `ALTER DATABASE` statements.
    async fn alter_database_options(
        &self,
        name: &str,
        original: &DatabaseOptions,
        modified: &DatabaseOptions,
    ) -> Result<()>;

    /// The script [`Connection::alter_database_options`] runs, for the dialog's SQL preview.
    fn alter_database_sql(
        &self,
        name: &str,
        original: &DatabaseOptions,
        modified: &DatabaseOptions,
    ) -> String;

    /// List a database's schemas, ordered by name. Engines without schemas return an empty list.
    /// Used by the connection tree so a newly created (still empty) schema is visible.
    async fn list_schemas(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// Create a schema in a database (SQL Server).
    async fn create_schema(&self, _database: &str, _schema: &str) -> Result<()> {
        Err(Error::Query(
            "schema management is not supported by this driver".to_string(),
        ))
    }

    /// Drop a schema from a database (SQL Server). Engines reject dropping a non-empty schema.
    async fn drop_schema(&self, _database: &str, _schema: &str) -> Result<()> {
        Err(Error::Query(
            "schema management is not supported by this driver".to_string(),
        ))
    }

    /// The column types this engine offers in the table designer's type list, in display order.
    fn column_types(&self) -> Vec<&'static str>;

    /// List the stored routines (functions and procedures) of a database, ordered by name, for
    /// the backup object tree.
    async fn list_routines(&self, database: &str) -> Result<Vec<String>>;

    /// List the scheduled events of a database, ordered by name, for the backup object tree.
    async fn list_events(&self, database: &str) -> Result<Vec<String>>;

    /// Introspect one object for a backup: its `CREATE` statement, column names and
    /// `CREATE TRIGGER` statements. The returned `rows` is always empty; tables stream their
    /// rows separately through [`Connection::stream_table_rows`], and views/functions/events
    /// have no data. Engines that do not support a kind should return an empty dump rather than
    /// failing.
    async fn backup_object_metadata(
        &self,
        database: &str,
        kind: BackupObjectKind,
        name: &str,
    ) -> Result<ObjectDump>;

    /// Stream a table's rows, in order, as pre-rendered SQL value tuples such as
    /// `(1, 'a', NULL)`, calling `on_row` once per row and returning the row count.
    ///
    /// Implementations must not buffer the whole table: rows are handed to `on_row` as they are
    /// decoded. An error returned by `on_row` (for example a failed backup write) aborts the
    /// stream and is propagated to the caller.
    async fn stream_table_rows(
        &self,
        database: &str,
        table: &str,
        on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64>;

    /// Restore one dumped object into `database`, replacing any object with the same name.
    async fn restore_object(&self, database: &str, object: &ObjectDump) -> Result<()>;

    /// The storage engines offered in the table designer's Options tab, in display order.
    fn storage_engines(&self) -> Vec<&'static str>;

    /// The editable definition of a document-store collection, or `None` when the engine has no
    /// such concept. The UI offers 设计集合 only when this returns `Some`.
    async fn collection_definition(
        &self,
        _database: &str,
        _collection: &str,
    ) -> Result<Option<CollectionDefinition>> {
        Ok(None)
    }

    /// Create a document-store collection.
    async fn create_collection(
        &self,
        _database: &str,
        _definition: &CollectionDefinition,
    ) -> Result<()> {
        Err(Error::Query(
            "This engine cannot create a collection.".into(),
        ))
    }

    /// Alter a document-store collection. `original` is its current name, so a rename is applied
    /// before the rest of the definition.
    async fn alter_collection(
        &self,
        _database: &str,
        _original: &str,
        _definition: &CollectionDefinition,
    ) -> Result<()> {
        Err(Error::Query(
            "This engine cannot alter a collection.".into(),
        ))
    }

    /// Introspect the full definition of an existing table (or view).
    async fn table_schema(&self, database: &str, table: &str) -> Result<TableSchema>;

    /// Build the DDL script that turns `original` into `modified`. When `original` is `None`
    /// the script creates the table; when it is `Some` it alters the table to match `modified`.
    /// The script is empty when nothing changed. It is used both for the designer's SQL preview
    /// and, via [`Connection::save_table_schema`], to apply the change.
    fn table_schema_sql(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String;

    /// Trigger statements that accompany a table change. They are separate from
    /// [`Connection::table_schema_sql`] because a trigger body may contain semicolons and must be
    /// sent to the server as one query; the default save runs them individually after the table
    /// DDL.
    fn table_schema_trigger_sql(
        &self,
        _database: &str,
        _table: &str,
        _original: Option<&TableSchema>,
        _modified: &TableSchema,
    ) -> Vec<String> {
        Vec::new()
    }

    /// The complete script the designer's SQL preview shows: table DDL plus trigger statements.
    fn table_schema_script(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        let mut sql = self.table_schema_sql(database, table, original, modified);
        let triggers = self.table_schema_trigger_sql(database, table, original, modified);
        if !triggers.is_empty() {
            if !sql.trim().is_empty() {
                sql.push_str(";\n");
            }
            sql.push_str(&triggers.join(";\n"));
        }
        sql
    }

    /// Apply the change described by [`Connection::table_schema_sql`] and
    /// [`Connection::table_schema_trigger_sql`].
    async fn save_table_schema(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> Result<()> {
        let sql = self.table_schema_sql(database, table, original, modified);
        if !sql.trim().is_empty() {
            self.execute_query(Some(database), &sql).await?;
        }
        for statement in self.table_schema_trigger_sql(database, table, original, modified) {
            self.execute_query(Some(database), &statement).await?;
        }
        Ok(())
    }

    async fn close(&self) -> Result<()>;

    // ----- Account (user/role) management --------------------------------------------------------
    //
    // Engine-agnostic account administration used by the Users main tab and the user editor.
    // Drivers without an equivalent capability keep the defaults, which report the feature as
    // unsupported rather than silently succeeding.

    /// List every account (user and role) on the server, for the Users tab.
    async fn list_users(&self) -> Result<Vec<UserAccount>> {
        Err(Error::Query(
            "user management is not supported by this driver".to_string(),
        ))
    }

    /// Whether this **server** can hold roles at all, so the Users tab only offers the 用户 / 角色
    /// switch where the 角色 list can actually be filled (and `CREATE ROLE` would not fail).
    ///
    /// Engines whose role support depends on the server version override this: MySQL 8 and MariaDB
    /// have roles, MySQL 5.7 and earlier do not. Oracle, SQL Server and PostgreSQL always do.
    async fn roles_supported(&self) -> bool {
        true
    }

    /// The privileges this engine can grant, for the account editor and the object-privilege
    /// manager. Drivers without account management return an empty catalog.
    fn privilege_catalog(&self) -> PrivilegeCatalog {
        PrivilegeCatalog::default()
    }

    /// Load one account's editable state: its attributes, granted server privileges, role edges
    /// and object-level grants.
    async fn user_details(&self, _user: &str, _host: &str) -> Result<UserDetails> {
        Err(Error::Query(
            "user management is not supported by this driver".to_string(),
        ))
    }

    /// The account's per-database mappings (SQL Server's 用户映射): one row per database, each
    /// with whether the account has a database user there and the database roles it belongs to.
    /// The editor's 用户映射 section uses this for a new account (whose `user_details` would
    /// fail); engines without database-scoped users return an empty list.
    async fn user_mappings(&self, _user: &str, _host: &str) -> Result<Vec<UserMapping>> {
        Ok(Vec::new())
    }

    /// The account's permissions on every server-level securable (SQL Server's endpoints and
    /// logins), one row per available securable, for the editor's 终端节点权限 / 登录权限
    /// sections. Engines without such securables return an empty list.
    async fn user_securables(&self, _user: &str, _host: &str) -> Result<Vec<ServerSecurableGrant>> {
        Ok(Vec::new())
    }

    /// The login verification types offered by the editor's 验证类型 dropdown (SQL Server's
    /// `SQL Server` / `Windows`). Empty hides the control.
    fn login_types(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// The SQL script [`Connection::save_user`] runs, for the editor's SQL preview.
    fn user_edit_sql(&self, _edit: &UserEdit) -> String {
        String::new()
    }

    /// The statements [`Connection::save_user`] runs, grouped by what they change, so the editor's
    /// confirmation dialog can annotate each group. Empty when the driver has no user management.
    fn user_edit_groups(&self, _edit: &UserEdit) -> Vec<(UserEditSection, Vec<String>)> {
        Vec::new()
    }

    /// Create or alter an account, and replace its server privileges, role memberships and
    /// object grants with the edit's.
    async fn save_user(&self, _edit: &UserEdit) -> Result<()> {
        Err(Error::Query(
            "user management is not supported by this driver".to_string(),
        ))
    }

    /// Drop an account.
    async fn drop_user(&self, _user: &str, _host: &str) -> Result<()> {
        Err(Error::Query(
            "user management is not supported by this driver".to_string(),
        ))
    }

    /// Rename an account (`RENAME USER 'user'@'host' TO 'new_user'@'new_host'`), keeping its grants.
    async fn rename_user(
        &self,
        _user: &str,
        _host: &str,
        _new_user: &str,
        _new_host: &str,
    ) -> Result<()> {
        Err(Error::Query(
            "user management is not supported by this driver".to_string(),
        ))
    }

    /// The authentication plugins offered by the account editor's Plugin dropdown.
    fn authentication_plugins(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// The SSL types offered by the account editor's SSL type dropdown.
    fn ssl_types(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// The schemas the account editor's 默认权限 section lists, in the database the driver reads
    /// and writes default privileges in. Engines without default privileges return an empty list.
    async fn default_privilege_schemas(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// Every account's privileges on one object (`database` plus an optional table/routine name,
    /// empty for a database-wide grant), for the privilege manager's matrix.
    async fn object_privilege_matrix(
        &self,
        _database: &str,
        _name: &str,
    ) -> Result<Vec<ObjectPrivilegeRow>> {
        Err(Error::Query(
            "privilege management is not supported by this driver".to_string(),
        ))
    }

    /// Replace the object-level privileges of the listed accounts on one object. Only that
    /// object's privileges are touched; every other grant is left alone. An account absent from
    /// `rows` keeps its current grants.
    async fn set_object_privileges(
        &self,
        _database: &str,
        _name: &str,
        _rows: &[ObjectPrivilegeRow],
    ) -> Result<()> {
        Err(Error::Query(
            "privilege management is not supported by this driver".to_string(),
        ))
    }

    /// The SQL script that [`Connection::set_object_privileges`] would run for one object, given
    /// its previously loaded rows and the edited ones. Used by the privilege manager's preview.
    fn object_privileges_sql(
        &self,
        _database: &str,
        _name: &str,
        _original: &[ObjectPrivilegeRow],
        _rows: &[ObjectPrivilegeRow],
    ) -> String {
        String::new()
    }

    // ----- Stored routines (functions and procedures) -------------------------------------------
    //
    // Engine-agnostic routine administration used by the Functions main tab and its editor.
    // Drivers without an equivalent capability keep the defaults, which report the feature as
    // unsupported rather than silently succeeding.

    /// List a database's stored routines (functions and procedures), ordered by name, with the
    /// metadata the routine list and the editor's 信息 tab show.
    async fn list_routine_infos(&self, _database: &str) -> Result<Vec<RoutineInfo>> {
        Err(Error::Query(
            "routine management is not supported by this driver".to_string(),
        ))
    }

    /// Load one stored routine's full `CREATE` statement and session settings.
    async fn routine_details(
        &self,
        _database: &str,
        _kind: RoutineKind,
        _name: &str,
    ) -> Result<RoutineDetails> {
        Err(Error::Query(
            "routine management is not supported by this driver".to_string(),
        ))
    }

    /// The SQL script [`Connection::save_routine`] runs, for the editor's SQL preview.
    /// `original` names the routine to drop first when it already exists.
    fn routine_sql(
        &self,
        _database: &str,
        _original: Option<(&str, RoutineKind)>,
        _edit: &RoutineEdit,
    ) -> String {
        String::new()
    }

    /// Create or replace a stored routine in `database`. `original` names the existing routine to
    /// drop first (a routine cannot always be replaced in place), when it already exists.
    async fn save_routine(
        &self,
        _database: &str,
        _original: Option<(&str, RoutineKind)>,
        _edit: &RoutineEdit,
    ) -> Result<()> {
        Err(Error::Query(
            "routine management is not supported by this driver".to_string(),
        ))
    }

    /// Drop a stored routine.
    async fn drop_routine(&self, _database: &str, _kind: RoutineKind, _name: &str) -> Result<()> {
        Err(Error::Query(
            "routine management is not supported by this driver".to_string(),
        ))
    }

    // ----- Views ---------------------------------------------------------------------------------
    //
    // Engine-agnostic view administration used by the Views main tab and its designer. Drivers
    // without an equivalent capability keep the defaults, which report the feature as unsupported
    // rather than silently succeeding.

    /// Load one view's full `CREATE` statement and creation settings.
    async fn view_details(&self, _database: &str, _name: &str) -> Result<ViewDetails> {
        Err(Error::Query(
            "view management is not supported by this driver".to_string(),
        ))
    }

    /// The SQL script [`Connection::save_view`] runs, for the designer's SQL 预览 page.
    /// `original` names the view to drop first when it already exists.
    fn view_sql(&self, _database: &str, _original: Option<&str>, _edit: &ViewEdit) -> String {
        String::new()
    }

    /// Create or replace a view in `database`. `original` names the existing view to drop first,
    /// when it already exists.
    async fn save_view(
        &self,
        _database: &str,
        _original: Option<&str>,
        _edit: &ViewEdit,
    ) -> Result<()> {
        Err(Error::Query(
            "view management is not supported by this driver".to_string(),
        ))
    }

    /// Drop a view.
    async fn drop_view(&self, _database: &str, _name: &str) -> Result<()> {
        Err(Error::Query(
            "view management is not supported by this driver".to_string(),
        ))
    }
}

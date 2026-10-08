use async_trait::async_trait;

use crate::capability::DriverCapabilities;
use crate::descriptor::{ConnectionFormSpec, DriverDescriptor, DriverIconStyle};
use crate::dialect::DriverDialect;
use crate::error::{Error, Result};
use crate::model::{
    BackupObjectKind, ColumnInfo, ConnectionConfig, DatabaseInfo, DatabaseOptions, DriverId,
    ObjectDump, PageRequest, QueryResult, RowInsert, RowUpdate, TableInfo, TablePage, TableSchema,
    TableStatus,
};
use crate::routine::{RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};
use crate::user::{
    ObjectPrivilegeRow, PrivilegeCatalog, ServerSecurableGrant, UserAccount, UserDetails, UserEdit,
    UserEditSection, UserMapping,
};
use crate::view::{ViewDetails, ViewEdit};

#[async_trait]
pub trait Driver: Send + Sync {
    fn id(&self) -> DriverId;

    fn display_name(&self) -> String;

    fn default_port(&self) -> u16;

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
            order: 1000,
        }
    }

    /// Whether this engine connects to a database file rather than a network server. File-based
    /// engines ignore host/port/username/password and take the file path as the profile's
    /// `database`.
    fn is_file_based(&self) -> bool {
        false
    }

    /// The SQL dialect the engine speaks, used by the UI to pick engine-specific data (today the
    /// built-in function list offered by completion) without matching on the engine id.
    fn dialect(&self) -> DriverDialect {
        DriverDialect::Generic
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
    /// MySQL/MariaDB: every field.
    pub fn mysql() -> Self {
        Self {
            host: true,
            authentication_plugin: true,
            password_expiry: true,
            password_valid_until: false,
            account_lock: true,
            account_enabled: false,
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

    /// Delete the rows identified by `keys`: each entry is one row's key columns
    /// (`(column, value)` pairs, `None` meaning SQL `NULL`). Implementations should delete
    /// atomically.
    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()>;

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

    /// Apply the change described by [`Connection::table_schema_sql`].
    async fn save_table_schema(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> Result<()> {
        let sql = self.table_schema_sql(database, table, original, modified);
        if sql.trim().is_empty() {
            return Ok(());
        }
        self.execute_query(Some(database), &sql).await.map(|_| ())
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

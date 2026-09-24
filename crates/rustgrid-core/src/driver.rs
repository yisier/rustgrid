use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::model::{
    BackupObjectKind, ColumnInfo, ConnectionConfig, DatabaseInfo, DriverId, ObjectDump,
    PageRequest, QueryResult, RowInsert, RowUpdate, TableInfo, TablePage, TableSchema, TableStatus,
};
use crate::routine::{RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};
use crate::user::{ObjectPrivilegeRow, UserAccount, UserDetails, UserEdit, UserEditSection};
use crate::view::{ViewDetails, ViewEdit};

#[async_trait]
pub trait Driver: Send + Sync {
    fn id(&self) -> DriverId;

    fn display_name(&self) -> String;

    fn default_port(&self) -> u16;

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>>;
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

    /// Create a database. `charset`/`collation` are omitted when `None`, letting the engine pick
    /// its defaults.
    async fn create_database(
        &self,
        name: &str,
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> Result<()>;

    /// The `CREATE DATABASE` statement [`Connection::create_database`] runs, for the dialog's SQL
    /// preview. `charset`/`collation` are omitted when `None`.
    fn create_database_sql(
        &self,
        name: &str,
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> String;

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

    async fn database_defaults(&self, name: &str) -> Result<(String, String)>;

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

    async fn character_sets(&self) -> Result<Vec<String>>;

    async fn collations(&self) -> Result<Vec<String>>;

    async fn alter_database_defaults(
        &self,
        name: &str,
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> Result<()>;

    fn alter_database_sql(
        &self,
        name: &str,
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> String;

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

    /// Load one account's editable state: its attributes, granted server privileges, role edges
    /// and object-level grants.
    async fn user_details(&self, _user: &str, _host: &str) -> Result<UserDetails> {
        Err(Error::Query(
            "user management is not supported by this driver".to_string(),
        ))
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

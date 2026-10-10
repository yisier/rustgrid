//! Live SQL Server integration test. Ignored by default because it needs a real server.
//!
//! Enable it by setting `RUSTGRID_SQLSERVER_HOST` (and optionally
//! `RUSTGRID_SQLSERVER_PORT` / `USER` / `PASSWORD` / `DATABASE`) and running:
//!
//! ```text
//! cargo test -p rustgrid-sqlserver -- --ignored
//! ```
//!
//! The test runs in a scratch **user** database (default `rustgrid_live`, created on demand): the
//! driver's `list_databases`/`user_mappings` intentionally hide the system databases, so a
//! `master` default would make the listing assertions fail.

use rustgrid_core::{
    BackupObjectKind, Connection, ConnectionConfig, Driver, DriverId, ObjectKind, PageRequest,
    Result, RowInsert, RowUpdate, UserAccount, UserDetails, UserEdit, ViewEdit,
};
use rustgrid_sqlserver::SqlServerDriver;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

fn config(database: &str) -> ConnectionConfig {
    ConnectionConfig {
        driver: DriverId::new("sqlserver"),
        host: env("RUSTGRID_SQLSERVER_HOST", "localhost"),
        port: env("RUSTGRID_SQLSERVER_PORT", "1433")
            .parse()
            .expect("RUSTGRID_SQLSERVER_PORT must be a number"),
        username: env("RUSTGRID_SQLSERVER_USER", "sa"),
        password: Some(env("RUSTGRID_SQLSERVER_PASSWORD", "")),
        database: Some(database.to_string()),
        options: Default::default(),
        settings: Default::default(),
    }
}

/// Connect to `master`, create the scratch database when it is missing, and hand back a connection
/// opened on it together with its name.
async fn scratch_database() -> (Box<dyn Connection>, String) {
    let database = env("RUSTGRID_SQLSERVER_DATABASE", "rustgrid_live");
    let admin = SqlServerDriver::new()
        .connect(&config("master"))
        .await
        .expect("connect (master)");
    admin
        .execute_query(
            None,
            &format!("IF DB_ID(N'{database}') IS NULL CREATE DATABASE [{database}]"),
        )
        .await
        .expect("create the scratch database");
    let connection = SqlServerDriver::new()
        .connect(&config(&database))
        .await
        .expect("connect");
    (connection, database)
}

#[tokio::test]
#[ignore = "requires a live SQL Server (RUSTGRID_SQLSERVER_*)"]
async fn round_trips_a_live_sql_server() {
    let (connection, database) = scratch_database().await;

    let table = "rustgrid_live_users";
    let _ = connection
        .execute_query(
            Some(&database),
            &format!("DROP TABLE IF EXISTS [dbo].[{table}]"),
        )
        .await;
    connection
        .execute_query(
            Some(&database),
            &format!(
                "CREATE TABLE [dbo].[{table}] (\
                    [id] INT IDENTITY(1,1) PRIMARY KEY, \
                    [name] NVARCHAR(100) NOT NULL, \
                    [age] INT NULL, \
                    [created] DATETIME2 NULL)"
            ),
        )
        .await
        .expect("create table");
    connection
        .execute_query(
            Some(&database),
            &format!(
                "INSERT INTO [dbo].[{table}] ([name], [age]) VALUES (N'ann', 30), (N'bob', 25)"
            ),
        )
        .await
        .expect("insert rows");

    // Catalog.
    let databases = connection.list_databases().await.unwrap();
    assert!(databases.iter().any(|entry| entry.name == database));
    // SQL Server object names are schema-qualified in the listing (`dbo.users`).
    let tables = connection.list_tables(&database).await.unwrap();
    assert!(
        tables
            .iter()
            .any(|entry| entry.name == format!("dbo.{table}") && entry.kind == ObjectKind::Table)
    );
    let columns = connection.columns(&database, table).await.unwrap();
    assert_eq!(columns.len(), 4);
    assert!(columns[0].primary_key);
    assert!(!columns[0].nullable);

    let page = connection
        .fetch_page(&database, table, PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.total_rows, Some(2));

    // A schema-qualified name (as the tree shows it) works the same way.
    let page = connection
        .fetch_page(&database, &format!("dbo.{table}"), PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);

    // Editing.
    connection
        .update_rows(
            &database,
            table,
            &[RowUpdate {
                set: vec![("age".to_string(), Some("31".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .unwrap();
    connection
        .insert_rows(
            &database,
            table,
            &[RowInsert {
                values: vec![
                    ("name".to_string(), Some("cy".to_string())),
                    ("age".to_string(), Some("40".to_string())),
                ],
            }],
        )
        .await
        .unwrap();
    connection
        .delete_rows(
            &database,
            table,
            &[vec![("id".to_string(), Some("2".to_string()))]],
        )
        .await
        .unwrap();

    let page = connection
        .fetch_page(&database, table, PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(2));

    // Multi-statement script: one result per statement.
    let results = connection
        .execute_query_many(Some(&database), "SELECT 1 AS a; SELECT 2 AS b")
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.has_result_set));

    // Schema introspection.
    let schema = connection.table_schema(&database, table).await.unwrap();
    assert_eq!(schema.columns.len(), 4);
    assert!(schema.columns[0].auto_increment);

    // Backup metadata + row streaming + restore.
    let mut dump = connection
        .backup_object_metadata(&database, BackupObjectKind::Table, table)
        .await
        .unwrap();
    assert!(dump.ddl.to_ascii_uppercase().contains("CREATE TABLE"));
    assert_eq!(dump.fields.len(), 4);

    let mut collected: Vec<String> = Vec::new();
    let count = {
        let mut sink = |tuple: &str| -> Result<()> {
            collected.push(tuple.to_string());
            Ok(())
        };
        connection
            .stream_table_rows(&database, table, &mut sink)
            .await
            .unwrap()
    };
    assert_eq!(count, 2);
    dump.rows = collected;
    connection.restore_object(&database, &dump).await.unwrap();
    let page = connection
        .fetch_page(&database, table, PageRequest::new(0, 10))
        .await
        .unwrap();
    assert_eq!(page.total_rows, Some(2));

    // Views.
    let view = "rustgrid_live_adults";
    let _ = connection
        .execute_query(
            Some(&database),
            &format!("DROP VIEW IF EXISTS [dbo].[{view}]"),
        )
        .await;
    connection
        .execute_query(
            Some(&database),
            &format!(
                "CREATE VIEW [dbo].[{view}] AS SELECT [name] FROM [dbo].[{table}] WHERE [age] >= 18"
            ),
        )
        .await
        .unwrap();
    let tables = connection.list_tables(&database).await.unwrap();
    assert!(
        tables
            .iter()
            .any(|entry| entry.name == format!("dbo.{view}") && entry.kind == ObjectKind::View)
    );
    let details = connection.view_details(&database, view).await.unwrap();
    assert!(
        details
            .definition
            .to_ascii_uppercase()
            .contains("CREATE VIEW")
    );
    connection
        .save_view(
            &database,
            Some(view),
            &ViewEdit {
                name: view.to_string(),
                definition: format!(
                    "CREATE VIEW [dbo].[{view}] AS SELECT [name] FROM [dbo].[{table}] WHERE [age] >= 21"
                ),
            },
        )
        .await
        .unwrap();
    let details = connection.view_details(&database, view).await.unwrap();
    assert!(details.definition.contains("21"));
    connection.drop_view(&database, view).await.unwrap();

    let _ = connection
        .execute_query(
            Some(&database),
            &format!("DROP TABLE IF EXISTS [dbo].[{table}]"),
        )
        .await;
    connection.close().await.unwrap();
}

/// A `UserEdit` that carries the account's loaded state unchanged, so only what the caller sets
/// afterwards is written.
fn edit_from(details: &UserDetails) -> UserEdit {
    let mut edit = empty_edit(details.account.clone());
    edit.original = Some(details.clone());
    edit.server_privileges = details.server_privileges.clone();
    edit.denied_server_privileges = details.denied_server_privileges.clone();
    edit.grant_option_server_privileges = details.grant_option_server_privileges.clone();
    edit.grants = details.grants.clone();
    edit.default_privileges = details.default_privileges.clone();
    edit.roles = details
        .roles
        .iter()
        .map(|edge| {
            (
                edge.role_user.clone(),
                edge.role_host.clone(),
                edge.admin_option,
            )
        })
        .collect();
    edit.members = details
        .members
        .iter()
        .map(|edge| {
            (
                edge.member_user.clone(),
                edge.member_host.clone(),
                edge.admin_option,
            )
        })
        .collect();
    edit
}

/// A `UserEdit` with every field except the account left empty.
fn empty_edit(account: UserAccount) -> UserEdit {
    use std::collections::BTreeSet;
    UserEdit {
        original: None,
        account,
        password: None,
        server_privileges: BTreeSet::new(),
        denied_server_privileges: BTreeSet::new(),
        grant_option_server_privileges: BTreeSet::new(),
        grants: Vec::new(),
        default_privileges: Vec::new(),
        roles: Vec::new(),
        members: Vec::new(),
        mappings: Vec::new(),
        original_mappings: Vec::new(),
        old_password: None,
        securables: Vec::new(),
        original_securables: Vec::new(),
    }
}

/// Exercises the SQL Server account features added for the Navicat-parity work: the full login
/// attribute set, 用户映射 (database user + database role) and 登录权限 (a securable grant).
#[tokio::test]
#[ignore = "requires a live SQL Server (RUSTGRID_SQLSERVER_*)"]
async fn manages_a_login_with_mapping_and_securables() {
    use rustgrid_core::PrivilegeId;
    use std::collections::BTreeSet;

    let (connection, database) = scratch_database().await;
    let login = "rustgrid_live_login";

    // Clean any leftovers from a previous run.
    let _ = connection
        .execute_query(
            Some(&database),
            &format!(
                "IF EXISTS (SELECT 1 FROM sys.database_principals WHERE name = N'{login}') \
                 DROP USER [{login}]"
            ),
        )
        .await;
    let _ = connection
        .execute_query(
            None,
            &format!("IF SUSER_ID(N'{login}') IS NOT NULL DROP LOGIN [{login}]"),
        )
        .await;

    // Create the login with the new attributes, then read them back.
    let account = UserAccount {
        user: login.to_string(),
        login_type: "SQL Server".to_string(),
        default_database: database.clone(),
        check_policy: false,
        check_expiration: false,
        ..Default::default()
    };
    let mut edit = empty_edit(account);
    edit.password = Some("Str0ng!passw0rd".to_string());
    connection.save_user(&edit).await.expect("create login");

    let details = connection
        .user_details(login, "")
        .await
        .expect("load login");
    assert_eq!(details.account.login_type, "SQL Server");
    assert_eq!(details.account.default_database, database);
    assert!(!details.account.check_policy);
    assert!(details.account.password_set);

    // 用户映射: map the login into the database with db_datareader.
    let before = connection.user_mappings(login, "").await.expect("mappings");
    assert!(before.iter().any(|mapping| mapping.database == database));
    let mut after = before.clone();
    for mapping in &mut after {
        if mapping.database == database {
            mapping.mapped = true;
            mapping.user_name = login.to_string();
            mapping.default_schema = "dbo".to_string();
            mapping.roles = ["db_datareader".to_string()].into_iter().collect();
        }
    }
    let mut edit = edit_from(&details);
    edit.mappings = after;
    edit.original_mappings = before;
    connection.save_user(&edit).await.expect("map login");

    let mapped = connection
        .user_mappings(login, "")
        .await
        .expect("mappings after");
    let row = mapped
        .iter()
        .find(|mapping| mapping.database == database)
        .expect("the mapped database");
    assert!(row.mapped);
    assert_eq!(row.user_name, login);
    assert!(row.roles.contains("db_datareader"), "{:?}", row.roles);

    // 登录权限: grant IMPERSONATE on the sa login.
    let before = connection
        .user_securables(login, "")
        .await
        .expect("securables");
    assert!(before.iter().any(|entry| entry.class == "ENDPOINT"));
    assert!(
        before
            .iter()
            .any(|entry| entry.class == "LOGIN" && entry.name == "sa")
    );
    let mut after = before.clone();
    for entry in &mut after {
        if entry.class == "LOGIN" && entry.name == "sa" {
            entry.privileges.insert(PrivilegeId::new("IMPERSONATE"));
        }
    }
    let details = connection
        .user_details(login, "")
        .await
        .expect("reload login");
    let mut edit = edit_from(&details);
    edit.securables = after;
    edit.original_securables = before;
    connection
        .save_user(&edit)
        .await
        .expect("grant impersonate");

    let after = connection
        .user_securables(login, "")
        .await
        .expect("securables after");
    let sa = after
        .iter()
        .find(|entry| entry.class == "LOGIN" && entry.name == "sa")
        .expect("the sa login row");
    assert!(
        sa.privileges.contains(&PrivilegeId::new("IMPERSONATE")),
        "{:?}",
        sa.privileges
    );

    // 服务器权限: grant VIEW SERVER STATE (also requires the master context).
    let details = connection
        .user_details(login, "")
        .await
        .expect("reload for server privilege");
    let mut edit = edit_from(&details);
    edit.server_privileges
        .insert(PrivilegeId::new("VIEW SERVER STATE"));
    connection
        .save_user(&edit)
        .await
        .expect("grant server privilege");
    let details = connection
        .user_details(login, "")
        .await
        .expect("reload after server privilege");
    assert!(
        details
            .server_privileges
            .contains(&PrivilegeId::new("VIEW SERVER STATE")),
        "{:?}",
        details.server_privileges
    );

    // 含授予选项: the 'W' state round-trips.
    let mut edit = edit_from(&details);
    edit.grant_option_server_privileges
        .insert(PrivilegeId::new("VIEW SERVER STATE"));
    connection
        .save_user(&edit)
        .await
        .expect("grant with grant option");
    let details = connection
        .user_details(login, "")
        .await
        .expect("reload after grant option");
    assert!(
        details
            .grant_option_server_privileges
            .contains(&PrivilegeId::new("VIEW SERVER STATE")),
        "{:?}",
        details.grant_option_server_privileges
    );

    // Cleanup: unmap, then drop the login (its securable grants go with it).
    let details = connection
        .user_details(login, "")
        .await
        .expect("reload cleanup");
    let before = connection
        .user_mappings(login, "")
        .await
        .expect("mappings cleanup");
    let mut after = before.clone();
    for mapping in &mut after {
        if mapping.database == database {
            mapping.mapped = false;
            mapping.roles = BTreeSet::new();
        }
    }
    let mut edit = edit_from(&details);
    edit.mappings = after;
    edit.original_mappings = before;
    connection.save_user(&edit).await.expect("unmap login");
    connection.drop_user(login, "").await.expect("drop login");

    connection.close().await.unwrap();
}

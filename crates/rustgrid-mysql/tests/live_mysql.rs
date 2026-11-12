use std::collections::BTreeMap;

use rustgrid_core::{ConnectionConfig, Driver, DriverId, PageRequest};

fn config() -> ConnectionConfig {
    let host = std::env::var("RUSTGRID_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("RUSTGRID_MYSQL_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3306);
    let username = std::env::var("RUSTGRID_MYSQL_USER").unwrap_or_else(|_| "root".to_string());
    let password = std::env::var("RUSTGRID_MYSQL_PASSWORD").ok();
    let database = std::env::var("RUSTGRID_MYSQL_DATABASE").ok();

    ConnectionConfig {
        driver: DriverId::new("mysql"),
        host,
        port,
        username,
        password,
        database,
        options: BTreeMap::new(),
        settings: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_catalog_and_page() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let databases = connection.list_databases().await.expect("list databases");
    assert!(
        databases
            .iter()
            .any(|database| database.name == "information_schema"),
        "expected information_schema to be listed"
    );

    let tables = connection
        .list_tables("information_schema")
        .await
        .expect("list tables");
    assert!(
        tables
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("TABLES")),
        "expected information_schema.TABLES to be listed"
    );

    let columns = connection
        .columns("information_schema", "TABLES")
        .await
        .expect("load columns");
    assert!(!columns.is_empty());

    let page = connection
        .fetch_page("information_schema", "TABLES", PageRequest::new(0, 10))
        .await
        .expect("fetch page");
    assert_eq!(page.columns.len(), columns.len());
    assert!(page.rows.len() <= 10);
    assert!(page.total_rows.unwrap_or(0) > 0);
    assert!(
        page.rows
            .iter()
            .flatten()
            .any(|cell| !matches!(cell, rustgrid_core::CellValue::Null)),
        "expected at least one non-null cell"
    );

    connection.close().await.expect("close");
}

/// A `TIMESTAMP` column read through the binary protocol (`fetch_page`) must decode to the same
/// wall-clock text the text protocol returns, and the session must keep the server's own time zone
/// (sqlx otherwise forces `+00:00`, which shows `TIMESTAMP` values in UTC while other clients show
/// them in the server zone).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_timestamps_keep_the_session_wall_clock() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let database = "rustgrid_live";
    connection
        .execute_query(None, &format!("CREATE DATABASE IF NOT EXISTS `{database}`"))
        .await
        .expect("create the scratch database");
    let table = "rustgrid_timestamps";
    let _ = connection.drop_table(database, table).await;
    connection
        .execute_query(
            Some(database),
            &format!(
                "CREATE TABLE `{table}` (\
                 `id` int NOT NULL PRIMARY KEY, \
                 `created_at` timestamp NULL, \
                 `updated_at` datetime NULL)"
            ),
        )
        .await
        .expect("create the timestamp table");
    connection
        .execute_query(
            Some(database),
            &format!(
                "INSERT INTO `{table}` VALUES \
                 (1, '2026-04-22 13:51:51', '2026-04-22 13:51:51')"
            ),
        )
        .await
        .expect("insert a timestamp");

    // The driver must not override the server's session time zone: forcing UTC makes every
    // TIMESTAMP read differ from what the server and other clients report.
    let zone = connection
        .execute_query(None, "SELECT @@session.time_zone")
        .await
        .expect("read the session time zone");
    assert_eq!(
        zone.rows[0][0].as_display(),
        "SYSTEM",
        "the MySQL driver must keep the server's session time zone"
    );

    let page = connection
        .fetch_page(database, table, PageRequest::new(0, 10))
        .await
        .expect("fetch page");
    assert!(
        matches!(page.rows[0][1], rustgrid_core::CellValue::Text(_)),
        "TIMESTAMP must decode to text, got {:?}",
        page.rows[0][1]
    );
    assert_eq!(page.rows[0][1].as_display(), "2026-04-22 13:51:51");
    assert_eq!(page.rows[0][2].as_display(), "2026-04-22 13:51:51");

    let result = connection
        .execute_query(
            Some(database),
            &format!("SELECT `created_at`, `updated_at` FROM `{table}`"),
        )
        .await
        .expect("query the timestamps");
    assert_eq!(
        result.rows[0][0].as_display(),
        page.rows[0][1].as_display(),
        "the binary and text protocols must agree"
    );
    assert_eq!(result.rows[0][1].as_display(), page.rows[0][2].as_display());

    let _ = connection.drop_table(database, table).await;
    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_table_statuses_report_exact_row_counts() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let Some(database) = connection
        .list_databases()
        .await
        .expect("list databases")
        .into_iter()
        .find(|database| database.name == "test")
    else {
        return;
    };
    let statuses = connection
        .table_statuses(&database.name)
        .await
        .expect("load table statuses");
    assert!(!statuses.is_empty(), "expected tables in test");
    for (name, status) in statuses {
        let sql = format!("SELECT COUNT(*) FROM `{}`", name.replace('`', "``"));
        let result = connection
            .execute_query(Some(&database.name), &sql)
            .await
            .expect("count rows");
        let actual = result.rows[0][0]
            .as_display()
            .parse::<u64>()
            .expect("numeric count");
        assert_eq!(
            status.rows,
            Some(actual),
            "{name} should report its exact row count"
        );
    }

    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_schema_keeps_an_explicit_empty_string_default() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let Some(database) = connection
        .list_databases()
        .await
        .expect("list databases")
        .into_iter()
        .find(|database| database.name == "test")
    else {
        return;
    };
    let schema = connection
        .table_schema(&database.name, "gen_table")
        .await
        .expect("load table schema");
    let Some(column) = schema
        .columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case("table_name"))
    else {
        return;
    };
    assert_eq!(
        column.default, "''",
        "DEFAULT '' must not collapse into DEFAULT NULL"
    );

    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_schema_loads_without_generation_expression() {
    use rustgrid_core::{ColumnDef, TableSchema};

    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let database = "rustgrid_live";
    connection
        .execute_query(None, &format!("CREATE DATABASE IF NOT EXISTS `{database}`"))
        .await
        .expect("create the scratch database");

    let table = "rustgrid_plain";
    let _ = connection.drop_table(database, table).await;
    let schema = TableSchema {
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                primary_key: true,
                auto_increment: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "name".to_string(),
                data_type: "varchar".to_string(),
                length: "50".to_string(),
                nullable: true,
                ..ColumnDef::default()
            },
        ],
        ..TableSchema::default()
    };
    connection
        .save_table_schema(database, table, None, &schema)
        .await
        .expect("create the table");

    // MySQL 5.6, MariaDB 10.1 and some MySQL-compatible servers have no
    // `INFORMATION_SCHEMA.COLUMNS.GENERATION_EXPRESSION`; loading the schema must retry without it
    // instead of surfacing `1054 - Unknown column 'generation_expression' in 'field list'`.
    let loaded = connection
        .table_schema(database, table)
        .await
        .expect("load the table schema");
    assert_eq!(loaded.columns.len(), 2, "{:?}", loaded.columns);
    assert!(
        loaded
            .columns
            .iter()
            .any(|column| column.name == "id" && column.primary_key && column.auto_increment),
        "the identity column did not load: {:?}",
        loaded.columns
    );

    let _ = connection.drop_table(database, table).await;
    connection.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_multi_statement_returns_every_result_set() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    // Two result sets, a zero-row SELECT (whose columns must still come back) and a session
    // variable set by an earlier statement in the same script.
    let results = connection
        .execute_query_many(
            None,
            "SET @rustgrid_probe = 41; \
             SELECT 1 AS a; \
             SELECT 2 AS b, 3 AS c; \
             SELECT @rustgrid_probe + 1 AS d WHERE 1 = 0; \
             SELECT @rustgrid_probe + 1 AS e",
        )
        .await
        .expect("run a multi-statement script");

    assert_eq!(results.len(), 5, "one result per statement");
    assert!(!results[0].has_result_set);
    assert!(results[0].statement.starts_with("SET @rustgrid_probe"));

    assert!(results[1].has_result_set);
    assert_eq!(results[1].columns.len(), 1);
    assert_eq!(results[1].columns[0].name, "a");
    assert_eq!(results[1].rows.len(), 1);
    assert_eq!(results[1].statement, "SELECT 1 AS a");

    assert!(results[2].has_result_set);
    assert_eq!(results[2].columns.len(), 2);

    // A zero-row SELECT still exposes its columns (via `describe_columns`).
    assert!(results[3].has_result_set);
    assert_eq!(results[3].columns[0].name, "d");
    assert!(results[3].rows.is_empty());

    // Session state set by the first statement is visible to the last.
    assert!(results[4].has_result_set);
    assert!(matches!(
        results[4].rows.first().and_then(|row| row.first()),
        Some(rustgrid_core::CellValue::Int(42))
    ));

    connection.close().await.expect("close");
}

/// The query editor's text path through a MySQL-protocol proxy. UDAL leaves a plain OK packet after
/// the column metadata when `CLIENT_DEPRECATE_EOF` is requested (issue #56); the patched client
/// must decode the real rows instead of that packet.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_text_query_rows_survive_the_proxy_metadata() {
    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let database = "rustgrid_live";
    connection
        .execute_query(None, &format!("CREATE DATABASE IF NOT EXISTS `{database}`"))
        .await
        .expect("create the scratch database");
    // Trigger the proxy's 1046 path first, exactly like opening a database in the tree.
    connection
        .list_tables(database)
        .await
        .expect("list tables through the proxy");
    let table = "rustgrid_text_proxy";
    let _ = connection.drop_table(database, table).await;
    connection
        .execute_query(
            Some(database),
            &format!(
                "CREATE TABLE `{table}` (\
                 `id` int NOT NULL PRIMARY KEY, \
                 `para_value` varchar(16000) NULL)"
            ),
        )
        .await
        .expect("create the scratch table");
    connection
        .execute_query(
            Some(database),
            &format!("INSERT INTO `{table}` VALUES (1, 'hello'), (2, NULL)"),
        )
        .await
        .expect("insert rows");

    let result = connection
        .execute_query(Some(database), &format!("SELECT * FROM `{table}` LIMIT 10"))
        .await
        .expect("run the text query");
    assert_eq!(result.columns.len(), 2);
    assert_eq!(result.rows.len(), 2);
    assert_eq!(result.rows[0][1].as_display(), "hello");

    let _ = connection.drop_table(database, table).await;
    connection.close().await.expect("close");
}

/// The table designer's write path: create a table from a `TableSchema`, read it back, alter it and
/// read it again — including MySQL's per-column index options (子部分 / `DESC`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_designer_round_trips_a_table() {
    use rustgrid_core::{ColumnDef, IndexColumnDef, IndexDef, TableSchema};

    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let database = "rustgrid_live";
    connection
        .execute_query(None, &format!("CREATE DATABASE IF NOT EXISTS `{database}`"))
        .await
        .expect("create the scratch database");

    let table = "rustgrid_designer";
    let _ = connection.drop_table(database, table).await;

    let mut schema = TableSchema {
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                primary_key: true,
                auto_increment: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "code".to_string(),
                data_type: "varchar".to_string(),
                length: "64".to_string(),
                nullable: true,
                ..ColumnDef::default()
            },
            ColumnDef {
                name: "amount".to_string(),
                data_type: "decimal".to_string(),
                length: "10".to_string(),
                decimals: "2".to_string(),
                nullable: true,
                comment: "the amount".to_string(),
                ..ColumnDef::default()
            },
        ],
        ..TableSchema::default()
    };
    schema.indexes.push(IndexDef {
        name: "ix_code".to_string(),
        columns: vec![
            IndexColumnDef {
                name: "code".to_string(),
                descending: true,
                prefix_length: "10".to_string(),
            },
            IndexColumnDef::new("amount"),
        ],
        unique: false,
        primary: false,
        index_type: "BTREE".to_string(),
        comment: String::new(),
    });
    connection
        .save_table_schema(database, table, None, &schema)
        .await
        .expect("create the table");

    let loaded = connection
        .table_schema(database, table)
        .await
        .expect("read the table back");
    assert!(
        loaded
            .columns
            .iter()
            .any(|column| column.name == "id" && column.primary_key && column.auto_increment),
        "the identity column did not round trip: {:?}",
        loaded.columns
    );
    let index = loaded
        .indexes
        .iter()
        .find(|index| index.name == "ix_code")
        .unwrap_or_else(|| panic!("ix_code is missing: {:?}", loaded.indexes));
    assert_eq!(index.columns.len(), 2, "{:?}", index.columns);
    assert_eq!(index.columns[0].name, "code");
    assert!(
        index.columns[0].descending,
        "the descending sort did not round trip: {:?}",
        index.columns
    );
    assert_eq!(index.columns[0].prefix_length, "10");
    assert_eq!(index.columns[1].name, "amount");

    // Alter: a new column, and the index back to an ascending whole-column list.
    let mut modified = loaded.clone();
    modified.columns.push(ColumnDef {
        name: "note".to_string(),
        data_type: "varchar".to_string(),
        length: "50".to_string(),
        nullable: true,
        ..ColumnDef::default()
    });
    let index = modified
        .indexes
        .iter_mut()
        .find(|index| index.name == "ix_code")
        .expect("ix_code");
    index.columns = vec![IndexColumnDef::new("code"), IndexColumnDef::new("amount")];
    connection
        .save_table_schema(database, table, Some(&loaded), &modified)
        .await
        .expect("alter the table");

    let reloaded = connection
        .table_schema(database, table)
        .await
        .expect("read the altered table back");
    assert!(reloaded.columns.iter().any(|column| column.name == "note"));
    let index = reloaded
        .indexes
        .iter()
        .find(|index| index.name == "ix_code")
        .expect("ix_code after the alter");
    assert!(
        !index.columns[0].descending,
        "the ascending sort did not round trip: {:?}",
        index.columns
    );
    assert!(
        index.columns[0].prefix_length.is_empty(),
        "the prefix should be gone: {:?}",
        index.columns
    );

    connection
        .drop_table(database, table)
        .await
        .expect("drop the scratch table");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server"]
async fn live_auth_failure_maps_to_authentication() {
    let mut config = config();
    config.password = Some("definitely-wrong-password".to_string());

    let driver = rustgrid_mysql::MysqlDriver::new();
    let error = match driver.connect(&config).await {
        Ok(_) => panic!("connect should fail with a wrong password"),
        Err(error) => error,
    };

    assert!(
        matches!(error, rustgrid_core::Error::Authentication(_)),
        "expected an authentication error, got: {error}"
    );
}

/// A `UserEdit` with every field except the account left empty.
fn empty_edit(account: rustgrid_core::UserAccount) -> rustgrid_core::UserEdit {
    use std::collections::BTreeSet;
    rustgrid_core::UserEdit {
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

/// A `UserEdit` carrying the loaded state, so only what the caller changes is written.
fn edit_from(details: &rustgrid_core::UserDetails) -> rustgrid_core::UserEdit {
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

/// Exercises the MySQL account editor the way the New/Edit User window drives it: create, load,
/// edit attributes, grant a server privilege, grant on a database, rename (keeping grants) and
/// drop.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (set RUSTGRID_MYSQL_PASSWORD)"]
async fn live_account_editor_round_trip() {
    use rustgrid_core::{ObjectGrant, PrivilegeId, UserAccount};

    let driver = rustgrid_mysql::MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let user = "rustgrid_live_user";
    let host = "%";
    let renamed = "rustgrid_live_user2";

    // Clean any leftovers from a previous run.
    let _ = connection.drop_user(user, host).await;
    let _ = connection.drop_user(renamed, host).await;

    // Create with a resource limit.
    let mut edit = empty_edit(UserAccount {
        user: user.to_string(),
        host: host.to_string(),
        plugin: "caching_sha2_password".to_string(),
        max_connections: 5,
        ..Default::default()
    });
    edit.password = Some("Str0ng!pass".to_string());
    connection.save_user(&edit).await.expect("create user");

    let details = connection
        .user_details(user, host)
        .await
        .expect("load user");
    assert_eq!(details.account.user, user);
    assert_eq!(details.account.host, host);
    assert_eq!(details.account.plugin, "caching_sha2_password");
    assert!(details.account.password_set);
    assert_eq!(details.account.max_connections, 5);

    // Edit: grant two server privileges, change a limit, lock the account and set a password
    // lifetime (exercises the `ACCOUNT` / `PASSWORD EXPIRE` clause ordering).
    let mut edit = edit_from(&details);
    edit.server_privileges.insert(PrivilegeId::new("SELECT"));
    edit.server_privileges.insert(PrivilegeId::new("SHOW VIEW"));
    edit.account.max_connections = 7;
    edit.account.account_locked = true;
    edit.account.password_lifetime = Some(30);
    connection.save_user(&edit).await.expect("edit user");

    let details = connection
        .user_details(user, host)
        .await
        .expect("load edited user");
    assert!(
        details
            .server_privileges
            .contains(&PrivilegeId::new("SELECT")),
        "{:?}",
        details.server_privileges
    );
    assert!(
        details
            .server_privileges
            .contains(&PrivilegeId::new("SHOW VIEW"))
    );
    assert_eq!(details.account.max_connections, 7);
    assert!(details.account.account_locked);
    assert_eq!(details.account.password_lifetime, Some(30));

    // A database-wide grant on a non-system database.
    let probe_database = connection
        .list_databases()
        .await
        .expect("list databases")
        .into_iter()
        .map(|database| database.name)
        .find(|name| {
            !matches!(
                name.as_str(),
                "information_schema" | "performance_schema" | "mysql" | "sys"
            )
        });
    if let Some(probe_database) = probe_database {
        let mut edit = edit_from(&details);
        edit.grants = vec![ObjectGrant {
            database: probe_database.clone(),
            schema: String::new(),
            name: String::new(),
            privileges: [PrivilegeId::new("SELECT"), PrivilegeId::new("INSERT")]
                .into_iter()
                .collect(),
        }];
        connection
            .save_user(&edit)
            .await
            .expect("grant on database");

        let details = connection
            .user_details(user, host)
            .await
            .expect("load after database grant");
        assert!(
            details.grants.iter().any(|grant| {
                grant.database == probe_database
                    && grant.name.is_empty()
                    && grant.privileges.contains(&PrivilegeId::new("SELECT"))
                    && grant.privileges.contains(&PrivilegeId::new("INSERT"))
            }),
            "{:?}",
            details.grants
        );
    }

    // Role membership: a granted role shows up in the 角色 tab.
    let role = "rustgrid_live_role";
    let _ = connection
        .execute_query(None, "DROP ROLE IF EXISTS 'rustgrid_live_role'@'%'")
        .await;
    connection
        .execute_query(None, "CREATE ROLE 'rustgrid_live_role'@'%'")
        .await
        .expect("create role");
    let details = connection
        .user_details(user, host)
        .await
        .expect("reload for role");
    let mut edit = edit_from(&details);
    edit.roles = vec![(role.to_string(), "%".to_string(), false)];
    connection.save_user(&edit).await.expect("grant role");
    let details = connection
        .user_details(user, host)
        .await
        .expect("reload after role");
    assert!(
        details
            .roles
            .iter()
            .any(|edge| edge.role_user == role && edge.member_user == user),
        "{:?}",
        details.roles
    );

    // 高级: the SSL/`REQUIRE` setting round-trips.
    let details = connection
        .user_details(user, host)
        .await
        .expect("reload for ssl");
    let mut edit = edit_from(&details);
    edit.account.ssl_type = "X509".to_string();
    connection.save_user(&edit).await.expect("set REQUIRE X509");
    let details = connection
        .user_details(user, host)
        .await
        .expect("reload after REQUIRE X509");
    assert_eq!(details.account.ssl_type, "X509");

    let mut edit = edit_from(&details);
    edit.account.ssl_type = "SPECIFIED".to_string();
    edit.account.ssl_cipher = "AES128-SHA".to_string();
    connection
        .save_user(&edit)
        .await
        .expect("set REQUIRE CIPHER");
    let details = connection
        .user_details(user, host)
        .await
        .expect("reload after REQUIRE CIPHER");
    assert_eq!(details.account.ssl_type, "SPECIFIED");
    assert_eq!(details.account.ssl_cipher, "AES128-SHA");

    // Rename keeps the grants.
    let details = connection.user_details(user, host).await.expect("reload");
    let mut edit = edit_from(&details);
    edit.account.user = renamed.to_string();
    connection.save_user(&edit).await.expect("rename user");
    let details = connection
        .user_details(renamed, host)
        .await
        .expect("load renamed user");
    assert!(
        details
            .server_privileges
            .contains(&PrivilegeId::new("SELECT")),
        "{:?}",
        details.server_privileges
    );

    connection
        .drop_user(renamed, host)
        .await
        .expect("drop user");
    let _ = connection
        .execute_query(None, "DROP ROLE IF EXISTS 'rustgrid_live_role'@'%'")
        .await;
    connection.close().await.expect("close");
}

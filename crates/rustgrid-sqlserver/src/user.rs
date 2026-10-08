//! SQL Server account (login) management. Mapped onto the engine-agnostic user models;
//! SQL Server has no `user@host` split, so the host part is always empty and accounts are
//! identified by login name alone.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    CellValue, Connection, Error, ObjectPrivilegeRow, PrivilegeCatalog, PrivilegeGroup,
    PrivilegeId, PrivilegeInfo, PrivilegePreset, Result, RoleMembership, SecurableClass,
    ServerSecurableGrant, UserAccount, UserDetails, UserEdit, UserEditSection, UserMapping,
};

use crate::SqlServerConnection;
use crate::connection::privilege_set;
use crate::helpers::{quote_identifier, quote_literal};

/// The SQL Server server-level permissions surfaced by the account editor, as
/// `(permission name, i18n key)`. `sys.server_permissions` can hold many more; this is the set the
/// editor shows. A permission the account holds but this list omits is still preserved on save.
const SERVER_PERMISSIONS: &[(&str, &str)] = &[
    ("CONNECT SQL", "user.priv.sqlserver.connect_sql"),
    ("VIEW ANY DATABASE", "user.priv.sqlserver.view_any_database"),
    (
        "VIEW ANY DEFINITION",
        "user.priv.sqlserver.view_any_definition",
    ),
    ("VIEW SERVER STATE", "user.priv.sqlserver.view_server_state"),
    (
        "ALTER ANY DATABASE",
        "user.priv.sqlserver.alter_any_database",
    ),
    ("ALTER ANY LOGIN", "user.priv.sqlserver.alter_any_login"),
    (
        "ALTER ANY SERVER ROLE",
        "user.priv.sqlserver.alter_any_server_role",
    ),
    ("ALTER SETTINGS", "user.priv.sqlserver.alter_settings"),
    (
        "CREATE ANY DATABASE",
        "user.priv.sqlserver.create_any_database",
    ),
    (
        "CREATE SERVER ROLE",
        "user.priv.sqlserver.create_server_role",
    ),
    (
        "ADMINISTER BULK OPERATIONS",
        "user.priv.sqlserver.administer_bulk_operations",
    ),
    ("CONTROL SERVER", "user.priv.sqlserver.control_server"),
    ("SHUTDOWN", "user.priv.sqlserver.shutdown"),
];

/// The SQL Server database-scope permissions offered by the 对象权限管理器, as
/// `(permission name, i18n key)`.
const DATABASE_PERMISSIONS: &[(&str, &str)] = &[
    ("CONNECT", "user.priv.sqlserver.connect"),
    ("CREATE TABLE", "user.priv.sqlserver.create_table"),
    ("CREATE VIEW", "user.priv.sqlserver.create_view"),
    ("CREATE PROCEDURE", "user.priv.sqlserver.create_procedure"),
    ("CREATE FUNCTION", "user.priv.sqlserver.create_function"),
];

/// Permissions valid at both database and object scope, as `(permission name, i18n key)`.
const SHARED_PERMISSIONS: &[(&str, &str)] = &[
    ("EXECUTE", "user.priv.execute"),
    ("ALTER", "user.priv.alter"),
    ("CONTROL", "user.priv.sqlserver.control"),
    ("VIEW DEFINITION", "user.priv.sqlserver.view_definition"),
];

/// The SQL Server object-scope permissions offered by the 对象权限管理器, as
/// `(permission name, i18n key)`.
const OBJECT_PERMISSIONS: &[(&str, &str)] = &[
    ("SELECT", "user.priv.select"),
    ("INSERT", "user.priv.insert"),
    ("UPDATE", "user.priv.update"),
    ("DELETE", "user.priv.delete"),
    ("REFERENCES", "user.priv.references"),
    ("TAKE OWNERSHIP", "user.priv.sqlserver.take_ownership"),
];

/// The SQL Server privilege catalog: server permissions plus the database/object permissions the
/// 对象权限管理器 grants.
pub(crate) fn privilege_catalog() -> PrivilegeCatalog {
    let mut privileges: Vec<PrivilegeInfo> = SERVER_PERMISSIONS
        .iter()
        .map(|(name, key)| PrivilegeInfo::server(*name, "server", Some(*key), name))
        .collect();
    privileges.extend(
        DATABASE_PERMISSIONS
            .iter()
            .map(|(name, key)| PrivilegeInfo::database(*name, "database", Some(*key), name)),
    );
    privileges.extend(SHARED_PERMISSIONS.iter().map(|(name, key)| {
        PrivilegeInfo::database_and_object(*name, "database", "object", Some(*key), name)
    }));
    privileges.extend(
        OBJECT_PERMISSIONS
            .iter()
            .map(|(name, key)| PrivilegeInfo::object(*name, "object", Some(*key), name)),
    );

    PrivilegeCatalog {
        groups: vec![
            PrivilegeGroup {
                id: "server".to_string(),
                label_key: Some("user.tab.server_privileges".to_string()),
                label: "Server".to_string(),
            },
            PrivilegeGroup {
                id: "database".to_string(),
                label_key: Some("user.create.priv_group.database".to_string()),
                label: "Database".to_string(),
            },
            PrivilegeGroup {
                id: "object".to_string(),
                label_key: Some("user.create.priv_group.object".to_string()),
                label: "Object".to_string(),
            },
        ],
        privileges,
        server_presets: vec![
            PrivilegePreset {
                id: "none".to_string(),
                label_key: Some("user.create.template.none".to_string()),
                label: "None".to_string(),
                privileges: Vec::new(),
            },
            PrivilegePreset {
                id: "connect".to_string(),
                label_key: Some("user.create.template.read_only".to_string()),
                label: "Connect".to_string(),
                privileges: vec![PrivilegeId::new("CONNECT SQL")],
            },
            PrivilegePreset {
                id: "admin".to_string(),
                label_key: Some("user.create.template.admin".to_string()),
                label: "Admin".to_string(),
                privileges: SERVER_PERMISSIONS
                    .iter()
                    .map(|(name, _)| PrivilegeId::new(*name))
                    .collect(),
            },
        ],
        object_presets: Vec::new(),
        default_privileges: Vec::new(),
        deny_supported: true,
        securable_classes: vec![
            SecurableClass {
                id: "endpoint".to_string(),
                class: "ENDPOINT".to_string(),
                label_key: Some("user.tab.endpoint_permissions".to_string()),
                label: "Endpoint Permissions".to_string(),
                privileges: [
                    "CONNECT",
                    "ALTER",
                    "CONTROL",
                    "TAKE OWNERSHIP",
                    "VIEW DEFINITION",
                ]
                .iter()
                .map(|name| PrivilegeId::new(*name))
                .collect(),
            },
            SecurableClass {
                id: "login".to_string(),
                class: "LOGIN".to_string(),
                label_key: Some("user.tab.login_permissions".to_string()),
                label: "Login Permissions".to_string(),
                privileges: [
                    "ALTER",
                    "CONTROL",
                    "IMPERSONATE",
                    "VIEW DEFINITION",
                    "TAKE OWNERSHIP",
                ]
                .iter()
                .map(|name| PrivilegeId::new(*name))
                .collect(),
            },
        ],
    }
}

fn account_from_row(row: &[rustgrid_core::CellValue]) -> UserAccount {
    let user = row
        .first()
        .map(rustgrid_core::CellValue::as_display)
        .unwrap_or_default();
    let disabled = row.get(1).map(is_truthy).unwrap_or(false);
    let is_admin = row.get(2).map(is_truthy).unwrap_or(false);
    UserAccount {
        user,
        host: String::new(),
        plugin: "SQL Server".to_string(),
        password_set: true,
        password_expired: false,
        password_lifetime: None,
        password_valid_until: None,
        account_locked: disabled,
        max_questions: 0,
        max_updates: 0,
        max_connections: 0,
        max_user_connections: 0,
        ssl_type: String::new(),
        ssl_cipher: String::new(),
        x509_issuer: String::new(),
        x509_subject: String::new(),
        default_tablespace: String::new(),
        profile: String::new(),
        tablespace_quota: String::new(),
        is_super_user: is_admin,
        ..UserAccount::default()
    }
}

fn is_truthy(value: &rustgrid_core::CellValue) -> bool {
    match value {
        rustgrid_core::CellValue::Bool(value) => *value,
        rustgrid_core::CellValue::Int(value) => *value != 0,
        rustgrid_core::CellValue::Uint(value) => *value != 0,
        rustgrid_core::CellValue::Text(value) => value == "1" || value.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A text cell, mapping `NULL`/missing to the empty string.
fn text_at(row: &[CellValue], index: usize) -> String {
    match row.get(index) {
        Some(CellValue::Null) | None => String::new(),
        Some(value) => value.as_display(),
    }
}

/// The SQL Server verification type shown by the editor, from `sys.server_principals.type_desc`.
fn verification_type(type_desc: &str) -> &'static str {
    match type_desc {
        "WINDOWS_LOGIN" | "WINDOWS_GROUP" => "Windows",
        "CERTIFICATE_MAPPED_LOGIN" => "Certificate",
        "ASYMMETRIC_KEY_MAPPED_LOGIN" => "Asymmetric Key",
        _ => "SQL Server",
    }
}

/// Load one login's editable attributes (identity, default database/language, password policy,
/// verification type and credential).
async fn load_account_detail(
    connection: &SqlServerConnection,
    user: &str,
) -> Result<Option<UserAccount>> {
    let sql = "SELECT p.name, p.is_disabled, p.default_database_name, p.default_language_name, \
                      COALESCE(s.is_policy_checked, 0), COALESCE(s.is_expiration_checked, 0), \
                      p.type_desc, \
                      CASE WHEN IS_SRVROLEMEMBER('sysadmin', p.name) = 1 THEN 1 ELSE 0 END, \
                      COALESCE(c.name, N'') \
               FROM sys.server_principals p \
               LEFT JOIN sys.sql_logins s ON s.principal_id = p.principal_id \
               LEFT JOIN sys.credentials c ON c.credential_id = p.credential_id \
               WHERE p.name = @P1 AND p.type IN ('S', 'U', 'G')";
    let result = connection.run(None, sql, &[Some(user.to_string())]).await?;
    Ok(result.rows.first().map(|row| {
        let type_desc = text_at(row, 6);
        let login_type = verification_type(&type_desc);
        let password_set = !matches!(type_desc.as_str(), "WINDOWS_LOGIN" | "WINDOWS_GROUP");
        UserAccount {
            user: text_at(row, 0),
            host: String::new(),
            plugin: "SQL Server".to_string(),
            password_set,
            account_locked: row.get(1).map(is_truthy).unwrap_or(false),
            default_database: text_at(row, 2),
            default_language: text_at(row, 3),
            check_policy: row.get(4).map(is_truthy).unwrap_or(false),
            check_expiration: row.get(5).map(is_truthy).unwrap_or(false),
            login_type: login_type.to_string(),
            is_super_user: row.get(7).map(is_truthy).unwrap_or(false),
            credential: text_at(row, 8),
            ..UserAccount::default()
        }
    }))
}

/// The per-principal projection shared by `list_users` and `user_details`.
async fn load_accounts(
    connection: &SqlServerConnection,
    filter: Option<&str>,
) -> Result<Vec<UserAccount>> {
    let predicate = if filter.is_some() {
        " AND p.name = @P1"
    } else {
        ""
    };
    let sql = format!(
        "SELECT p.name, p.is_disabled, \
                CASE WHEN IS_SRVROLEMEMBER('sysadmin', p.name) = 1 THEN 1 ELSE 0 END \
         FROM sys.server_principals p \
         WHERE p.type IN ('S', 'U', 'G') AND p.name NOT LIKE '##%'{predicate} \
         ORDER BY p.name"
    );
    let params: Vec<Option<String>> = filter
        .map(|value| vec![Some(value.to_string())])
        .unwrap_or_default();
    let result = connection.run(None, &sql, &params).await?;
    Ok(result
        .rows
        .iter()
        .map(|row| account_from_row(row))
        .collect())
}

pub(crate) async fn list_users(connection: &SqlServerConnection) -> Result<Vec<UserAccount>> {
    load_accounts(connection, None).await
}

/// The login's mapping into every user database (SQL Server's 用户映射): whether it has a database
/// user there, that user's name and default schema, the database roles it belongs to, and every
/// role the database offers. Best effort per database: one whose catalog cannot be read (no access,
/// offline) is reported unmapped with no roles rather than failing the whole load.
pub(crate) async fn user_mappings(
    connection: &SqlServerConnection,
    user: &str,
) -> Result<Vec<UserMapping>> {
    let databases = connection.list_databases().await?;
    let mut mappings = Vec::with_capacity(databases.len());
    for database in databases {
        let name = database.name;
        let param = [Some(user.to_string())];

        // A database user linked to the login is found by SID; the name test covers a user whose
        // SID differs (e.g. a contained/Windows login with the same name).
        let mapping_sql = "SELECT u.name, u.default_schema_name \
             FROM sys.database_principals u \
             WHERE u.type IN ('S', 'U', 'G') AND u.principal_id > 4 \
               AND (u.sid = SUSER_SID(CAST(@P1 AS sysname)) OR u.name = @P1)";
        let (mapped, user_name, default_schema) = match connection
            .run(Some(name.as_str()), mapping_sql, &param)
            .await
        {
            Ok(result) => match result.rows.first() {
                Some(row) => (
                    true,
                    row.first().map(CellValue::as_display).unwrap_or_default(),
                    row.get(1).map(CellValue::as_display).unwrap_or_default(),
                ),
                None => (false, String::new(), String::new()),
            },
            Err(_) => (false, String::new(), String::new()),
        };

        let roles_sql = "SELECT r.name, \
                 CASE WHEN m.member_principal_id IS NULL THEN 0 ELSE 1 END \
             FROM sys.database_principals r \
             LEFT JOIN sys.database_role_members m \
               ON m.role_principal_id = r.principal_id \
              AND m.member_principal_id = ( \
                  SELECT TOP 1 u.principal_id FROM sys.database_principals u \
                  WHERE u.type IN ('S', 'U', 'G') AND u.principal_id > 4 \
                    AND (u.sid = SUSER_SID(CAST(@P1 AS sysname)) OR u.name = @P1) ) \
             WHERE r.type = 'R' AND r.name <> 'public' \
             ORDER BY r.name";
        let mut available_roles = Vec::new();
        let mut roles = BTreeSet::new();
        if let Ok(result) = connection.run(Some(name.as_str()), roles_sql, &param).await {
            for row in &result.rows {
                let role = row.first().map(CellValue::as_display).unwrap_or_default();
                if role.is_empty() {
                    continue;
                }
                if row.get(1).map(is_truthy).unwrap_or(false) {
                    roles.insert(role.clone());
                }
                available_roles.push(role);
            }
        }

        mappings.push(UserMapping {
            database: name,
            mapped,
            user_name,
            default_schema,
            roles,
            available_roles,
        });
    }
    Ok(mappings)
}

/// Every server-level securable (endpoints and logins) with the account's permissions on it, for
/// the editor's 终端节点权限 / 登录权限 sections.
pub(crate) async fn user_securables(
    connection: &SqlServerConnection,
    user: &str,
) -> Result<Vec<ServerSecurableGrant>> {
    let mut rows: Vec<ServerSecurableGrant> = Vec::new();
    let endpoints = connection
        .run(None, "SELECT name FROM sys.endpoints ORDER BY name", &[])
        .await?;
    for row in &endpoints.rows {
        rows.push(ServerSecurableGrant {
            class: "ENDPOINT".to_string(),
            name: text_at(row, 0),
            ..Default::default()
        });
    }
    let logins = connection
        .run(
            None,
            "SELECT name FROM sys.server_principals \
             WHERE type IN ('S', 'U', 'G') ORDER BY name",
            &[],
        )
        .await?;
    for row in &logins.rows {
        rows.push(ServerSecurableGrant {
            class: "LOGIN".to_string(),
            name: text_at(row, 0),
            ..Default::default()
        });
    }

    let sql = "SELECT sp.class, COALESCE(e.name, p.name), sp.permission_name, sp.state \
               FROM sys.server_permissions sp \
               LEFT JOIN sys.endpoints e ON sp.class = 105 AND e.endpoint_id = sp.major_id \
               LEFT JOIN sys.server_principals p ON sp.class = 101 AND p.principal_id = sp.major_id \
               WHERE sp.class IN (101, 105) AND sp.grantee_principal_id = SUSER_ID(@P1) \
                 AND sp.state IN ('G', 'W', 'D')";
    let grants = connection.run(None, sql, &[Some(user.to_string())]).await?;
    for row in &grants.rows {
        let class = match text_at(row, 0).as_str() {
            "105" => "ENDPOINT",
            "101" => "LOGIN",
            _ => continue,
        };
        let name = text_at(row, 1);
        let permission = PrivilegeId::new(text_at(row, 2).trim().to_ascii_uppercase());
        if name.is_empty() || permission.as_str().is_empty() {
            continue;
        }
        let denied = text_at(row, 3).eq_ignore_ascii_case("D");
        if let Some(entry) = rows
            .iter_mut()
            .find(|entry| entry.class == class && entry.name == name)
        {
            if denied {
                entry.denied.insert(permission);
            } else {
                entry.privileges.insert(permission);
            }
        }
    }
    Ok(rows)
}

pub(crate) async fn user_details(
    connection: &SqlServerConnection,
    user: &str,
    _host: &str,
) -> Result<UserDetails> {
    let account = load_account_detail(connection, user)
        .await?
        .ok_or_else(|| Error::Query(format!("login {user} not found")))?;

    let permission_sql = "SELECT sp.permission_name, sp.state FROM sys.server_permissions sp \
         JOIN sys.server_principals p ON p.principal_id = sp.grantee_principal_id \
         WHERE p.name = @P1 AND sp.class = 100 AND sp.state IN ('G', 'W', 'D')";
    let permissions = connection
        .run(None, permission_sql, &[Some(user.to_string())])
        .await?;
    let is_deny = |row: &&Vec<CellValue>| {
        row.get(1)
            .map(CellValue::as_display)
            .is_some_and(|state| state.eq_ignore_ascii_case("D"))
    };
    let server_privileges: BTreeSet<PrivilegeId> = if account.is_super_user {
        // A sysadmin can do anything; present the catalog's full set rather than the empty
        // explicit-grant list.
        privilege_catalog()
            .server()
            .map(|info| info.id.clone())
            .collect()
    } else {
        privilege_set(
            permissions
                .rows
                .iter()
                .filter(|row| !is_deny(row))
                .filter_map(|row| row.first().map(CellValue::as_display)),
        )
    };
    let denied_server_privileges: BTreeSet<PrivilegeId> = permissions
        .rows
        .iter()
        .filter(|row| is_deny(row))
        .filter_map(|row| row.first().map(CellValue::as_display))
        .map(|name| PrivilegeId::new(name.trim().to_ascii_uppercase()))
        .filter(|id| !id.as_str().is_empty())
        .collect();

    let role_sql = "SELECT r.name FROM sys.server_role_members m \
         JOIN sys.server_principals r ON r.principal_id = m.role_principal_id \
         JOIN sys.server_principals p ON p.principal_id = m.member_principal_id \
         WHERE p.name = @P1 ORDER BY r.name";
    let role_result = connection
        .run(None, role_sql, &[Some(user.to_string())])
        .await?;
    let roles = role_result
        .rows
        .iter()
        .map(|row| RoleMembership {
            role_user: row
                .first()
                .map(rustgrid_core::CellValue::as_display)
                .unwrap_or_default(),
            role_host: "SERVER".to_string(),
            member_user: user.to_string(),
            member_host: String::new(),
            admin_option: false,
        })
        .collect();

    let member_sql = "SELECT p.name FROM sys.server_role_members m \
         JOIN sys.server_principals r ON r.principal_id = m.role_principal_id \
         JOIN sys.server_principals p ON p.principal_id = m.member_principal_id \
         WHERE r.name = @P1 ORDER BY p.name";
    let member_result = connection
        .run(None, member_sql, &[Some(user.to_string())])
        .await?;
    let members = member_result
        .rows
        .iter()
        .map(|row| RoleMembership {
            role_user: user.to_string(),
            role_host: String::new(),
            member_user: row
                .first()
                .map(rustgrid_core::CellValue::as_display)
                .unwrap_or_default(),
            member_host: "SERVER".to_string(),
            admin_option: false,
        })
        .collect();

    Ok(UserDetails {
        account,
        server_privileges,
        denied_server_privileges,
        grants: Vec::new(),
        default_privileges: Vec::new(),
        roles,
        members,
    })
}

/// The `WITH ...` option list for a new login (password, policy, defaults and credential).
fn create_login_options(account: &UserAccount, password: Option<&str>) -> Vec<String> {
    let mut options = Vec::new();
    if let Some(password) = password {
        // `MUST_CHANGE` attaches to `PASSWORD` without a comma.
        let mut password_option = format!("PASSWORD = {}", quote_literal(password));
        if account.must_change {
            password_option.push_str(" MUST_CHANGE");
        }
        options.push(password_option);
    }
    options.push(format!(
        "CHECK_POLICY = {}",
        if account.check_policy { "ON" } else { "OFF" }
    ));
    options.push(format!(
        "CHECK_EXPIRATION = {}",
        if account.check_expiration {
            "ON"
        } else {
            "OFF"
        }
    ));
    if !account.default_database.trim().is_empty() {
        options.push(format!(
            "DEFAULT_DATABASE = {}",
            quote_identifier(account.default_database.trim())
        ));
    }
    if !account.default_language.trim().is_empty() {
        options.push(format!(
            "DEFAULT_LANGUAGE = {}",
            quote_identifier(account.default_language.trim())
        ));
    }
    if !account.credential.trim().is_empty() {
        options.push(format!(
            "CREDENTIAL = {}",
            quote_identifier(account.credential.trim())
        ));
    }
    options
}

/// The identity/authentication statements for one save.
fn account_statements(edit: &UserEdit) -> Vec<String> {
    let account = &edit.account;
    let name = quote_identifier(&account.user);
    let mut statements = Vec::new();

    match &edit.original {
        None => {
            let create = if account.login_type.eq_ignore_ascii_case("Windows") {
                // A Windows login only accepts default database/language in its WITH list.
                let mut options = Vec::new();
                if !account.default_database.trim().is_empty() {
                    options.push(format!(
                        "DEFAULT_DATABASE = {}",
                        quote_identifier(account.default_database.trim())
                    ));
                }
                if !account.default_language.trim().is_empty() {
                    options.push(format!(
                        "DEFAULT_LANGUAGE = {}",
                        quote_identifier(account.default_language.trim())
                    ));
                }
                if options.is_empty() {
                    format!("CREATE LOGIN {name} FROM WINDOWS")
                } else {
                    format!(
                        "CREATE LOGIN {name} FROM WINDOWS WITH {}",
                        options.join(", ")
                    )
                }
            } else if account.login_type.eq_ignore_ascii_case("Certificate")
                && !account.certificate.trim().is_empty()
            {
                format!(
                    "CREATE LOGIN {name} FROM CERTIFICATE {}",
                    quote_identifier(account.certificate.trim())
                )
            } else if account.login_type.eq_ignore_ascii_case("Asymmetric Key")
                && !account.asymmetric_key.trim().is_empty()
            {
                format!(
                    "CREATE LOGIN {name} FROM ASYMMETRIC KEY {}",
                    quote_identifier(account.asymmetric_key.trim())
                )
            } else {
                let options =
                    create_login_options(account, Some(edit.password.as_deref().unwrap_or("")));
                format!("CREATE LOGIN {name} WITH {}", options.join(", "))
            };
            statements.push(create);
            if account.account_locked {
                statements.push(format!("ALTER LOGIN {name} DISABLE"));
            }
        }
        Some(original) => {
            let original_name = quote_identifier(&original.account.user);
            if original.account.user != account.user {
                statements.push(format!("ALTER LOGIN {original_name} WITH NAME = {name}"));
            }
            if let Some(password) = &edit.password {
                let mut sql = format!(
                    "ALTER LOGIN {name} WITH PASSWORD = {}",
                    quote_literal(password)
                );
                if let Some(old_password) = &edit.old_password {
                    sql.push_str(&format!(" OLD_PASSWORD = {}", quote_literal(old_password)));
                }
                if account.must_change {
                    sql.push_str(" MUST_CHANGE");
                }
                statements.push(sql);
            }
            if original.account.account_locked != account.account_locked {
                statements.push(format!(
                    "ALTER LOGIN {name} {}",
                    if account.account_locked {
                        "DISABLE"
                    } else {
                        "ENABLE"
                    }
                ));
            }
            let mut with = Vec::new();
            if original.account.default_database != account.default_database
                && !account.default_database.trim().is_empty()
            {
                with.push(format!(
                    "DEFAULT_DATABASE = {}",
                    quote_identifier(account.default_database.trim())
                ));
            }
            if original.account.default_language != account.default_language
                && !account.default_language.trim().is_empty()
            {
                with.push(format!(
                    "DEFAULT_LANGUAGE = {}",
                    quote_identifier(account.default_language.trim())
                ));
            }
            if !with.is_empty() {
                statements.push(format!("ALTER LOGIN {name} WITH {}", with.join(", ")));
            }
            let mut policy = Vec::new();
            if original.account.check_policy != account.check_policy {
                policy.push(format!(
                    "CHECK_POLICY = {}",
                    if account.check_policy { "ON" } else { "OFF" }
                ));
            }
            if original.account.check_expiration != account.check_expiration {
                policy.push(format!(
                    "CHECK_EXPIRATION = {}",
                    if account.check_expiration {
                        "ON"
                    } else {
                        "OFF"
                    }
                ));
            }
            if !policy.is_empty() {
                statements.push(format!("ALTER LOGIN {name} WITH {}", policy.join(", ")));
            }
            if original.account.credential != account.credential {
                // A login maps to at most one credential, so drop the old mapping before adding.
                if !original.account.credential.trim().is_empty() {
                    statements.push(format!(
                        "ALTER LOGIN {name} DROP CREDENTIAL {}",
                        quote_identifier(original.account.credential.trim())
                    ));
                }
                if !account.credential.trim().is_empty() {
                    statements.push(format!(
                        "ALTER LOGIN {name} ADD CREDENTIAL {}",
                        quote_identifier(account.credential.trim())
                    ));
                }
            }
        }
    }
    statements
}

/// Server-wide privilege statements, diffing the edit against the loaded grants/denies.
fn server_privilege_statements(edit: &UserEdit) -> Vec<String> {
    let name = quote_identifier(&edit.account.user);
    let (old_granted, old_denied) = edit
        .original
        .as_ref()
        .map(|original| {
            (
                original.server_privileges.clone(),
                original.denied_server_privileges.clone(),
            )
        })
        .unwrap_or_default();

    let mut revoke = Vec::new();
    let mut grant = Vec::new();
    let mut deny = Vec::new();
    let mut all: BTreeSet<PrivilegeId> = old_granted.clone();
    all.extend(old_denied.iter().cloned());
    all.extend(edit.server_privileges.iter().cloned());
    all.extend(edit.denied_server_privileges.iter().cloned());
    for privilege in all {
        let was_granted = old_granted.contains(&privilege);
        let was_denied = old_denied.contains(&privilege);
        let now_granted = edit.server_privileges.contains(&privilege);
        let now_denied = edit.denied_server_privileges.contains(&privilege);
        if was_granted == now_granted && was_denied == now_denied {
            continue;
        }
        if was_granted || was_denied {
            revoke.push(privilege.as_str().to_string());
        }
        if now_granted {
            grant.push(privilege.as_str().to_string());
        }
        if now_denied {
            deny.push(privilege.as_str().to_string());
        }
    }

    let mut statements = Vec::new();
    if !revoke.is_empty() {
        statements.push(format!("REVOKE {} FROM {name}", revoke.join(", ")));
    }
    if !grant.is_empty() {
        statements.push(format!("GRANT {} TO {name}", grant.join(", ")));
    }
    if !deny.is_empty() {
        statements.push(format!("DENY {} TO {name}", deny.join(", ")));
    }
    statements
}

/// Role membership statements (成员属于 plus any member grants shown by the editor).
fn role_statements(edit: &UserEdit) -> Vec<String> {
    let name = quote_identifier(&edit.account.user);
    let mut statements = Vec::new();
    let existing: BTreeSet<(String, String)> = edit
        .original
        .as_ref()
        .map(|original| {
            original
                .roles
                .iter()
                .map(|role| (role.role_user.clone(), role.role_host.clone()))
                .collect()
        })
        .unwrap_or_default();

    for (role, _host, _admin) in &edit.roles {
        if role.is_empty() {
            continue;
        }
        if !existing.contains(&(role.clone(), "SERVER".to_string())) {
            statements.push(format!(
                "ALTER SERVER ROLE {} ADD MEMBER {name}",
                quote_identifier(role)
            ));
        }
    }
    for role in &existing {
        if !edit
            .roles
            .iter()
            .any(|(candidate, _, _)| candidate == &role.0)
        {
            statements.push(format!(
                "ALTER SERVER ROLE {} DROP MEMBER {name}",
                quote_identifier(&role.0)
            ));
        }
    }
    statements
}

/// Object-level grant statements (best effort; the model's privileges map directly).
fn grant_statements(edit: &UserEdit) -> Vec<String> {
    let name = quote_identifier(&edit.account.user);
    let mut statements = Vec::new();
    for grant in &edit.grants {
        let database = quote_identifier(&grant.database);
        let target = if grant.name.is_empty() {
            format!("DATABASE::{database}")
        } else {
            format!("{database}.{}", quote_identifier(&grant.name))
        };
        for privilege in &grant.privileges {
            statements.push(format!(
                "GRANT {} ON {target} TO {name}",
                privilege.as_str()
            ));
        }
    }
    statements
}

/// The `CREATE USER ... FOR LOGIN` guard for one database (run after a `USE`), so a login mapped
/// into a database gets a user without failing when one already exists.
fn create_mapping_user_statement(user: &str, login: &str) -> String {
    let user_name = quote_identifier(user);
    let login_name = quote_identifier(login);
    let user_literal = quote_literal(user);
    let login_literal = quote_literal(login);
    format!(
        "IF SUSER_ID({login_literal}) IS NOT NULL AND NOT EXISTS \
         (SELECT 1 FROM sys.database_principals WHERE name = {user_literal}) \
         CREATE USER {user_name} FOR LOGIN {login_name}"
    )
}

/// Per-database mapping statements (SQL Server's 用户映射). Each mapped database is preceded by a
/// `USE [db]`, so the login's database users, their default schemas and their database-role
/// memberships are created, changed or dropped in the right database. A database with no change
/// emits nothing, so an untouched mapping never appears in the preview.
fn mapping_statements(edit: &UserEdit) -> Vec<String> {
    let login = &edit.account.user;
    let original: BTreeMap<&str, &UserMapping> = edit
        .original_mappings
        .iter()
        .map(|mapping| (mapping.database.as_str(), mapping))
        .collect();

    let mut statements = Vec::new();
    for mapping in &edit.mappings {
        let previous = original.get(mapping.database.as_str()).copied();
        let was_mapped = previous.map(|m| m.mapped).unwrap_or(false);
        let old_name = previous
            .map(|m| m.user_name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| login.clone());
        let old_schema = previous
            .map(|m| m.default_schema.clone())
            .unwrap_or_default();
        let old_roles: BTreeSet<String> = previous.map(|m| m.roles.clone()).unwrap_or_default();
        let new_name = if mapping.user_name.trim().is_empty() {
            login.clone()
        } else {
            mapping.user_name.clone()
        };
        let new_user = quote_identifier(&new_name);

        let mut local = Vec::new();
        if !was_mapped && mapping.mapped {
            local.push(create_mapping_user_statement(&new_name, login));
            let schema = mapping.default_schema.trim();
            if !schema.is_empty() && !schema.eq_ignore_ascii_case("dbo") {
                local.push(format!(
                    "ALTER USER {new_user} WITH DEFAULT_SCHEMA = {}",
                    quote_identifier(schema)
                ));
            }
            for role in &mapping.roles {
                local.push(format!(
                    "ALTER ROLE {} ADD MEMBER {new_user}",
                    quote_identifier(role)
                ));
            }
        } else if was_mapped && !mapping.mapped {
            local.push(format!("DROP USER {}", quote_identifier(&old_name)));
        } else if was_mapped && mapping.mapped {
            if new_name != old_name {
                local.push(format!(
                    "ALTER USER {} WITH NAME = {new_user}",
                    quote_identifier(&old_name)
                ));
            }
            let schema = mapping.default_schema.trim();
            if !schema.is_empty() && !schema.eq_ignore_ascii_case(&old_schema) {
                local.push(format!(
                    "ALTER USER {new_user} WITH DEFAULT_SCHEMA = {}",
                    quote_identifier(schema)
                ));
            }
            for role in mapping.roles.difference(&old_roles) {
                local.push(format!(
                    "ALTER ROLE {} ADD MEMBER {new_user}",
                    quote_identifier(role)
                ));
            }
            for role in old_roles.difference(&mapping.roles) {
                local.push(format!(
                    "ALTER ROLE {} DROP MEMBER {new_user}",
                    quote_identifier(role)
                ));
            }
        }
        if !local.is_empty() {
            statements.push(format!("USE {}", quote_identifier(&mapping.database)));
            statements.extend(local);
        }
    }
    statements
}

/// Server-level securable grant statements (SQL Server's `ON ENDPOINT::` / `ON LOGIN::`),
/// diffing the edit against the loaded grants.
fn securable_statements(edit: &UserEdit) -> Vec<String> {
    let login = quote_identifier(&edit.account.user);
    let original: BTreeMap<(String, String), &ServerSecurableGrant> = edit
        .original_securables
        .iter()
        .map(|grant| ((grant.class.clone(), grant.name.clone()), grant))
        .collect();

    let mut statements = Vec::new();
    for grant in &edit.securables {
        let key = (grant.class.clone(), grant.name.clone());
        let (old_granted, old_denied) = original
            .get(&key)
            .map(|loaded| (loaded.privileges.clone(), loaded.denied.clone()))
            .unwrap_or_default();
        let mut revoke = Vec::new();
        let mut add_grant = Vec::new();
        let mut add_deny = Vec::new();
        let mut all: BTreeSet<PrivilegeId> = old_granted.clone();
        all.extend(old_denied.iter().cloned());
        all.extend(grant.privileges.iter().cloned());
        all.extend(grant.denied.iter().cloned());
        for privilege in all {
            let was_granted = old_granted.contains(&privilege);
            let was_denied = old_denied.contains(&privilege);
            let now_granted = grant.privileges.contains(&privilege);
            let now_denied = grant.denied.contains(&privilege);
            if was_granted == now_granted && was_denied == now_denied {
                continue;
            }
            if was_granted || was_denied {
                revoke.push(privilege.as_str().to_string());
            }
            if now_granted {
                add_grant.push(privilege.as_str().to_string());
            }
            if now_denied {
                add_deny.push(privilege.as_str().to_string());
            }
        }
        if revoke.is_empty() && add_grant.is_empty() && add_deny.is_empty() {
            continue;
        }
        let target = format!("{}::{}", grant.class, quote_identifier(&grant.name));
        if !revoke.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {target} FROM {login}",
                revoke.join(", ")
            ));
        }
        if !add_grant.is_empty() {
            statements.push(format!(
                "GRANT {} ON {target} TO {login}",
                add_grant.join(", ")
            ));
        }
        if !add_deny.is_empty() {
            statements.push(format!(
                "DENY {} ON {target} TO {login}",
                add_deny.join(", ")
            ));
        }
    }
    statements
}

pub(crate) fn user_edit_groups(edit: &UserEdit) -> Vec<(UserEditSection, Vec<String>)> {
    let mut groups = Vec::new();
    for (section, statements) in [
        (UserEditSection::Account, account_statements(edit)),
        (
            UserEditSection::ServerPrivileges,
            server_privilege_statements(edit),
        ),
        (UserEditSection::ObjectGrants, grant_statements(edit)),
        (UserEditSection::Roles, role_statements(edit)),
        (UserEditSection::Securables, securable_statements(edit)),
        // The per-database 用户映射 runs last: its `USE [db]` must not leak into a later group.
        (UserEditSection::UserMapping, mapping_statements(edit)),
    ] {
        if !statements.is_empty() {
            groups.push((section, statements));
        }
    }
    groups
}

pub(crate) fn user_edit_sql(edit: &UserEdit) -> String {
    let statements: Vec<String> = user_edit_groups(edit)
        .into_iter()
        .flat_map(|(_, statements)| statements)
        .collect();
    if statements.is_empty() {
        return String::new();
    }
    // Server-scope logins/permissions (and `ALTER SERVER ROLE`) are only valid in `master`; the
    // connection's default database may be a user database, so pin the script to master. The
    // 用户映射 group's own `USE [db]` switches context per database afterwards.
    format!("USE [master];\n{}", statements.join(";\n"))
}

pub(crate) async fn save_user(connection: &SqlServerConnection, edit: &UserEdit) -> Result<()> {
    let sql = user_edit_sql(edit);
    if sql.trim().is_empty() {
        return Ok(());
    }
    connection.execute_query(None, &sql).await.map(|_| ())
}

pub(crate) async fn drop_user(
    connection: &SqlServerConnection,
    user: &str,
    _host: &str,
) -> Result<()> {
    let sql = format!("DROP LOGIN {}", quote_identifier(user));
    connection.execute_batch(None, &sql).await
}

pub(crate) async fn rename_user(
    connection: &SqlServerConnection,
    user: &str,
    _host: &str,
    new_user: &str,
    _new_host: &str,
) -> Result<()> {
    let sql = format!(
        "ALTER LOGIN {} WITH NAME = {}",
        quote_identifier(user),
        quote_identifier(new_user)
    );
    connection.execute_batch(None, &sql).await
}

// ----- Object privileges (the 对象权限管理器) ---------------------------------------------------

/// Split a `schema.object` name; a bare name belongs to `dbo`.
fn split_object(name: &str) -> (String, String) {
    match name.split_once('.') {
        Some((schema, object)) => (schema.to_string(), object.to_string()),
        None => ("dbo".to_string(), name.to_string()),
    }
}

/// The granted and denied privilege sets of one principal on one object.
type CurrentPrivileges = (BTreeSet<PrivilegeId>, BTreeSet<PrivilegeId>);

/// Every principal's privileges on one database object, for the privilege manager's matrix. The
/// principal is reported by its **login** name (`SUSER_SNAME`) so the matrix lines up with the
/// Users tab's login list. `name` is empty for a database-wide (`DATABASE::`) grant.
pub(crate) async fn object_privilege_matrix(
    connection: &SqlServerConnection,
    database: &str,
    name: &str,
) -> Result<Vec<ObjectPrivilegeRow>> {
    let db = quote_identifier(database);
    let database_wide = name.is_empty();
    let (filter, params): (String, Vec<Option<String>>) = if database_wide {
        ("dp.class = 0".to_string(), Vec::new())
    } else {
        let (schema, object) = split_object(name);
        (
            "dp.class = 1 AND s.name = @P1 AND o.name = @P2".to_string(),
            vec![Some(schema), Some(object)],
        )
    };
    let joins = if database_wide {
        format!("JOIN {db}.sys.database_principals p ON p.principal_id = dp.grantee_principal_id")
    } else {
        format!(
            "JOIN {db}.sys.database_principals p ON p.principal_id = dp.grantee_principal_id \
             JOIN {db}.sys.objects o ON o.object_id = dp.major_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id"
        )
    };
    let sql = format!(
        "SELECT COALESCE(SUSER_SNAME(p.sid), p.name), dp.permission_name, dp.state \
         FROM {db}.sys.database_permissions dp {joins} \
         WHERE {filter} AND dp.state IN ('G', 'W', 'D') \
         ORDER BY 1, dp.permission_name"
    );
    let result = connection.run(None, &sql, &params).await?;

    let mut rows: Vec<ObjectPrivilegeRow> = Vec::new();
    for row in &result.rows {
        let principal = row.first().map(CellValue::as_display).unwrap_or_default();
        let permission = row.get(1).map(CellValue::as_display).unwrap_or_default();
        let state = row.get(2).map(CellValue::as_display).unwrap_or_default();
        let privilege = PrivilegeId::new(permission.trim().to_ascii_uppercase());
        if principal.is_empty() || privilege.as_str().is_empty() {
            continue;
        }
        let denied = state.eq_ignore_ascii_case("D");
        match rows.iter_mut().find(|entry| entry.user == principal) {
            Some(entry) => {
                if denied {
                    entry.denied.insert(privilege);
                } else {
                    entry.privileges.insert(privilege);
                }
            }
            None => {
                let mut entry = ObjectPrivilegeRow::new(principal, String::new());
                if denied {
                    entry.denied.insert(privilege);
                } else {
                    entry.privileges.insert(privilege);
                }
                rows.push(entry);
            }
        }
    }
    Ok(rows)
}

/// Replace the listed principals' privileges on one object, leaving every other grant untouched.
pub(crate) async fn set_object_privileges(
    connection: &SqlServerConnection,
    database: &str,
    name: &str,
    rows: &[ObjectPrivilegeRow],
) -> Result<()> {
    let current = current_privileges(connection, database, name).await?;
    for statement in object_privilege_statements(database, name, &current, rows) {
        connection.execute_batch(Some(database), &statement).await?;
    }
    Ok(())
}

/// The statements that turn `current` into `rows`, for the editor's SQL preview.
pub(crate) fn object_privileges_sql(
    database: &str,
    name: &str,
    original: &[ObjectPrivilegeRow],
    rows: &[ObjectPrivilegeRow],
) -> String {
    let current: BTreeMap<(String, String), CurrentPrivileges> = original
        .iter()
        .map(|row| {
            (
                (row.user.clone(), row.host.clone()),
                (row.privileges.clone(), row.denied.clone()),
            )
        })
        .collect();
    let statements = object_privilege_statements(database, name, &current, rows);
    if statements.is_empty() {
        return String::new();
    }
    let mut script = statements.join(";\n");
    script.push(';');
    script
}

/// The matrix keyed by `(user, host)`, for the statement diff.
async fn current_privileges(
    connection: &SqlServerConnection,
    database: &str,
    name: &str,
) -> Result<BTreeMap<(String, String), CurrentPrivileges>> {
    Ok(object_privilege_matrix(connection, database, name)
        .await?
        .into_iter()
        .map(|row| ((row.user, row.host), (row.privileges, row.denied)))
        .collect())
}

fn object_privilege_statements(
    database: &str,
    name: &str,
    current: &BTreeMap<(String, String), CurrentPrivileges>,
    rows: &[ObjectPrivilegeRow],
) -> Vec<String> {
    let target = grant_target(database, name);
    let mut statements = Vec::new();
    for row in rows {
        let key = (row.user.clone(), row.host.clone());
        let (old_granted, old_denied) = current.get(&key).cloned().unwrap_or_default();
        let nothing_new = row.privileges.is_empty() && row.denied.is_empty();
        if old_granted.is_empty() && old_denied.is_empty() && nothing_new {
            continue;
        }
        // A login needs a database user before it can hold object permissions.
        if !current.contains_key(&key) && !nothing_new {
            statements.push(create_user_statement(database, &row.user));
        }
        let user = quote_identifier(&row.user);
        let mut revoke: Vec<String> = Vec::new();
        let mut grant: Vec<String> = Vec::new();
        let mut deny: Vec<String> = Vec::new();
        let mut all: BTreeSet<PrivilegeId> = old_granted.clone();
        all.extend(old_denied.iter().cloned());
        all.extend(row.privileges.iter().cloned());
        all.extend(row.denied.iter().cloned());
        for privilege in all {
            let was_granted = old_granted.contains(&privilege);
            let was_denied = old_denied.contains(&privilege);
            let now_granted = row.privileges.contains(&privilege);
            let now_denied = row.denied.contains(&privilege);
            if was_granted == now_granted && was_denied == now_denied {
                continue;
            }
            if was_granted || was_denied {
                revoke.push(privilege.as_str().to_string());
            }
            if now_granted {
                grant.push(privilege.as_str().to_string());
            }
            if now_denied {
                deny.push(privilege.as_str().to_string());
            }
        }
        if !revoke.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {} FROM {}",
                revoke.join(", "),
                target,
                user
            ));
        }
        if !grant.is_empty() {
            statements.push(format!(
                "GRANT {} ON {} TO {}",
                grant.join(", "),
                target,
                user
            ));
        }
        if !deny.is_empty() {
            statements.push(format!(
                "DENY {} ON {} TO {}",
                deny.join(", "),
                target,
                user
            ));
        }
    }
    // Principals dropped from the matrix lose every privilege they held on the object.
    for ((user, _host), (old_granted, old_denied)) in current {
        if rows.iter().any(|row| &row.user == user) {
            continue;
        }
        let mut removed: Vec<String> = old_granted
            .iter()
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        removed.extend(
            old_denied
                .iter()
                .map(|privilege| privilege.as_str().to_string()),
        );
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {} FROM {}",
                removed.join(", "),
                target,
                quote_identifier(user)
            ));
        }
    }
    statements
}

/// A guarded `CREATE USER ... FOR LOGIN`, so a login can be granted object permissions that only
/// apply to a database user. A no-op when the database user (or the login) already exists.
fn create_user_statement(database: &str, login: &str) -> String {
    let db = quote_identifier(database);
    let name = quote_identifier(login);
    let literal = quote_literal(login);
    format!(
        "IF SUSER_ID({literal}) IS NOT NULL AND NOT EXISTS \
         (SELECT 1 FROM {db}.sys.database_principals WHERE name = {literal}) \
         CREATE USER {name} FOR LOGIN {name}"
    )
}

/// The `ON` clause: `DATABASE::db` for a database-wide grant, else `[schema].[object]`.
fn grant_target(database: &str, name: &str) -> String {
    if name.is_empty() {
        format!("DATABASE::{}", quote_identifier(database))
    } else {
        let (schema, object) = split_object(name);
        format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&object)
        )
    }
}

pub(crate) fn authentication_plugins() -> Vec<&'static str> {
    vec!["SQL Server"]
}

/// The login verification types offered by the editor's 验证类型 dropdown.
pub(crate) fn login_types() -> Vec<&'static str> {
    vec!["SQL Server", "Windows"]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(user: &str, privileges: &[&str]) -> Vec<ObjectPrivilegeRow> {
        vec![ObjectPrivilegeRow {
            user: user.to_string(),
            host: String::new(),
            privileges: privileges
                .iter()
                .map(|name| PrivilegeId::new(*name))
                .collect(),
            denied: BTreeSet::new(),
        }]
    }

    fn granted(entries: &[(&str, &[&str])]) -> BTreeMap<(String, String), CurrentPrivileges> {
        entries
            .iter()
            .map(|(user, privileges)| {
                (
                    (user.to_string(), String::new()),
                    (
                        privileges
                            .iter()
                            .map(|name| PrivilegeId::new(*name))
                            .collect(),
                        BTreeSet::new(),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn object_statements_grant_and_revoke_on_a_table() {
        // alice already has SELECT; the edit adds INSERT.
        let current = granted(&[("alice", &["SELECT"])]);
        let statements = object_privilege_statements(
            "shop",
            "dbo.users",
            &current,
            &rows("alice", &["SELECT", "INSERT"]),
        );
        assert_eq!(statements, vec!["GRANT INSERT ON [dbo].[users] TO [alice]"]);

        let removed =
            object_privilege_statements("shop", "dbo.users", &current, &rows("alice", &[]));
        assert_eq!(removed, vec!["REVOKE SELECT ON [dbo].[users] FROM [alice]"]);
    }

    #[test]
    fn a_new_grantee_gets_a_database_user_first() {
        let statements =
            object_privilege_statements("shop", "", &BTreeMap::new(), &rows("bob", &["CONNECT"]));
        assert_eq!(
            statements,
            vec![
                create_user_statement("shop", "bob"),
                "GRANT CONNECT ON DATABASE::[shop] TO [bob]".to_string(),
            ]
        );
    }

    #[test]
    fn lifting_a_deny_revokes_then_grants() {
        let current = BTreeMap::from([(
            ("alice".to_string(), String::new()),
            (
                BTreeSet::new(),
                BTreeSet::from([PrivilegeId::new("SELECT")]),
            ),
        )]);
        let rows = vec![ObjectPrivilegeRow {
            user: "alice".to_string(),
            host: String::new(),
            privileges: BTreeSet::from([PrivilegeId::new("SELECT")]),
            denied: BTreeSet::new(),
        }];
        let statements = object_privilege_statements("shop", "dbo.users", &current, &rows);
        assert_eq!(
            statements,
            vec![
                "REVOKE SELECT ON [dbo].[users] FROM [alice]",
                "GRANT SELECT ON [dbo].[users] TO [alice]",
            ]
        );
    }

    #[test]
    fn denying_a_granted_privilege_revokes_then_denies() {
        let current = granted(&[("alice", &["SELECT"])]);
        let rows = vec![ObjectPrivilegeRow {
            user: "alice".to_string(),
            host: String::new(),
            privileges: BTreeSet::new(),
            denied: BTreeSet::from([PrivilegeId::new("SELECT")]),
        }];
        let statements = object_privilege_statements("shop", "dbo.users", &current, &rows);
        assert_eq!(
            statements,
            vec![
                "REVOKE SELECT ON [dbo].[users] FROM [alice]",
                "DENY SELECT ON [dbo].[users] TO [alice]",
            ]
        );
    }

    #[test]
    fn a_bare_object_name_defaults_to_dbo() {
        assert_eq!(
            split_object("users"),
            ("dbo".to_string(), "users".to_string())
        );
        assert_eq!(
            split_object("sales.orders"),
            ("sales".to_string(), "orders".to_string())
        );
    }

    #[test]
    fn denying_a_server_privilege_revokes_then_denies() {
        let original = UserDetails {
            server_privileges: BTreeSet::from([PrivilegeId::new("CONTROL SERVER")]),
            ..Default::default()
        };
        let edit = UserEdit {
            original: Some(original),
            account: UserAccount {
                user: "sa".to_string(),
                ..Default::default()
            },
            password: None,
            server_privileges: BTreeSet::new(),
            denied_server_privileges: BTreeSet::from([PrivilegeId::new("CONTROL SERVER")]),
            grants: Vec::new(),
            default_privileges: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
            mappings: Vec::new(),
            original_mappings: Vec::new(),
            old_password: None,
            securables: Vec::new(),
            original_securables: Vec::new(),
        };
        assert_eq!(
            server_privilege_statements(&edit),
            vec![
                "REVOKE CONTROL SERVER FROM [sa]",
                "DENY CONTROL SERVER TO [sa]",
            ]
        );
    }

    #[test]
    fn guarded_create_user_uses_a_single_n_prefix() {
        // `quote_literal` already prepends `N`; adding another yields `NN'...'` and a syntax error.
        assert_eq!(
            create_mapping_user_statement("bob", "bob"),
            "IF SUSER_ID(N'bob') IS NOT NULL AND NOT EXISTS \
             (SELECT 1 FROM sys.database_principals WHERE name = N'bob') \
             CREATE USER [bob] FOR LOGIN [bob]"
        );
        assert_eq!(
            create_user_statement("shop", "bob"),
            "IF SUSER_ID(N'bob') IS NOT NULL AND NOT EXISTS \
             (SELECT 1 FROM [shop].sys.database_principals WHERE name = N'bob') \
             CREATE USER [bob] FOR LOGIN [bob]"
        );
    }

    fn mapping(database: &str, mapped: bool, user_name: &str, roles: &[&str]) -> UserMapping {
        UserMapping {
            database: database.to_string(),
            mapped,
            user_name: user_name.to_string(),
            default_schema: "dbo".to_string(),
            roles: roles.iter().map(|role| role.to_string()).collect(),
            available_roles: Vec::new(),
        }
    }

    fn mapping_edit(original: Vec<UserMapping>, mappings: Vec<UserMapping>) -> UserEdit {
        UserEdit {
            original: Some(UserDetails {
                account: UserAccount {
                    user: "alice".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            account: UserAccount {
                user: "alice".to_string(),
                ..Default::default()
            },
            password: None,
            server_privileges: BTreeSet::new(),
            denied_server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            default_privileges: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
            mappings,
            original_mappings: original,
            old_password: None,
            securables: Vec::new(),
            original_securables: Vec::new(),
        }
    }

    #[test]
    fn mapping_adds_a_user_and_role_and_changes_only_the_diff() {
        // alice is in shop with db_datareader; sales is new.
        let edit = mapping_edit(
            vec![mapping("shop", true, "alice", &["db_datareader"])],
            vec![
                mapping("shop", true, "alice", &["db_datareader", "db_datawriter"]),
                mapping("sales", true, "alice", &[]),
            ],
        );
        assert_eq!(
            mapping_statements(&edit),
            vec![
                "USE [shop]".to_string(),
                "ALTER ROLE [db_datawriter] ADD MEMBER [alice]".to_string(),
                "USE [sales]".to_string(),
                create_mapping_user_statement("alice", "alice"),
            ]
        );
    }

    #[test]
    fn mapping_unchanged_emits_nothing() {
        let edit = mapping_edit(
            vec![mapping("shop", true, "alice", &["db_datareader"])],
            vec![mapping("shop", true, "alice", &["db_datareader"])],
        );
        assert!(mapping_statements(&edit).is_empty());
    }

    #[test]
    fn unmapping_drops_the_database_user() {
        let edit = mapping_edit(
            vec![mapping("shop", true, "alice", &["db_datareader"])],
            vec![mapping("shop", false, "alice", &[])],
        );
        assert_eq!(
            mapping_statements(&edit),
            vec!["USE [shop]", "DROP USER [alice]"]
        );
    }

    fn securable(
        class: &str,
        name: &str,
        granted: &[&str],
        denied: &[&str],
    ) -> ServerSecurableGrant {
        ServerSecurableGrant {
            class: class.to_string(),
            name: name.to_string(),
            privileges: granted.iter().map(|p| PrivilegeId::new(*p)).collect(),
            denied: denied.iter().map(|p| PrivilegeId::new(*p)).collect(),
        }
    }

    fn securable_edit(
        original: Vec<ServerSecurableGrant>,
        changes: Vec<ServerSecurableGrant>,
    ) -> UserEdit {
        UserEdit {
            original: None,
            account: UserAccount {
                user: "alice".to_string(),
                ..Default::default()
            },
            password: None,
            server_privileges: BTreeSet::new(),
            denied_server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            default_privileges: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
            mappings: Vec::new(),
            original_mappings: Vec::new(),
            old_password: None,
            securables: changes,
            original_securables: original,
        }
    }

    #[test]
    fn securables_grant_and_deny_by_class() {
        let edit = securable_edit(
            Vec::new(),
            vec![
                securable("LOGIN", "bob", &["IMPERSONATE"], &[]),
                securable("ENDPOINT", "TSQL", &[], &["CONNECT"]),
            ],
        );
        assert_eq!(
            securable_statements(&edit),
            vec![
                "GRANT IMPERSONATE ON LOGIN::[bob] TO [alice]",
                "DENY CONNECT ON ENDPOINT::[TSQL] TO [alice]",
            ]
        );
    }

    #[test]
    fn securables_diff_revokes_then_regrants() {
        let edit = securable_edit(
            vec![securable("LOGIN", "bob", &["IMPERSONATE"], &[])],
            vec![securable("LOGIN", "bob", &["CONTROL"], &[])],
        );
        assert_eq!(
            securable_statements(&edit),
            vec![
                "REVOKE IMPERSONATE ON LOGIN::[bob] FROM [alice]",
                "GRANT CONTROL ON LOGIN::[bob] TO [alice]",
            ]
        );
    }

    fn base_edit(account: UserAccount, original: Option<UserDetails>) -> UserEdit {
        UserEdit {
            original,
            account,
            password: None,
            server_privileges: BTreeSet::new(),
            denied_server_privileges: BTreeSet::new(),
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

    #[test]
    fn create_sql_login_emits_the_options_in_order() {
        let mut edit = base_edit(
            UserAccount {
                user: "app".to_string(),
                login_type: "SQL Server".to_string(),
                check_policy: true,
                check_expiration: true,
                must_change: true,
                default_database: "shop".to_string(),
                default_language: "us_english".to_string(),
                credential: "cred".to_string(),
                ..Default::default()
            },
            None,
        );
        edit.password = Some("p'w".to_string());
        assert_eq!(
            account_statements(&edit),
            vec![
                "CREATE LOGIN [app] WITH PASSWORD = N'p''w' MUST_CHANGE, CHECK_POLICY = ON, \
                 CHECK_EXPIRATION = ON, DEFAULT_DATABASE = [shop], DEFAULT_LANGUAGE = [us_english], \
                 CREDENTIAL = [cred]"
            ]
        );
    }

    #[test]
    fn create_windows_login_has_no_password_or_policy() {
        let edit = base_edit(
            UserAccount {
                user: "BUILTIN\\Users".to_string(),
                login_type: "Windows".to_string(),
                check_policy: true,
                must_change: true,
                default_database: "shop".to_string(),
                ..Default::default()
            },
            None,
        );
        assert_eq!(
            account_statements(&edit),
            vec!["CREATE LOGIN [BUILTIN\\Users] FROM WINDOWS WITH DEFAULT_DATABASE = [shop]"]
        );
    }

    #[test]
    fn alter_login_applies_defaults_policy_and_credential() {
        let original = UserDetails {
            account: UserAccount {
                user: "app".to_string(),
                default_database: "master".to_string(),
                credential: "old".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let edit = base_edit(
            UserAccount {
                user: "app".to_string(),
                default_database: "shop".to_string(),
                default_language: "us_english".to_string(),
                check_policy: true,
                credential: "cred".to_string(),
                ..Default::default()
            },
            Some(original),
        );
        assert_eq!(
            account_statements(&edit),
            vec![
                "ALTER LOGIN [app] WITH DEFAULT_DATABASE = [shop], DEFAULT_LANGUAGE = [us_english]",
                "ALTER LOGIN [app] WITH CHECK_POLICY = ON",
                "ALTER LOGIN [app] DROP CREDENTIAL [old]",
                "ALTER LOGIN [app] ADD CREDENTIAL [cred]",
            ]
        );
    }
}

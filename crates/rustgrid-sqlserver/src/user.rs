//! SQL Server account (login) management. Mapped onto the engine-agnostic user models;
//! SQL Server has no `user@host` split, so the host part is always empty and accounts are
//! identified by login name alone.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    CellValue, Connection, Error, ObjectPrivilegeRow, PrivilegeCatalog, PrivilegeGroup,
    PrivilegeId, PrivilegeInfo, PrivilegePreset, Result, RoleMembership, UserAccount, UserDetails,
    UserEdit, UserEditSection,
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
        deny_supported: true,
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

pub(crate) async fn user_details(
    connection: &SqlServerConnection,
    user: &str,
    _host: &str,
) -> Result<UserDetails> {
    let account = load_accounts(connection, Some(user))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Query(format!("login {user} not found")))?;

    let permission_sql = "SELECT sp.permission_name, sp.state FROM sys.server_permissions sp \
         JOIN sys.server_principals p ON p.principal_id = sp.grantee_principal_id \
         WHERE p.name = @P1 AND sp.state IN ('G', 'W', 'D')";
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
        roles,
        members,
    })
}

/// The identity/authentication statements for one save.
fn account_statements(edit: &UserEdit) -> Vec<String> {
    let account = &edit.account;
    let name = quote_identifier(&account.user);
    let mut statements = Vec::new();

    match &edit.original {
        None => {
            let mut sql = format!(
                "CREATE LOGIN {name} WITH PASSWORD = {}",
                quote_literal(edit.password.as_deref().unwrap_or(""))
            );
            if account.account_locked {
                sql.push_str(", CHECK_POLICY = OFF");
            }
            statements.push(sql);
        }
        Some(original) => {
            let original_name = quote_identifier(&original.account.user);
            if original.account.user != account.user {
                statements.push(format!("ALTER LOGIN {original_name} WITH NAME = {name}"));
            }
            if let Some(password) = &edit.password {
                statements.push(format!(
                    "ALTER LOGIN {name} WITH PASSWORD = {}",
                    quote_literal(password)
                ));
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
    ] {
        if !statements.is_empty() {
            groups.push((section, statements));
        }
    }
    groups
}

pub(crate) fn user_edit_sql(edit: &UserEdit) -> String {
    user_edit_groups(edit)
        .into_iter()
        .flat_map(|(_, statements)| statements)
        .collect::<Vec<_>>()
        .join(";\n")
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
        "IF SUSER_ID(N{literal}) IS NOT NULL AND NOT EXISTS \
         (SELECT 1 FROM {db}.sys.database_principals WHERE name = N{literal}) \
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
            roles: Vec::new(),
            members: Vec::new(),
        };
        assert_eq!(
            server_privilege_statements(&edit),
            vec![
                "REVOKE CONTROL SERVER FROM [sa]",
                "DENY CONTROL SERVER TO [sa]",
            ]
        );
    }
}

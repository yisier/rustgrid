//! SQL Server account (login) management. Mapped onto the engine-agnostic user models;
//! SQL Server has no `user@host` split, so the host part is always empty and accounts are
//! identified by login name alone.

use std::collections::BTreeSet;

use rustgrid_core::{
    Connection, Error, Privilege, Result, RoleMembership, UserAccount, UserDetails, UserEdit,
    UserEditSection,
};

use crate::SqlServerConnection;
use crate::connection::privilege_set;
use crate::helpers::{quote_identifier, quote_literal};

/// The server-level permissions that can be surfaced through the shared privilege model.
fn server_permission(privilege: Privilege) -> Option<&'static str> {
    match privilege {
        Privilege::Super => Some("CONTROL SERVER"),
        Privilege::Process => Some("VIEW SERVER STATE"),
        Privilege::CreateUser => Some("ALTER ANY LOGIN"),
        Privilege::Shutdown => Some("SHUTDOWN"),
        Privilege::ShowDatabases => Some("VIEW ANY DATABASE"),
        Privilege::Alter => Some("ALTER ANY DATABASE"),
        _ => None,
    }
}

/// The reverse of [`server_permission`], from a SQL Server permission name.
fn privilege_from_server_permission(name: &str) -> Option<Privilege> {
    let normalized = name.trim().to_ascii_uppercase();
    if let Some(privilege) = Privilege::ALL
        .into_iter()
        .find(|privilege| server_permission(*privilege).is_some_and(|value| value == normalized))
    {
        return Some(privilege);
    }
    Privilege::from_sql_name(&normalized)
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
        account_locked: disabled,
        max_questions: 0,
        max_updates: 0,
        max_connections: 0,
        max_user_connections: 0,
        ssl_type: String::new(),
        ssl_cipher: String::new(),
        x509_issuer: String::new(),
        x509_subject: String::new(),
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

    let permission_sql = "SELECT sp.permission_name FROM sys.server_permissions sp \
         JOIN sys.server_principals p ON p.principal_id = sp.grantee_principal_id \
         WHERE p.name = @P1 AND sp.state IN ('G', 'W')";
    let permissions = connection
        .run(None, permission_sql, &[Some(user.to_string())])
        .await?;
    let server_privileges: BTreeSet<Privilege> = if account.is_super_user {
        // A sysadmin can do anything; present the full set rather than the empty
        // explicit-grant list.
        Privilege::ALL.into_iter().collect()
    } else {
        privilege_set(
            permissions
                .rows
                .iter()
                .filter_map(|row| row.first().map(rustgrid_core::CellValue::as_display))
                .filter_map(|name| privilege_from_server_permission(&name))
                .map(|privilege| privilege.sql_name().to_string()),
        )
    };

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

/// Server-wide privilege statements, diffing the edit against the loaded grants.
fn server_privilege_statements(edit: &UserEdit) -> Vec<String> {
    let name = quote_identifier(&edit.account.user);
    let mut statements = Vec::new();
    let original: BTreeSet<Privilege> = edit
        .original
        .as_ref()
        .map(|original| original.server_privileges.iter().copied().collect())
        .unwrap_or_default();

    for privilege in &edit.server_privileges {
        if let Some(permission) = server_permission(*privilege)
            && !original.contains(privilege)
        {
            statements.push(format!("GRANT {permission} TO {name}"));
        }
    }
    for privilege in &original {
        if !edit.server_privileges.contains(privilege)
            && let Some(permission) = server_permission(*privilege)
        {
            statements.push(format!("REVOKE {permission} FROM {name}"));
        }
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
                privilege.sql_name()
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

pub(crate) fn authentication_plugins() -> Vec<&'static str> {
    vec!["SQL Server"]
}

//! Oracle account (user/role) catalog access and `CREATE/ALTER/DROP USER` + `GRANT`/`REVOKE`
//! generation.
//!
//! Oracle has no `user@host` model: accounts are schema-wide, so `UserAccount::host` is always
//! empty. In Oracle a schema *is* a user, so the Users tab and the connection tree's schema list
//! intentionally overlap.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    Error, ObjectGrant, ObjectPrivilegeRow, Privilege, Result, RoleMembership, UserAccount,
    UserDetails, UserEdit, UserEditSection,
};

use crate::connection::{OracleConnection, map_query_error};
use crate::helpers::{qualify, quote_identifier};

/// The Oracle system privilege for a core privilege, when one exists.
fn server_privilege(privilege: Privilege) -> Option<&'static str> {
    match privilege {
        Privilege::Alter => Some("ALTER ANY TABLE"),
        Privilege::AlterRoutine => Some("ALTER ANY PROCEDURE"),
        Privilege::Create => Some("CREATE TABLE"),
        Privilege::CreateRoutine => Some("CREATE PROCEDURE"),
        Privilege::CreateUser => Some("CREATE USER"),
        Privilege::CreateView => Some("CREATE VIEW"),
        Privilege::Delete => Some("DELETE ANY TABLE"),
        Privilege::Drop => Some("DROP ANY TABLE"),
        Privilege::Execute => Some("EXECUTE ANY PROCEDURE"),
        Privilege::Index => Some("CREATE ANY INDEX"),
        Privilege::Insert => Some("INSERT ANY TABLE"),
        Privilege::Select => Some("SELECT ANY TABLE"),
        Privilege::Update => Some("UPDATE ANY TABLE"),
        _ => None,
    }
}

/// The core privilege for an Oracle system privilege name.
fn server_privilege_from(name: &str) -> Option<Privilege> {
    let upper = name.to_ascii_uppercase();
    Privilege::ALL
        .into_iter()
        .find(|privilege| server_privilege(*privilege).is_some_and(|value| value == upper))
}

/// The Oracle object-level privilege for a core privilege, when one exists.
fn object_privilege(privilege: Privilege) -> Option<&'static str> {
    match privilege {
        Privilege::Select => Some("SELECT"),
        Privilege::Insert => Some("INSERT"),
        Privilege::Update => Some("UPDATE"),
        Privilege::Delete => Some("DELETE"),
        Privilege::Alter => Some("ALTER"),
        Privilege::Index => Some("INDEX"),
        Privilege::References => Some("REFERENCES"),
        Privilege::Execute => Some("EXECUTE"),
        _ => None,
    }
}

/// The core privilege for an Oracle object privilege name.
fn object_privilege_from(name: &str) -> Option<Privilege> {
    match name.to_ascii_uppercase().as_str() {
        "SELECT" | "READ" => Some(Privilege::Select),
        "INSERT" => Some(Privilege::Insert),
        "UPDATE" => Some(Privilege::Update),
        "DELETE" => Some(Privilege::Delete),
        "ALTER" => Some(Privilege::Alter),
        "INDEX" => Some(Privilege::Index),
        "REFERENCES" => Some(Privilege::References),
        "EXECUTE" => Some(Privilege::Execute),
        _ => None,
    }
}

/// Escape a password for `IDENTIFIED BY "..."` (the quotes are doubled).
fn quote_password(password: &str) -> String {
    format!("\"{}\"", password.replace('"', "\"\""))
}

/// A user account read from `dba_users` (or `all_users`, which lacks the status columns).
fn account_row(
    raw: &oracledb::Connection,
    user: Option<&str>,
    dba: bool,
) -> Result<Vec<UserAccount>> {
    let sql = if dba {
        match user {
            Some(_) => {
                "SELECT username, account_status, default_tablespace, profile, created \
                 FROM dba_users WHERE username = :1 ORDER BY username"
            }
            None => {
                "SELECT username, account_status, default_tablespace, profile, created \
                 FROM dba_users ORDER BY username"
            }
        }
    } else {
        match user {
            Some(_) => {
                "SELECT username, created FROM all_users WHERE username = :1 ORDER BY username"
            }
            None => "SELECT username, created FROM all_users ORDER BY username",
        }
    };
    let binds: Vec<String> = user
        .map(|value| vec![value.to_string()])
        .unwrap_or_default();
    let refs = crate::connection::bind_refs(&binds);
    let cursor = raw.query(sql, &refs).map_err(map_query_error)?;
    let mut accounts = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        let username = row
            .get::<Option<String>>(0)
            .ok()
            .flatten()
            .unwrap_or_default();
        if username.is_empty() {
            continue;
        }
        let status = if dba {
            row.get::<Option<String>>(1)
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            "OPEN".to_string()
        };
        let upper = status.to_ascii_uppercase();
        accounts.push(UserAccount {
            user: username,
            host: String::new(),
            plugin: String::new(),
            password_set: true,
            password_expired: upper.contains("EXPIRED"),
            password_lifetime: None,
            account_locked: upper.contains("LOCKED"),
            max_questions: 0,
            max_updates: 0,
            max_connections: 0,
            max_user_connections: 0,
            ssl_type: String::new(),
            ssl_cipher: String::new(),
            x509_issuer: String::new(),
            x509_subject: String::new(),
            is_super_user: false,
        });
    }
    Ok(accounts)
}

pub(crate) async fn list_users(connection: &OracleConnection) -> Result<Vec<UserAccount>> {
    connection
        .with_conn(|raw| match account_row(raw, None, true) {
            Ok(accounts) => Ok(accounts),
            Err(_) => account_row(raw, None, false),
        })
        .await
}

pub(crate) async fn user_details(connection: &OracleConnection, user: &str) -> Result<UserDetails> {
    let user = user.to_string();
    connection
        .with_conn(move |raw| {
            let account = match account_row(raw, Some(&user), true) {
                Ok(accounts) if !accounts.is_empty() => accounts.into_iter().next().unwrap(),
                _ => account_row(raw, Some(&user), false)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| Error::Query(format!("user {user} not found")))?,
            };

            let mut server_privileges = BTreeSet::new();
            for name in query_strings(
                raw,
                "SELECT privilege FROM dba_sys_privs WHERE grantee = :1",
                &user,
                "SELECT privilege FROM user_sys_privs",
            )? {
                if let Some(privilege) = server_privilege_from(&name) {
                    server_privileges.insert(privilege);
                }
            }

            let roles = query_role_edges(raw, "grantee", &user)?
                .into_iter()
                .map(|(granted, _grantee, admin)| RoleMembership {
                    role_user: granted,
                    role_host: String::new(),
                    member_user: user.clone(),
                    member_host: String::new(),
                    admin_option: admin,
                })
                .collect();
            let members = query_role_edges(raw, "granted_role", &user)?
                .into_iter()
                .map(|(_granted, grantee, admin)| RoleMembership {
                    role_user: user.clone(),
                    role_host: String::new(),
                    member_user: grantee,
                    member_host: String::new(),
                    admin_option: admin,
                })
                .collect();

            let grants = query_object_grants(raw, &user)?;

            Ok(UserDetails {
                account,
                server_privileges,
                grants,
                roles,
                members,
            })
        })
        .await
}

/// The string values of a privileged catalog query, falling back to the non-`DBA` view.
fn query_strings(
    raw: &oracledb::Connection,
    dba_sql: &str,
    user: &str,
    fallback_sql: &str,
) -> Result<Vec<String>> {
    let binds: Vec<String> = vec![user.to_string()];
    let refs = crate::connection::bind_refs(&binds);
    let cursor = match raw.query(dba_sql, &refs) {
        Ok(cursor) => cursor,
        Err(_) => raw.query(fallback_sql, &[]).map_err(map_query_error)?,
    };
    let mut values = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        if let Ok(Some(value)) = row.get::<Option<String>>(0) {
            values.push(value);
        }
    }
    Ok(values)
}

/// `(granted_role, grantee, admin_option)` rows from `dba_role_privs`, filtered on `column`.
fn query_role_edges(
    raw: &oracledb::Connection,
    column: &str,
    user: &str,
) -> Result<Vec<(String, String, bool)>> {
    let sql = format!(
        "SELECT granted_role, grantee, admin_option FROM dba_role_privs WHERE {column} = :1"
    );
    let binds: Vec<String> = vec![user.to_string()];
    let refs = crate::connection::bind_refs(&binds);
    let cursor = match raw.query(&sql, &refs) {
        Ok(cursor) => cursor,
        Err(_) => return Ok(Vec::new()),
    };
    let mut edges = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        let granted = row
            .get::<Option<String>>(0)
            .ok()
            .flatten()
            .unwrap_or_default();
        let grantee = row
            .get::<Option<String>>(1)
            .ok()
            .flatten()
            .unwrap_or_default();
        let admin = row
            .get::<Option<String>>(2)
            .ok()
            .flatten()
            .map(|value| value.eq_ignore_ascii_case("YES"))
            .unwrap_or(false);
        edges.push((granted, grantee, admin));
    }
    Ok(edges)
}

/// The object grants of a user, grouped by object.
fn query_object_grants(raw: &oracledb::Connection, user: &str) -> Result<Vec<ObjectGrant>> {
    let binds: Vec<String> = vec![user.to_string()];
    let refs = crate::connection::bind_refs(&binds);
    let cursor = match raw.query(
        "SELECT owner, table_name, privilege FROM dba_tab_privs WHERE grantee = :1 \
         ORDER BY owner, table_name",
        &refs,
    ) {
        Ok(cursor) => cursor,
        Err(_) => raw
            .query(
                "SELECT table_schema, table_name, privilege FROM user_tab_privs \
                 ORDER BY table_schema, table_name",
                &[],
            )
            .map_err(map_query_error)?,
    };
    let mut grants: Vec<ObjectGrant> = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        let owner = row
            .get::<Option<String>>(0)
            .ok()
            .flatten()
            .unwrap_or_default();
        let table = row
            .get::<Option<String>>(1)
            .ok()
            .flatten()
            .unwrap_or_default();
        let privilege_name = row
            .get::<Option<String>>(2)
            .ok()
            .flatten()
            .unwrap_or_default();
        let Some(privilege) = object_privilege_from(&privilege_name) else {
            continue;
        };
        let object = format!("{owner}.{table}");
        match grants.iter_mut().find(|grant| grant.name == object) {
            Some(existing) => {
                existing.privileges.insert(privilege);
            }
            None => {
                let mut grant = ObjectGrant::new(String::new(), object);
                grant.privileges.insert(privilege);
                grants.push(grant);
            }
        }
    }
    Ok(grants)
}

/// Build the statements an edit runs, grouped by what they change.
pub(crate) fn edit_groups(edit: &UserEdit) -> Vec<(UserEditSection, Vec<String>)> {
    let account = &edit.account;
    let user = quote_identifier(&account.user);
    let mut groups: Vec<(UserEditSection, Vec<String>)> = Vec::new();

    let mut account_statements = Vec::new();
    if edit.original.is_none() {
        let password = edit
            .password
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "change_me".to_string());
        account_statements.push(format!(
            "CREATE USER {user} IDENTIFIED BY {}",
            quote_password(&password)
        ));
    } else if let Some(password) = edit.password.clone().filter(|value| !value.is_empty()) {
        account_statements.push(format!(
            "ALTER USER {user} IDENTIFIED BY {}",
            quote_password(&password)
        ));
    }
    if let Some(original) = &edit.original
        && original.account.account_locked != account.account_locked
    {
        account_statements.push(format!(
            "ALTER USER {user} {}",
            if account.account_locked {
                "ACCOUNT LOCK"
            } else {
                "ACCOUNT UNLOCK"
            }
        ));
    }
    if !account_statements.is_empty() {
        groups.push((UserEditSection::Account, account_statements));
    }

    let privilege_statements = privilege_statements(edit);
    if !privilege_statements.is_empty() {
        groups.push((UserEditSection::ServerPrivileges, privilege_statements));
    }

    let grant_statements = grant_statements(edit);
    if !grant_statements.is_empty() {
        groups.push((UserEditSection::ObjectGrants, grant_statements));
    }

    let role_statements = role_statements(edit);
    if !role_statements.is_empty() {
        groups.push((UserEditSection::Roles, role_statements));
    }

    groups
}

fn privilege_statements(edit: &UserEdit) -> Vec<String> {
    let user = quote_identifier(&edit.account.user);
    let before: BTreeSet<Privilege> = edit
        .original
        .as_ref()
        .map(|details| details.server_privileges.clone())
        .unwrap_or_default();
    let mut statements = Vec::new();
    let added: Vec<&str> = edit
        .server_privileges
        .difference(&before)
        .filter_map(|privilege| server_privilege(*privilege))
        .collect();
    if !added.is_empty() {
        statements.push(format!("GRANT {} TO {user}", added.join(", ")));
    }
    let removed: Vec<&str> = before
        .difference(&edit.server_privileges)
        .filter_map(|privilege| server_privilege(*privilege))
        .collect();
    if !removed.is_empty() {
        statements.push(format!("REVOKE {} FROM {user}", removed.join(", ")));
    }
    statements
}

fn grant_statements(edit: &UserEdit) -> Vec<String> {
    let user = quote_identifier(&edit.account.user);
    let original: BTreeMap<&str, &BTreeSet<Privilege>> = edit
        .original
        .as_ref()
        .map(|details| {
            details
                .grants
                .iter()
                .map(|grant| (grant.name.as_str(), &grant.privileges))
                .collect()
        })
        .unwrap_or_default();
    let empty = BTreeSet::new();
    let mut statements = Vec::new();
    for grant in &edit.grants {
        let before = original.get(grant.name.as_str()).copied().unwrap_or(&empty);
        let target = match grant.name.split_once('.') {
            Some((schema, name)) => qualify(schema, name),
            None => quote_identifier(&grant.name),
        };
        let added: Vec<&str> = grant
            .privileges
            .difference(before)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !added.is_empty() {
            statements.push(format!("GRANT {} ON {target} TO {user}", added.join(", ")));
        }
        let removed: Vec<&str> = before
            .difference(&grant.privileges)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {target} FROM {user}",
                removed.join(", ")
            ));
        }
    }
    statements
}

fn role_statements(edit: &UserEdit) -> Vec<String> {
    let user = quote_identifier(&edit.account.user);
    let before: BTreeSet<&str> = edit
        .original
        .as_ref()
        .map(|details| {
            details
                .roles
                .iter()
                .map(|edge| edge.role_user.as_str())
                .collect()
        })
        .unwrap_or_default();
    let mut statements = Vec::new();
    for (role, _host, admin) in &edit.roles {
        let mut statement = format!("GRANT {} TO {user}", quote_identifier(role));
        if *admin {
            statement.push_str(" WITH ADMIN OPTION");
        }
        statements.push(statement);
    }
    for role in &before {
        if !edit.roles.iter().any(|(name, _, _)| name == role) {
            statements.push(format!("REVOKE {} FROM {user}", quote_identifier(role)));
        }
    }
    statements
}

pub(crate) fn edit_sql(edit: &UserEdit) -> String {
    edit_groups(edit)
        .into_iter()
        .flat_map(|(_, statements)| statements)
        .collect::<Vec<_>>()
        .join(";\n")
}

pub(crate) async fn save_user(connection: &OracleConnection, edit: &UserEdit) -> Result<()> {
    let groups = edit_groups(edit);
    connection
        .with_conn(move |raw| {
            for (_, statements) in &groups {
                for statement in statements {
                    crate::connection::run_text_public(raw, statement)?;
                }
            }
            Ok(())
        })
        .await
}

pub(crate) async fn drop_user(connection: &OracleConnection, user: &str) -> Result<()> {
    // `CASCADE` drops the schema's objects too; the UI asks for confirmation before calling this.
    let sql = format!("DROP USER {} CASCADE", quote_identifier(user));
    connection
        .with_conn(move |raw| crate::connection::run_text_public(raw, &sql))
        .await
}

pub(crate) async fn object_privilege_matrix(
    connection: &OracleConnection,
    _database: &str,
    name: &str,
) -> Result<Vec<ObjectPrivilegeRow>> {
    let (schema, bare) = connection.resolve(name);
    connection
        .with_conn(move |raw| {
            let binds: Vec<String> = vec![schema.clone(), bare.clone()];
            let refs = crate::connection::bind_refs(&binds);
            let cursor = match raw.query(
                "SELECT grantee, privilege FROM dba_tab_privs \
                 WHERE owner = :1 AND table_name = :2 ORDER BY grantee, privilege",
                &refs,
            ) {
                Ok(cursor) => cursor,
                Err(_) => return Ok(Vec::new()),
            };
            let mut result: Vec<ObjectPrivilegeRow> = Vec::new();
            for row in cursor {
                let row = row.map_err(map_query_error)?;
                let grantee = row
                    .get::<Option<String>>(0)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let privilege_name = row
                    .get::<Option<String>>(1)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let Some(privilege) = object_privilege_from(&privilege_name) else {
                    continue;
                };
                match result.iter_mut().find(|entry| entry.user == grantee) {
                    Some(entry) => {
                        entry.privileges.insert(privilege);
                    }
                    None => {
                        let mut entry = ObjectPrivilegeRow::new(grantee, String::new());
                        entry.privileges.insert(privilege);
                        result.push(entry);
                    }
                }
            }
            Ok(result)
        })
        .await
}

pub(crate) fn object_privileges_sql(
    _database: &str,
    name: &str,
    original: &[ObjectPrivilegeRow],
    rows: &[ObjectPrivilegeRow],
) -> String {
    let target = match name.split_once('.') {
        Some((schema, bare)) => qualify(schema, bare),
        None => quote_identifier(name),
    };
    let mut statements = Vec::new();
    for row in rows {
        let before = original
            .iter()
            .find(|entry| entry.user == row.user)
            .map(|entry| &entry.privileges)
            .cloned()
            .unwrap_or_default();
        let user = quote_identifier(&row.user);
        let added: Vec<&str> = row
            .privileges
            .difference(&before)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !added.is_empty() {
            statements.push(format!("GRANT {} ON {target} TO {user}", added.join(", ")));
        }
        let removed: Vec<&str> = before
            .difference(&row.privileges)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {target} FROM {user}",
                removed.join(", ")
            ));
        }
    }
    statements.join(";\n")
}

pub(crate) async fn set_object_privileges(
    connection: &OracleConnection,
    database: &str,
    name: &str,
    rows: &[ObjectPrivilegeRow],
) -> Result<()> {
    let original = object_privilege_matrix(connection, database, name).await?;
    let sql = object_privileges_sql(database, name, &original, rows);
    if sql.trim().is_empty() {
        return Ok(());
    }
    connection.run_text(&sql).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_privileges_both_ways() {
        assert_eq!(object_privilege(Privilege::Select), Some("SELECT"));
        assert_eq!(object_privilege_from("INSERT"), Some(Privilege::Insert));
        assert_eq!(
            server_privilege_from("CREATE USER"),
            Some(Privilege::CreateUser)
        );
    }

    #[test]
    fn builds_create_user_preview() {
        let edit = UserEdit {
            original: None,
            account: UserAccount {
                user: "APP".to_string(),
                ..Default::default()
            },
            password: Some("p\"w".to_string()),
            server_privileges: BTreeSet::from([Privilege::Create]),
            grants: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
        };
        let sql = edit_sql(&edit);
        assert!(sql.contains("CREATE USER \"APP\" IDENTIFIED BY \"p\"\"w\""));
        assert!(sql.contains("GRANT CREATE TABLE TO \"APP\""));
    }
}

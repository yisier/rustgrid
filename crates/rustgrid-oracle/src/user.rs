//! Oracle account (user/role) catalog access and `CREATE/ALTER/DROP USER` + `GRANT`/`REVOKE`
//! generation.
//!
//! Oracle has no `user@host` model: accounts are schema-wide, so `UserAccount::host` is always
//! empty. In Oracle a schema *is* a user, so the Users tab and the connection tree's schema list
//! intentionally overlap.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    Error, ObjectGrant, ObjectPrivilegeRow, PrivilegeCatalog, PrivilegeGroup, PrivilegeId,
    PrivilegeInfo, PrivilegePreset, Result, RoleMembership, UserAccount, UserDetails, UserEdit,
    UserEditSection,
};

use crate::connection::{OracleConnection, map_query_error};
use crate::helpers::{qualify, quote_identifier};

/// The Oracle system privileges surfaced by the account editor's 服务器权限 grid, as
/// `(privilege, i18n key)`.
const SYSTEM_PRIVILEGES: &[(&str, &str)] = &[
    ("CREATE SESSION", "user.priv.oracle.create_session"),
    ("SELECT ANY TABLE", "user.priv.oracle.select_any_table"),
    ("INSERT ANY TABLE", "user.priv.oracle.insert_any_table"),
    ("UPDATE ANY TABLE", "user.priv.oracle.update_any_table"),
    ("DELETE ANY TABLE", "user.priv.oracle.delete_any_table"),
    ("CREATE TABLE", "user.priv.oracle.create_table"),
    ("ALTER ANY TABLE", "user.priv.oracle.alter_any_table"),
    ("DROP ANY TABLE", "user.priv.oracle.drop_any_table"),
    ("CREATE ANY INDEX", "user.priv.oracle.create_any_index"),
    ("CREATE VIEW", "user.priv.oracle.create_view"),
    ("CREATE PROCEDURE", "user.priv.oracle.create_procedure"),
    (
        "ALTER ANY PROCEDURE",
        "user.priv.oracle.alter_any_procedure",
    ),
    (
        "EXECUTE ANY PROCEDURE",
        "user.priv.oracle.execute_any_procedure",
    ),
    ("CREATE USER", "user.priv.oracle.create_user"),
];

/// The Oracle object privileges surfaced by the 权限 grid, as `(privilege, i18n key, group)`.
const OBJECT_PRIVILEGES: &[(&str, &str, &str)] = &[
    ("SELECT", "user.priv.select", "dml"),
    ("INSERT", "user.priv.insert", "dml"),
    ("UPDATE", "user.priv.update", "dml"),
    ("DELETE", "user.priv.delete", "dml"),
    ("ALTER", "user.priv.alter", "ddl"),
    ("INDEX", "user.priv.index", "ddl"),
    ("REFERENCES", "user.priv.references", "ddl"),
    ("EXECUTE", "user.priv.execute", "routines"),
];

/// The Oracle privilege catalog: its system privileges (server), object privileges, groups and
/// presets.
pub(crate) fn privilege_catalog() -> PrivilegeCatalog {
    let groups = [
        ("system", "user.tab.server_privileges"),
        ("dml", "user.create.priv_group.dml"),
        ("ddl", "user.create.priv_group.ddl"),
        ("routines", "user.create.priv_group.routines"),
    ]
    .into_iter()
    .map(|(id, label_key)| PrivilegeGroup {
        id: id.to_string(),
        label_key: Some(label_key.to_string()),
        label: id.to_string(),
    })
    .collect();

    let mut privileges: Vec<PrivilegeInfo> = SYSTEM_PRIVILEGES
        .iter()
        .map(|(name, label_key)| PrivilegeInfo::server(*name, "system", Some(*label_key), name))
        .collect();
    privileges.extend(OBJECT_PRIVILEGES.iter().map(|(name, label_key, group)| {
        PrivilegeInfo::object(*name, group, Some(*label_key), name)
    }));

    PrivilegeCatalog {
        groups,
        privileges,
        server_presets: vec![
            preset("none", "user.create.template.none", &[]),
            preset(
                "connect",
                "user.create.template.read_only",
                &["CREATE SESSION"],
            ),
            preset(
                "admin",
                "user.create.template.admin",
                &SYSTEM_PRIVILEGES
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>(),
            ),
        ],
        object_presets: vec![
            preset("none", "user.create.db_template.none", &[]),
            preset(
                "read_only",
                "user.create.db_template.read_only",
                &["SELECT"],
            ),
            preset(
                "read_write",
                "user.create.db_template.read_write",
                &["SELECT", "INSERT", "UPDATE", "DELETE"],
            ),
            preset(
                "full",
                "user.create.db_template.full",
                &OBJECT_PRIVILEGES
                    .iter()
                    .map(|(name, _, _)| *name)
                    .collect::<Vec<_>>(),
            ),
        ],
        default_privileges: Vec::new(),
        deny_supported: false,
        securable_classes: Vec::new(),
    }
}

fn preset(id: &str, label_key: &str, names: &[&str]) -> PrivilegePreset {
    PrivilegePreset {
        id: id.to_string(),
        label_key: Some(label_key.to_string()),
        label: id.to_string(),
        privileges: names.iter().map(|name| PrivilegeId::new(*name)).collect(),
    }
}

/// Escape a password for `IDENTIFIED BY "..."` (the quotes are doubled).
fn quote_password(password: &str) -> String {
    format!("\"{}\"", password.replace('"', "\"\""))
}

/// `QUOTA <size> ON <tablespace>`, or `None` when either part is missing. The size is passed
/// through as typed (`UNLIMITED`, `100M`, ...), which Oracle parses.
fn quota_clause(tablespace: &str, quota: &str) -> Option<String> {
    let tablespace = tablespace.trim();
    let quota = quota.trim();
    if tablespace.is_empty() || quota.is_empty() {
        return None;
    }
    Some(format!(
        "QUOTA {} ON {}",
        quota,
        quote_identifier(tablespace)
    ))
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
        // `all_users` (the fallback) has no tablespace/profile columns.
        let default_tablespace = if dba {
            row.get::<Option<String>>(2)
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };
        let profile = if dba {
            row.get::<Option<String>>(3)
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };
        accounts.push(UserAccount {
            user: username,
            host: String::new(),
            plugin: String::new(),
            password_set: true,
            password_expired: upper.contains("EXPIRED"),
            password_lifetime: None,
            password_valid_until: None,
            account_locked: upper.contains("LOCKED"),
            max_questions: 0,
            max_updates: 0,
            max_connections: 0,
            max_user_connections: 0,
            ssl_type: String::new(),
            ssl_cipher: String::new(),
            x509_issuer: String::new(),
            x509_subject: String::new(),
            default_tablespace,
            profile,
            // Loaded separately by `user_details` (it needs the tablespace name).
            tablespace_quota: String::new(),
            is_super_user: false,
            ..UserAccount::default()
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
            let mut account = match account_row(raw, Some(&user), true) {
                Ok(accounts) if !accounts.is_empty() => accounts.into_iter().next().unwrap(),
                _ => account_row(raw, Some(&user), false)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| Error::Query(format!("user {user} not found")))?,
            };
            if !account.default_tablespace.is_empty() {
                account.tablespace_quota =
                    query_tablespace_quota(raw, &user, &account.default_tablespace);
            }

            let mut server_privileges: BTreeSet<PrivilegeId> = BTreeSet::new();
            for name in query_strings(
                raw,
                "SELECT privilege FROM dba_sys_privs WHERE grantee = :1",
                &user,
                "SELECT privilege FROM user_sys_privs",
            )? {
                server_privileges.insert(PrivilegeId::new(name.trim().to_ascii_uppercase()));
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

            let grants = query_object_grants(raw, &container_name(raw), &user)?;

            Ok(UserDetails {
                account,
                server_privileges,
                denied_server_privileges: BTreeSet::new(),
                grants,
                default_privileges: Vec::new(),
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

/// The user's quota on one tablespace: `UNLIMITED` when `dba_ts_quotas.max_bytes` is negative,
/// else the byte count. Empty when there is no quota row.
fn query_tablespace_quota(raw: &oracledb::Connection, user: &str, tablespace: &str) -> String {
    let binds: Vec<String> = vec![user.to_string(), tablespace.to_string()];
    let refs = crate::connection::bind_refs(&binds);
    let cursor = match raw.query(
        "SELECT max_bytes FROM dba_ts_quotas WHERE username = :1 AND tablespace_name = :2",
        &refs,
    ) {
        Ok(cursor) => cursor,
        Err(_) => return String::new(),
    };
    for row in cursor {
        let row = match row {
            Ok(row) => row,
            Err(_) => return String::new(),
        };
        if let Ok(Some(bytes)) = row.get::<Option<i64>>(0) {
            return if bytes < 0 {
                "UNLIMITED".to_string()
            } else {
                bytes.to_string()
            };
        }
    }
    String::new()
}

/// The current container/PDB name, used as the object grants' database so they line up with the
/// connection tree's single database row.
fn container_name(raw: &oracledb::Connection) -> String {
    raw.query_row(
        "SELECT COALESCE(SYS_CONTEXT('USERENV', 'CON_NAME'), SYS_CONTEXT('USERENV', 'DB_NAME')) \
         FROM dual",
        &[],
    )
    .ok()
    .and_then(|row| row.get::<Option<String>>(0).ok().flatten())
    .filter(|value| !value.is_empty())
    .unwrap_or_default()
}

/// The object grants of a user, grouped by object. `database` is the current container/PDB, the
/// schema is the object's owner (Oracle's `OWNER.TABLE`).
fn query_object_grants(
    raw: &oracledb::Connection,
    database: &str,
    user: &str,
) -> Result<Vec<ObjectGrant>> {
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
        let privilege = PrivilegeId::new(privilege_name.trim().to_ascii_uppercase());
        if privilege.as_str().is_empty() {
            continue;
        }
        match grants
            .iter_mut()
            .find(|grant| grant.schema == owner && grant.name == table)
        {
            Some(existing) => {
                existing.privileges.insert(privilege);
            }
            None => {
                let mut grant = ObjectGrant {
                    database: database.to_string(),
                    schema: owner,
                    name: table,
                    privileges: BTreeSet::new(),
                };
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
        let mut sql = format!(
            "CREATE USER {user} IDENTIFIED BY {}",
            quote_password(&password)
        );
        if !account.default_tablespace.is_empty() {
            sql.push_str(&format!(
                " DEFAULT TABLESPACE {}",
                quote_identifier(&account.default_tablespace)
            ));
        }
        if !account.profile.is_empty() {
            sql.push_str(&format!(" PROFILE {}", quote_identifier(&account.profile)));
        }
        if let Some(quota) = quota_clause(&account.default_tablespace, &account.tablespace_quota) {
            sql.push(' ');
            sql.push_str(&quota);
        }
        account_statements.push(sql);
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
    if let Some(original) = &edit.original {
        if original.account.default_tablespace != account.default_tablespace
            && !account.default_tablespace.is_empty()
        {
            account_statements.push(format!(
                "ALTER USER {user} DEFAULT TABLESPACE {}",
                quote_identifier(&account.default_tablespace)
            ));
        }
        if original.account.profile != account.profile && !account.profile.is_empty() {
            account_statements.push(format!(
                "ALTER USER {user} PROFILE {}",
                quote_identifier(&account.profile)
            ));
        }
        let quota_changed = original.account.tablespace_quota != account.tablespace_quota
            || original.account.default_tablespace != account.default_tablespace;
        if quota_changed
            && let Some(quota) =
                quota_clause(&account.default_tablespace, &account.tablespace_quota)
        {
            account_statements.push(format!("ALTER USER {user} {quota}"));
        }
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
    let before: BTreeSet<PrivilegeId> = edit
        .original
        .as_ref()
        .map(|details| details.server_privileges.clone())
        .unwrap_or_default();
    let mut statements = Vec::new();
    let added: Vec<String> = edit
        .server_privileges
        .difference(&before)
        .map(|privilege| privilege.as_str().to_string())
        .collect();
    if !added.is_empty() {
        statements.push(format!("GRANT {} TO {user}", added.join(", ")));
    }
    let removed: Vec<String> = before
        .difference(&edit.server_privileges)
        .map(|privilege| privilege.as_str().to_string())
        .collect();
    if !removed.is_empty() {
        statements.push(format!("REVOKE {} FROM {user}", removed.join(", ")));
    }
    statements
}

fn grant_statements(edit: &UserEdit) -> Vec<String> {
    let user = quote_identifier(&edit.account.user);
    let original: BTreeMap<String, &BTreeSet<PrivilegeId>> = edit
        .original
        .as_ref()
        .map(|details| {
            details
                .grants
                .iter()
                .map(|grant| (grant.object_name(), &grant.privileges))
                .collect()
        })
        .unwrap_or_default();
    let empty = BTreeSet::new();
    let mut statements = Vec::new();
    for grant in &edit.grants {
        let before = original
            .get(&grant.object_name())
            .copied()
            .unwrap_or(&empty);
        let target = if grant.schema.is_empty() {
            match grant.name.split_once('.') {
                Some((schema, name)) => qualify(schema, name),
                None => quote_identifier(&grant.name),
            }
        } else {
            qualify(&grant.schema, &grant.name)
        };
        let added: Vec<String> = grant
            .privileges
            .difference(before)
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        if !added.is_empty() {
            statements.push(format!("GRANT {} ON {target} TO {user}", added.join(", ")));
        }
        let removed: Vec<String> = before
            .difference(&grant.privileges)
            .map(|privilege| privilege.as_str().to_string())
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
                let privilege = PrivilegeId::new(privilege_name.trim().to_ascii_uppercase());
                if privilege.as_str().is_empty() {
                    continue;
                }
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
        let added: Vec<String> = row
            .privileges
            .difference(&before)
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        if !added.is_empty() {
            statements.push(format!("GRANT {} ON {target} TO {user}", added.join(", ")));
        }
        let removed: Vec<String> = before
            .difference(&row.privileges)
            .map(|privilege| privilege.as_str().to_string())
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
    fn the_catalog_lists_system_and_object_privileges() {
        let catalog = privilege_catalog();
        assert!(catalog.is_server(&PrivilegeId::new("CREATE SESSION")));
        assert!(catalog.is_object(&PrivilegeId::new("SELECT")));
        assert!(!catalog.is_object(&PrivilegeId::new("CREATE SESSION")));
        assert!(catalog.is_server(&PrivilegeId::new("CREATE USER")));
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
            server_privileges: BTreeSet::from([PrivilegeId::new("CREATE TABLE")]),
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
        };
        let sql = edit_sql(&edit);
        assert!(sql.contains("CREATE USER \"APP\" IDENTIFIED BY \"p\"\"w\""));
        assert!(sql.contains("GRANT CREATE TABLE TO \"APP\""));
    }

    #[test]
    fn create_statement_carries_tablespace_and_profile() {
        let edit = UserEdit {
            original: None,
            account: UserAccount {
                user: "APP".to_string(),
                default_tablespace: "USERS".to_string(),
                profile: "DEFAULT".to_string(),
                tablespace_quota: "UNLIMITED".to_string(),
                ..Default::default()
            },
            password: Some("pw".to_string()),
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
        };
        let sql = edit_sql(&edit);
        assert!(sql.contains(
            "CREATE USER \"APP\" IDENTIFIED BY \"pw\" \
             DEFAULT TABLESPACE \"USERS\" PROFILE \"DEFAULT\" QUOTA UNLIMITED ON \"USERS\""
        ));
    }
}

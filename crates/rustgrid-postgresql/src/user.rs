//! PostgreSQL role (account) catalog access and `CREATE/ALTER/DROP ROLE` generation.
//!
//! PostgreSQL has no `user@host` model: roles are server-wide. `UserAccount::host` is therefore
//! always empty. Object privileges are read from the ACL (`aclexplode`) rather than
//! `information_schema`, which only exposes grants related to the current user.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    Error, ObjectGrant, ObjectPrivilegeRow, Privilege, Result, RoleMembership, UserAccount,
    UserDetails, UserEdit, UserEditSection,
};
use sqlx::Row;

use crate::connection::{PostgresConnection, map_query_error};
use crate::helpers::{qualify, quote_identifier, quote_literal, split_qualified};

/// Map a core privilege to the PostgreSQL object-level keyword, when it has an equivalent.
fn object_privilege(privilege: Privilege) -> Option<&'static str> {
    match privilege {
        Privilege::Select => Some("SELECT"),
        Privilege::Insert => Some("INSERT"),
        Privilege::Update => Some("UPDATE"),
        Privilege::Delete => Some("DELETE"),
        Privilege::References => Some("REFERENCES"),
        Privilege::Trigger => Some("TRIGGER"),
        Privilege::Execute => Some("EXECUTE"),
        Privilege::Create => Some("CREATE"),
        Privilege::CreateTemporaryTables => Some("TEMPORARY"),
        Privilege::CreateView => Some("CREATE"),
        _ => None,
    }
}

/// Resolve a `schema.name` object name, defaulting to `public`.
fn object_parts(name: &str) -> (String, String) {
    split_qualified(name)
        .map(|(schema, name)| (schema.to_string(), name.to_string()))
        .unwrap_or_else(|| ("public".to_string(), name.to_string()))
}

fn account_from_row(row: &sqlx::postgres::PgRow) -> UserAccount {
    let can_login: bool = row.try_get("rolcanlogin").unwrap_or(true);
    UserAccount {
        user: row.try_get("rolname").unwrap_or_default(),
        host: String::new(),
        plugin: String::new(),
        password_set: row.try_get("has_password").unwrap_or(false),
        password_expired: false,
        password_lifetime: None,
        account_locked: !can_login,
        max_questions: 0,
        max_updates: 0,
        max_connections: row
            .try_get::<i32, _>("rolconnlimit")
            .ok()
            .filter(|value| *value >= 0)
            .map(|value| value as u64)
            .unwrap_or(0),
        max_user_connections: 0,
        ssl_type: String::new(),
        ssl_cipher: String::new(),
        x509_issuer: String::new(),
        x509_subject: String::new(),
        is_super_user: row.try_get("rolsuper").unwrap_or(false),
    }
}

const ACCOUNT_COLUMNS: &str = "rolname, rolsuper, rolcreaterole, rolcreatedb, rolcanlogin, \
     rolreplication, rolbypassrls, rolconnlimit, rolvaliduntil, \
     rolpassword IS NOT NULL AS has_password";

pub(crate) async fn list_users(connection: &PostgresConnection) -> Result<Vec<UserAccount>> {
    let pool = connection
        .pool_for(connection.default_database_name())
        .await?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM pg_roles \
         WHERE rolname NOT LIKE 'pg\\_%' ORDER BY rolname"
    )))
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    Ok(rows.iter().map(account_from_row).collect())
}

pub(crate) async fn user_details(
    connection: &PostgresConnection,
    user: &str,
) -> Result<UserDetails> {
    let pool = connection
        .pool_for(connection.default_database_name())
        .await?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ACCOUNT_COLUMNS} FROM pg_roles WHERE rolname = $1"
    )))
    .bind(user)
    .fetch_optional(&pool)
    .await
    .map_err(map_query_error)?
    .ok_or_else(|| Error::Query(format!("role {user} not found")))?;
    let account = account_from_row(&row);

    let mut server_privileges = BTreeSet::new();
    if row.try_get::<bool, _>("rolsuper").unwrap_or(false) {
        server_privileges.insert(Privilege::Super);
    }
    if row.try_get::<bool, _>("rolcreaterole").unwrap_or(false) {
        server_privileges.insert(Privilege::CreateUser);
    }
    if row.try_get::<bool, _>("rolcreatedb").unwrap_or(false) {
        server_privileges.insert(Privilege::Create);
    }
    if row.try_get::<bool, _>("rolreplication").unwrap_or(false) {
        server_privileges.insert(Privilege::ReplicationSlave);
    }

    let roles = role_edges(&pool, "am.member", user).await?;
    let members = role_edges(&pool, "am.roleid", user).await?;
    let grants = object_grants(&pool, user).await?;

    Ok(UserDetails {
        account,
        server_privileges,
        grants,
        roles,
        members,
    })
}

/// The role edges where `column` (the member or role side) is `user`.
async fn role_edges(pool: &sqlx::PgPool, column: &str, user: &str) -> Result<Vec<RoleMembership>> {
    let sql = format!(
        "SELECT role.rolname, member.rolname, am.admin_option \
         FROM pg_auth_members am \
         JOIN pg_roles role ON role.oid = am.roleid \
         JOIN pg_roles member ON member.oid = am.member \
         JOIN pg_roles target ON target.oid = {column} \
         WHERE target.rolname = $1"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(user)
        .fetch_all(pool)
        .await
        .map_err(map_query_error)?;
    Ok(rows
        .iter()
        .map(|row| RoleMembership {
            role_user: row.try_get(0).unwrap_or_default(),
            role_host: String::new(),
            member_user: row.try_get(1).unwrap_or_default(),
            member_host: String::new(),
            admin_option: row.try_get(2).unwrap_or(false),
        })
        .collect())
}

/// Object grants visible in the relation ACLs for `user`, grouped by object.
async fn object_grants(pool: &sqlx::PgPool, user: &str) -> Result<Vec<ObjectGrant>> {
    let rows = sqlx::query(
        "SELECT n.nspname, c.relname, a.privilege_type \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         CROSS JOIN LATERAL aclexplode(c.relacl) AS a \
         JOIN pg_roles r ON r.oid = a.grantee \
         WHERE r.rolname = $1 AND c.relkind IN ('r','p','v','m','f') \
         ORDER BY n.nspname, c.relname",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .map_err(map_query_error)?;

    let mut grants: Vec<ObjectGrant> = Vec::new();
    for row in &rows {
        let schema: String = row.try_get(0).unwrap_or_default();
        let name: String = row.try_get(1).unwrap_or_default();
        let privilege_type: String = row.try_get(2).unwrap_or_default();
        let object = format!("{schema}.{name}");
        let Some(privilege) = Privilege::from_sql_name(&privilege_type) else {
            continue;
        };
        if let Some(existing) = grants.iter_mut().find(|grant| grant.name == object) {
            existing.privileges.insert(privilege);
        } else {
            let mut grant = ObjectGrant::new(String::new(), object);
            grant.privileges.insert(privilege);
            grants.push(grant);
        }
    }
    Ok(grants)
}

/// Build the statements an edit runs, grouped by what they change.
pub(crate) fn edit_groups(edit: &UserEdit) -> Vec<(UserEditSection, Vec<String>)> {
    let account = &edit.account;
    let mut groups: Vec<(UserEditSection, Vec<String>)> = Vec::new();

    let mut account_statements = Vec::new();
    let attributes = role_attributes(edit);
    if edit.original.is_none() {
        account_statements.push(format!(
            "CREATE ROLE {} {attributes}",
            quote_identifier(&account.user)
        ));
    } else {
        if let Some(original) = &edit.original
            && original.account.user != account.user
        {
            account_statements.push(format!(
                "ALTER ROLE {} RENAME TO {}",
                quote_identifier(&original.account.user),
                quote_identifier(&account.user)
            ));
        }
        account_statements.push(format!(
            "ALTER ROLE {} {attributes}",
            quote_identifier(&account.user)
        ));
    }
    if !account_statements.is_empty() {
        groups.push((UserEditSection::Account, account_statements));
    }

    if !edit.grants.is_empty() || edit.original.as_ref().is_some_and(|o| !o.grants.is_empty()) {
        let statements = grant_statements(edit);
        if !statements.is_empty() {
            groups.push((UserEditSection::ObjectGrants, statements));
        }
    }

    if !edit.roles.is_empty() || !edit.members.is_empty() {
        let statements = role_statements(edit);
        if !statements.is_empty() {
            groups.push((UserEditSection::Roles, statements));
        }
    }

    groups
}

fn role_attributes(edit: &UserEdit) -> String {
    let account = &edit.account;
    let mut parts = Vec::new();
    parts.push(
        if account.account_locked {
            "NOLOGIN"
        } else {
            "LOGIN"
        }
        .to_string(),
    );
    parts.push(
        if account.is_super_user {
            "SUPERUSER"
        } else {
            "NOSUPERUSER"
        }
        .to_string(),
    );
    parts.push(
        if edit.server_privileges.contains(&Privilege::CreateUser) {
            "CREATEROLE"
        } else {
            "NOCREATEROLE"
        }
        .to_string(),
    );
    parts.push(
        if edit.server_privileges.contains(&Privilege::Create) {
            "CREATEDB"
        } else {
            "NOCREATEDB"
        }
        .to_string(),
    );
    parts.push(
        if edit
            .server_privileges
            .contains(&Privilege::ReplicationSlave)
        {
            "REPLICATION"
        } else {
            "NOREPLICATION"
        }
        .to_string(),
    );
    if account.max_connections > 0 {
        parts.push(format!("CONNECTION LIMIT {}", account.max_connections));
    }
    if let Some(password) = &edit.password {
        parts.push(format!("PASSWORD {}", quote_literal(password)));
    }
    parts.join(" ")
}

/// The GRANT/REVOKE statements for object grants that changed.
fn grant_statements(edit: &UserEdit) -> Vec<String> {
    let user = quote_identifier(&edit.account.user);
    let mut statements = Vec::new();
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
    for grant in &edit.grants {
        let before = original.get(grant.name.as_str()).copied().unwrap_or(&empty);
        let (schema, name) = object_parts(&grant.name);
        let qualified = qualify(&schema, &name);
        let added: Vec<&str> = grant
            .privileges
            .difference(before)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !added.is_empty() {
            statements.push(format!(
                "GRANT {} ON TABLE {qualified} TO {user}",
                added.join(", ")
            ));
        }
        let removed: Vec<&str> = before
            .difference(&grant.privileges)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON TABLE {qualified} FROM {user}",
                removed.join(", ")
            ));
        }
    }
    statements
}

fn role_statements(edit: &UserEdit) -> Vec<String> {
    let user = quote_identifier(&edit.account.user);
    let mut statements = Vec::new();
    for (role, _host, admin) in &edit.roles {
        let mut statement = format!("GRANT {} TO {user}", quote_identifier(role));
        if *admin {
            statement.push_str(" WITH ADMIN OPTION");
        }
        statements.push(statement);
    }
    for (member, _host, admin) in &edit.members {
        let mut statement = format!("GRANT {user} TO {}", quote_identifier(member));
        if *admin {
            statement.push_str(" WITH ADMIN OPTION");
        }
        statements.push(statement);
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

pub(crate) async fn save_user(connection: &PostgresConnection, edit: &UserEdit) -> Result<()> {
    let database = connection.default_database_name();
    for (_, statements) in edit_groups(edit) {
        for statement in statements {
            connection.run_text(Some(database), &statement).await?;
        }
    }
    Ok(())
}

pub(crate) async fn drop_user(connection: &PostgresConnection, user: &str) -> Result<()> {
    let database = connection.default_database_name();
    let sql = format!("DROP ROLE {}", quote_identifier(user));
    connection.run_text(Some(database), &sql).await.map(|_| ())
}

pub(crate) async fn rename_user(
    connection: &PostgresConnection,
    user: &str,
    new_user: &str,
) -> Result<()> {
    let database = connection.default_database_name();
    let sql = format!(
        "ALTER ROLE {} RENAME TO {}",
        quote_identifier(user),
        quote_identifier(new_user)
    );
    connection.run_text(Some(database), &sql).await.map(|_| ())
}

pub(crate) async fn object_privilege_matrix(
    connection: &PostgresConnection,
    database: &str,
    name: &str,
) -> Result<Vec<ObjectPrivilegeRow>> {
    let (schema, bare) = connection
        .resolve_object(database, name)
        .await
        .unwrap_or_else(|_| object_parts(name));
    let pool = connection.pool_for(database).await?;
    let rows = sqlx::query(
        "SELECT r.rolname, a.privilege_type \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         CROSS JOIN LATERAL aclexplode(c.relacl) AS a \
         JOIN pg_roles r ON r.oid = a.grantee \
         WHERE n.nspname = $1 AND c.relname = $2",
    )
    .bind(&schema)
    .bind(&bare)
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;

    let mut result: Vec<ObjectPrivilegeRow> = Vec::new();
    for row in &rows {
        let role: String = row.try_get(0).unwrap_or_default();
        let privilege_type: String = row.try_get(1).unwrap_or_default();
        let Some(privilege) = Privilege::from_sql_name(&privilege_type) else {
            continue;
        };
        match result.iter_mut().find(|entry| entry.user == role) {
            Some(entry) => {
                entry.privileges.insert(privilege);
            }
            None => {
                let mut entry = ObjectPrivilegeRow::new(role, String::new());
                entry.privileges.insert(privilege);
                result.push(entry);
            }
        }
    }
    Ok(result)
}

pub(crate) fn object_privileges_sql(
    _database: &str,
    name: &str,
    original: &[ObjectPrivilegeRow],
    rows: &[ObjectPrivilegeRow],
) -> String {
    let (schema, bare) = object_parts(name);
    let qualified = qualify(&schema, &bare);
    let mut statements = Vec::new();
    for row in rows {
        let before = original
            .iter()
            .find(|entry| entry.user == row.user)
            .map(|entry| &entry.privileges)
            .cloned()
            .unwrap_or_default();
        let role = quote_identifier(&row.user);
        let added: Vec<&str> = row
            .privileges
            .difference(&before)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !added.is_empty() {
            statements.push(format!(
                "GRANT {} ON TABLE {qualified} TO {role}",
                added.join(", ")
            ));
        }
        let removed: Vec<&str> = before
            .difference(&row.privileges)
            .filter_map(|privilege| object_privilege(*privilege))
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON TABLE {qualified} FROM {role}",
                removed.join(", ")
            ));
        }
    }
    statements.join(";\n")
}

pub(crate) async fn set_object_privileges(
    connection: &PostgresConnection,
    database: &str,
    name: &str,
    rows: &[ObjectPrivilegeRow],
) -> Result<()> {
    let original = object_privilege_matrix(connection, database, name).await?;
    let sql = object_privileges_sql(database, name, &original, rows);
    if sql.trim().is_empty() {
        return Ok(());
    }
    connection.run_text(Some(database), &sql).await.map(|_| ())
}

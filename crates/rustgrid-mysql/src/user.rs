//! MySQL account (user/role) and privilege administration, backing the Users main tab and the
//! user editor.
//!
//! Accounts and their attributes live in `mysql.user`; the granted privileges are read from
//! `information_schema.*_PRIVILEGES` views (which report the effective grants as keywords) and
//! role memberships from `mysql.role_edges`. Writes are replayed as a script of `CREATE USER` /
//! `ALTER USER` / `GRANT` / `REVOKE` statements through the text protocol.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    ObjectGrant, ObjectPrivilegeRow, Privilege, Result, RoleMembership, UserAccount, UserDetails,
    UserEdit, UserEditSection,
};
use sqlx::mysql::MySqlRow;
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use crate::connection::map_query_error;

/// The `mysql.user` columns the account editor reads. `plugin`, `password_expired`, the resource
/// limits and the TLS columns are the ones the 常规/高级 tabs edit.
const ACCOUNT_QUERY: &str = "SELECT User, Host, plugin, authentication_string, password_expired, \
     password_lifetime, account_locked, max_questions, max_updates, max_connections, \
     max_user_connections, ssl_type, ssl_cipher, x509_issuer, x509_subject, super_priv \
     FROM mysql.user";

/// Every account on the server, ordered by user then host.
pub(crate) async fn list_users(pool: &MySqlPool) -> Result<Vec<UserAccount>> {
    let sql = format!("{ACCOUNT_QUERY} ORDER BY User, Host");
    let rows = sqlx::query(AssertSqlSafe(sql))
        .fetch_all(pool)
        .await
        .map_err(map_query_error)?;
    Ok(rows.iter().map(read_account).collect())
}

/// One account's attributes plus its granted server privileges, role edges and object grants.
pub(crate) async fn user_details(pool: &MySqlPool, user: &str, host: &str) -> Result<UserDetails> {
    let sql = format!("{ACCOUNT_QUERY} WHERE User = ? AND Host = ?");
    let row = sqlx::query(AssertSqlSafe(sql))
        .bind(user)
        .bind(host)
        .fetch_optional(pool)
        .await
        .map_err(map_query_error)?;
    let account = row
        .as_ref()
        .map(read_account)
        .unwrap_or_else(|| UserAccount {
            user: user.to_string(),
            host: host.to_string(),
            ..UserAccount::default()
        });

    let grantee = quote_grantee(user, host);
    let server_privileges = fetch_server_privileges(pool, &grantee).await?;
    let grants = fetch_object_grants(pool, &grantee).await?;
    let (roles, members) = fetch_role_edges(pool, user, host).await?;

    Ok(UserDetails {
        account,
        server_privileges,
        grants,
        roles,
        members,
    })
}

/// Drop one account.
pub(crate) async fn drop_user(pool: &MySqlPool, user: &str, host: &str) -> Result<()> {
    let sql = format!("DROP USER {}", quote_account(user, host));
    sqlx::raw_sql(AssertSqlSafe(sql))
        .execute(pool)
        .await
        .map_err(map_query_error)?;
    Ok(())
}

/// Rename one account, keeping its grants (`RENAME USER` rewrites the ACL rows in place).
pub(crate) async fn rename_user(
    pool: &MySqlPool,
    user: &str,
    host: &str,
    new_user: &str,
    new_host: &str,
) -> Result<()> {
    let sql = format!(
        "RENAME USER {} TO {}",
        quote_account(user, host),
        quote_account(new_user, new_host)
    );
    sqlx::raw_sql(AssertSqlSafe(sql))
        .execute(pool)
        .await
        .map_err(map_query_error)?;
    Ok(())
}

/// The SQL script [`save_user`] replays, for the editor's SQL preview.
pub(crate) fn edit_sql(edit: &UserEdit) -> String {
    let statements = edit_statements(edit);
    if statements.is_empty() {
        return String::new();
    }
    let mut script = statements.join(";\n");
    script.push(';');
    script
}

/// Create or alter the account and replace its privileges and role memberships with the edit's.
pub(crate) async fn save_user(pool: &MySqlPool, edit: &UserEdit) -> Result<()> {
    for statement in edit_statements(edit) {
        sqlx::raw_sql(AssertSqlSafe(statement))
            .execute(pool)
            .await
            .map_err(map_query_error)?;
    }
    Ok(())
}

/// The authentication plugins the account editor offers. The server may support a subset; an
/// unsupported choice fails at save time with the server's own error.
pub(crate) fn authentication_plugins() -> Vec<&'static str> {
    vec![
        "caching_sha2_password",
        "mysql_native_password",
        "sha256_password",
        "mysql_no_login",
        "auth_socket",
    ]
}

/// The values `mysql.user.ssl_type` can take, in the order the SSL type dropdown shows them.
pub(crate) fn ssl_types() -> Vec<&'static str> {
    vec!["", "ANY", "X509", "SPECIFIED"]
}

// ----- Reading -------------------------------------------------------------------------------

fn read_account(row: &MySqlRow) -> UserAccount {
    UserAccount {
        user: text(row, "User"),
        host: text(row, "Host"),
        plugin: text(row, "plugin"),
        password_set: !text(row, "authentication_string").is_empty(),
        password_expired: flag(row, "password_expired"),
        password_lifetime: int(row, "password_lifetime").map(|value| value.max(0) as u32),
        account_locked: flag(row, "account_locked"),
        max_questions: uint(row, "max_questions"),
        max_updates: uint(row, "max_updates"),
        max_connections: uint(row, "max_connections"),
        max_user_connections: uint(row, "max_user_connections"),
        ssl_type: text(row, "ssl_type"),
        ssl_cipher: text(row, "ssl_cipher"),
        x509_issuer: text(row, "x509_issuer"),
        x509_subject: text(row, "x509_subject"),
        is_super_user: flag(row, "super_priv"),
    }
}

async fn fetch_server_privileges(pool: &MySqlPool, grantee: &str) -> Result<BTreeSet<Privilege>> {
    // The view can be unavailable on some servers; report no granted privileges rather than
    // failing the account load.
    let rows = match sqlx::query(
        "SELECT PRIVILEGE_TYPE FROM information_schema.USER_PRIVILEGES WHERE GRANTEE = ?",
    )
    .bind(grantee)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return Ok(BTreeSet::new()),
    };
    Ok(rows
        .iter()
        .filter_map(|row| Privilege::from_sql_name(&text(row, "PRIVILEGE_TYPE")))
        .collect())
}

/// The object-level grants of an account, merged into one row per `(database, object)`.
async fn fetch_object_grants(pool: &MySqlPool, grantee: &str) -> Result<Vec<ObjectGrant>> {
    let mut merged: BTreeMap<(String, String), BTreeSet<Privilege>> = BTreeMap::new();
    for sql in [
        "SELECT TABLE_SCHEMA, '' AS object_name, PRIVILEGE_TYPE \
         FROM information_schema.SCHEMA_PRIVILEGES WHERE GRANTEE = ?",
        "SELECT TABLE_SCHEMA, TABLE_NAME AS object_name, PRIVILEGE_TYPE \
         FROM information_schema.TABLE_PRIVILEGES WHERE GRANTEE = ?",
        "SELECT ROUTINE_SCHEMA AS TABLE_SCHEMA, ROUTINE_NAME AS object_name, PRIVILEGE_TYPE \
         FROM information_schema.ROUTINE_PRIVILEGES WHERE GRANTEE = ?",
    ] {
        // A view may be missing on some servers (e.g. `ROUTINE_PRIVILEGES` on MariaDB builds);
        // a missing scope simply contributes no grants rather than failing the whole load.
        let rows = match sqlx::query(sql).bind(grantee).fetch_all(pool).await {
            Ok(rows) => rows,
            Err(_) => continue,
        };
        for row in &rows {
            let Some(privilege) = Privilege::from_sql_name(&text(row, "PRIVILEGE_TYPE")) else {
                continue;
            };
            merged
                .entry((text(row, "TABLE_SCHEMA"), text(row, "object_name")))
                .or_default()
                .insert(privilege);
        }
    }

    Ok(merged
        .into_iter()
        .map(|((database, name), privileges)| ObjectGrant {
            database,
            name,
            privileges,
        })
        .collect())
}

/// The role edges touching one account: the roles it is a member of, and the members of it (when
/// it is a role). MySQL 5.7 has no `mysql.role_edges`, so a missing table yields no edges.
async fn fetch_role_edges(
    pool: &MySqlPool,
    user: &str,
    host: &str,
) -> Result<(Vec<RoleMembership>, Vec<RoleMembership>)> {
    let rows = match sqlx::query(
        "SELECT FROM_USER, FROM_HOST, TO_USER, TO_HOST, WITH_ADMIN_OPTION FROM mysql.role_edges",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return Ok((Vec::new(), Vec::new())),
    };

    let edges: Vec<RoleMembership> = rows
        .iter()
        .map(|row| RoleMembership {
            role_user: text(row, "FROM_USER"),
            role_host: text(row, "FROM_HOST"),
            member_user: text(row, "TO_USER"),
            member_host: text(row, "TO_HOST"),
            admin_option: flag(row, "WITH_ADMIN_OPTION"),
        })
        .collect();

    let roles = edges
        .iter()
        .filter(|edge| edge.member_user == user && edge.member_host == host)
        .cloned()
        .collect();
    let members = edges
        .iter()
        .filter(|edge| edge.role_user == user && edge.role_host == host)
        .cloned()
        .collect();
    Ok((roles, members))
}

// ----- Writing -------------------------------------------------------------------------------

/// The ordered statements that turn the loaded state into the edit's state, grouped by what they
/// change so the editor's confirmation dialog can annotate each group.
pub(crate) fn edit_groups(edit: &UserEdit) -> Vec<(UserEditSection, Vec<String>)> {
    let account = &edit.account;
    let target = quote_account(&account.user, &account.host);
    let is_new = edit.original.is_none();
    let mut groups = Vec::new();

    let mut account_statements = Vec::new();
    // A renamed account keeps its grants and role edges, so rename first and address every
    // following statement at the new identity.
    if let Some(original) = &edit.original {
        let old_user = &original.account.user;
        let old_host = &original.account.host;
        if old_user != &account.user || old_host != &account.host {
            account_statements.push(format!(
                "RENAME USER {} TO {target}",
                quote_account(old_user, old_host)
            ));
        }
    }
    account_statements.extend(account_statements_inner(edit, &target, is_new));
    if !account_statements.is_empty() {
        groups.push((UserEditSection::Account, account_statements));
    }

    let server = server_privilege_statements(edit, &target);
    if !server.is_empty() {
        groups.push((UserEditSection::ServerPrivileges, server));
    }
    let objects = object_grant_statements(edit, &target);
    if !objects.is_empty() {
        groups.push((UserEditSection::ObjectGrants, objects));
    }
    let roles = role_statements(edit, &target);
    if !roles.is_empty() {
        groups.push((UserEditSection::Roles, roles));
    }
    groups
}

/// The ordered statements that turn the loaded state into the edit's state.
fn edit_statements(edit: &UserEdit) -> Vec<String> {
    let mut statements: Vec<String> = edit_groups(edit)
        .into_iter()
        .flat_map(|(_, statements)| statements)
        .collect();
    // Account-management statements are applied immediately, but the editor's preview shows (and
    // the save runs) an explicit refresh so the grant tables are reloaded deterministically.
    if !statements.is_empty() {
        statements.push("FLUSH PRIVILEGES".to_string());
    }
    statements
}

fn account_statements_inner(edit: &UserEdit, target: &str, is_new: bool) -> Vec<String> {
    let account = &edit.account;
    // The account as loaded; `None` while creating, in which case every option is written.
    let original = edit.original.as_ref().map(|details| &details.account);
    // Only emit an option when it differs from the loaded account, so an untouched editor produces
    // no statement at all (the confirmation diff stays empty and Save can be disabled).
    let changed = |same: bool| original.is_none() || !same;

    let mut clauses: Vec<String> = Vec::new();

    // The password hash is rewritten only when the user asked for it (or always when creating), so
    // a plain attribute edit never touches the stored secret.
    if let Some(password) = &edit.password {
        clauses.push(format!(
            "IDENTIFIED WITH {} BY {}",
            quote_string(&account.plugin),
            quote_string(password)
        ));
    } else if is_new {
        clauses.push(format!("IDENTIFIED WITH {}", quote_string(&account.plugin)));
    }

    // MySQL's account-option order is `[REQUIRE ...] [WITH ...] [PASSWORD ...] [ACCOUNT ...]`;
    // putting the resource limits before REQUIRE is a syntax error (1064) on ALTER USER.
    let require = require_clause(account);
    if changed(original.is_some_and(|original| require_clause(original) == require)) {
        clauses.push(require);
    }
    if changed(original.is_some_and(|original| {
        original.max_questions == account.max_questions
            && original.max_updates == account.max_updates
            && original.max_connections == account.max_connections
            && original.max_user_connections == account.max_user_connections
    })) {
        clauses.push(format!(
            "WITH MAX_QUERIES_PER_HOUR {} MAX_UPDATES_PER_HOUR {} \
             MAX_CONNECTIONS_PER_HOUR {} MAX_USER_CONNECTIONS {}",
            account.max_questions,
            account.max_updates,
            account.max_connections,
            account.max_user_connections
        ));
    }
    if changed(original.is_some_and(|original| original.account_locked == account.account_locked)) {
        clauses.push(format!(
            "ACCOUNT {}",
            if account.account_locked {
                "LOCK"
            } else {
                "UNLOCK"
            }
        ));
    }
    if changed(
        original.is_some_and(|original| original.password_lifetime == account.password_lifetime),
    ) {
        match account.password_lifetime {
            None => clauses.push("PASSWORD EXPIRE DEFAULT".to_string()),
            Some(0) => clauses.push("PASSWORD EXPIRE NEVER".to_string()),
            Some(days) => clauses.push(format!("PASSWORD EXPIRE INTERVAL {days} DAY")),
        }
    }

    let mut statements = Vec::new();
    if is_new || !clauses.is_empty() {
        let verb = if is_new { "CREATE USER" } else { "ALTER USER" };
        statements.push(format!("{verb} {target} {}", clauses.join(" ")));
    }

    // Expiring the password immediately is its own clause, so it cannot share the statement with
    // the lifetime policy above. Re-emit it after a password change (which clears the flag), but
    // leave an already-expired password alone otherwise.
    if account.password_expired
        && (edit.password.is_some()
            || changed(
                original
                    .is_some_and(|original| original.password_expired == account.password_expired),
            ))
    {
        statements.push(format!("ALTER USER {target} PASSWORD EXPIRE"));
    }
    statements
}

fn server_privilege_statements(edit: &UserEdit, target: &str) -> Vec<String> {
    let original = edit
        .original
        .as_ref()
        .map(|details| details.server_privileges.clone())
        .unwrap_or_default();
    let current = &edit.server_privileges;

    let mut statements = Vec::new();

    // Only the privileges this app knows about are diffed, so dynamic privileges the account may
    // hold (SYSTEM_VARIABLES_ADMIN, BACKUP_ADMIN, ...) are left untouched.
    let removed: Vec<&str> = original
        .difference(current)
        .filter(|privilege| **privilege != Privilege::GrantOption)
        .map(|privilege| privilege.sql_name())
        .collect();
    if !removed.is_empty() {
        statements.push(format!(
            "REVOKE {} ON *.* FROM {}",
            removed.join(", "),
            target
        ));
    }
    if original.contains(&Privilege::GrantOption) && !current.contains(&Privilege::GrantOption) {
        statements.push(format!("REVOKE GRANT OPTION ON *.* FROM {target}"));
    }

    let grant_option = current.contains(&Privilege::GrantOption);
    let mut added: Vec<&str> = current
        .difference(&original)
        .filter(|privilege| **privilege != Privilege::GrantOption)
        .map(|privilege| privilege.sql_name())
        .collect();
    let grant_option_added = grant_option && !original.contains(&Privilege::GrantOption);
    if added.is_empty() && !grant_option_added {
        return statements;
    }
    if added.is_empty() {
        added.push("USAGE");
    }
    let mut statement = format!("GRANT {} ON *.* TO {target}", added.join(", "));
    if grant_option {
        statement.push_str(" WITH GRANT OPTION");
    }
    statements.push(statement);
    statements
}

fn object_grant_statements(edit: &UserEdit, target: &str) -> Vec<String> {
    let original: BTreeMap<(String, String), BTreeSet<Privilege>> = edit
        .original
        .as_ref()
        .map(|details| {
            details
                .grants
                .iter()
                .map(|grant| {
                    (
                        (grant.database.clone(), grant.name.clone()),
                        grant.privileges.clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let current: BTreeMap<(String, String), BTreeSet<Privilege>> = edit
        .grants
        .iter()
        .map(|grant| {
            (
                (grant.database.clone(), grant.name.clone()),
                grant.privileges.clone(),
            )
        })
        .collect();

    let mut objects: BTreeSet<(String, String)> = BTreeSet::new();
    objects.extend(original.keys().cloned());
    objects.extend(current.keys().cloned());

    let mut statements = Vec::new();
    for key in objects {
        let old = original.get(&key).cloned().unwrap_or_default();
        let new = current.get(&key).cloned().unwrap_or_default();
        let object = object_spec(&key.0, &key.1);

        let removed: Vec<&str> = old
            .difference(&new)
            .filter(|privilege| **privilege != Privilege::GrantOption)
            .map(|privilege| privilege.sql_name())
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {} FROM {}",
                removed.join(", "),
                object,
                target
            ));
        }

        if new.is_empty() {
            // `REVOKE ALL [PRIVILEGES], GRANT OPTION` has no `ON` clause (it only strips a user's
            // global privileges), so an object that loses every privilege is emptied by revoking
            // exactly what it held above, plus its GRANT OPTION here.
            if old.contains(&Privilege::GrantOption) {
                statements.push(format!("REVOKE GRANT OPTION ON {} FROM {}", object, target));
            }
            continue;
        }

        if old.contains(&Privilege::GrantOption) && !new.contains(&Privilege::GrantOption) {
            statements.push(format!("REVOKE GRANT OPTION ON {} FROM {}", object, target));
        }

        let grant_option = new.contains(&Privilege::GrantOption);
        let mut added: Vec<&str> = new
            .difference(&old)
            .filter(|privilege| **privilege != Privilege::GrantOption)
            .map(|privilege| privilege.sql_name())
            .collect();
        let grant_option_added = grant_option && !old.contains(&Privilege::GrantOption);
        if added.is_empty() && !grant_option_added {
            continue;
        }
        if added.is_empty() {
            added.push("USAGE");
        }
        let mut statement = format!("GRANT {} ON {} TO {}", added.join(", "), object, target);
        if grant_option {
            statement.push_str(" WITH GRANT OPTION");
        }
        statements.push(statement);
    }
    statements
}

fn role_statements(edit: &UserEdit, target: &str) -> Vec<String> {
    let mut statements = Vec::new();

    // Roles granted to this account (the 成员属于 tab).
    let original_roles: BTreeMap<(String, String), bool> = edit
        .original
        .as_ref()
        .map(|details| {
            details
                .roles
                .iter()
                .map(|edge| {
                    (
                        (edge.role_user.clone(), edge.role_host.clone()),
                        edge.admin_option,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let current_roles: BTreeMap<(String, String), bool> = edit
        .roles
        .iter()
        .map(|(user, host, admin)| ((user.clone(), host.clone()), *admin))
        .collect();
    statements.extend(role_edge_statements(
        target,
        &original_roles,
        &current_roles,
        false,
    ));

    // Accounts that are members of this one (the 成员 tab). The this-account side is the role.
    let original_members: BTreeMap<(String, String), bool> = edit
        .original
        .as_ref()
        .map(|details| {
            details
                .members
                .iter()
                .map(|edge| {
                    (
                        (edge.member_user.clone(), edge.member_host.clone()),
                        edge.admin_option,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let current_members: BTreeMap<(String, String), bool> = edit
        .members
        .iter()
        .map(|(user, host, admin)| ((user.clone(), host.clone()), *admin))
        .collect();
    statements.extend(role_edge_statements(
        target,
        &original_members,
        &current_members,
        true,
    ));

    statements
}

/// Diff one side of the role edges. `target` is the account whose editor is open; `other_is_member`
/// selects the direction (this account is the role, `other` the member).
fn role_edge_statements(
    target: &str,
    original: &BTreeMap<(String, String), bool>,
    current: &BTreeMap<(String, String), bool>,
    other_is_member: bool,
) -> Vec<String> {
    let mut keys: BTreeSet<(String, String)> = BTreeSet::new();
    keys.extend(original.keys().cloned());
    keys.extend(current.keys().cloned());

    let mut statements = Vec::new();
    for key in keys {
        let other = quote_account(&key.0, &key.1);
        let old = original.get(&key).copied();
        let new = current.get(&key).copied();
        let (role, member) = if other_is_member {
            (target.to_string(), other)
        } else {
            (other, target.to_string())
        };
        match (old, new) {
            (Some(_), None) => statements.push(format!("REVOKE {role} FROM {member}")),
            (old, Some(admin)) => {
                if old == Some(admin) {
                    continue;
                }
                if old.is_some() {
                    statements.push(format!("REVOKE {role} FROM {member}"));
                }
                let mut statement = format!("GRANT {role} TO {member}");
                if admin {
                    statement.push_str(" WITH ADMIN OPTION");
                }
                statements.push(statement);
            }
            (None, None) => {}
        }
    }
    statements
}

fn require_clause(account: &UserAccount) -> String {
    match account.ssl_type.as_str() {
        "ANY" => "REQUIRE SSL".to_string(),
        "X509" => "REQUIRE X509".to_string(),
        "SPECIFIED" => {
            let mut parts = Vec::new();
            if !account.ssl_cipher.is_empty() {
                parts.push(format!("CIPHER {}", quote_string(&account.ssl_cipher)));
            }
            if !account.x509_issuer.is_empty() {
                parts.push(format!("ISSUER {}", quote_string(&account.x509_issuer)));
            }
            if !account.x509_subject.is_empty() {
                parts.push(format!("SUBJECT {}", quote_string(&account.x509_subject)));
            }
            if parts.is_empty() {
                "REQUIRE NONE".to_string()
            } else {
                format!("REQUIRE {}", parts.join(" AND "))
            }
        }
        _ => "REQUIRE NONE".to_string(),
    }
}

/// `db.*` for a database grant, ``db`.`table`` for an object grant.
fn object_spec(database: &str, name: &str) -> String {
    if name.is_empty() {
        format!("{}.*", quote_identifier(database))
    } else {
        format!("{}.{}", quote_identifier(database), quote_identifier(name))
    }
}

// ----- Decoding and quoting ------------------------------------------------------------------

fn text(row: &MySqlRow, name: &str) -> String {
    if let Ok(value) = row.try_get::<String, _>(name) {
        return value;
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(name) {
        return String::from_utf8_lossy(&value).into_owned();
    }
    String::new()
}

/// A `Y`/`N` enum column as a bool.
fn flag(row: &MySqlRow, name: &str) -> bool {
    text(row, name).eq_ignore_ascii_case("Y")
}

fn int(row: &MySqlRow, name: &str) -> Option<i64> {
    row.try_get::<Option<i64>, _>(name).ok().flatten()
}

fn uint(row: &MySqlRow, name: &str) -> u64 {
    if let Ok(value) = row.try_get::<Option<u64>, _>(name) {
        return value.unwrap_or(0);
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(name) {
        return value.unwrap_or(0).max(0) as u64;
    }
    0
}

/// `'user'@'host'`, the form `information_schema.GRANTEE` uses.
fn quote_grantee(user: &str, host: &str) -> String {
    format!("{}@{}", quote_string(user), quote_string(host))
}

/// `'user'@'host'`, the form used in account statements.
fn quote_account(user: &str, host: &str) -> String {
    format!("{}@{}", quote_string(user), quote_string(host))
}

fn quote_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('\'');
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\'' => output.push_str("''"),
            _ => output.push(character),
        }
    }
    output.push('\'');
    output
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

/// Every account's privileges on one object, for the privilege manager's matrix.
pub(crate) async fn object_privilege_matrix(
    pool: &MySqlPool,
    database: &str,
    name: &str,
) -> Result<Vec<ObjectPrivilegeRow>> {
    let rows = if name.is_empty() {
        sqlx::query(
            "SELECT GRANTEE, PRIVILEGE_TYPE FROM information_schema.SCHEMA_PRIVILEGES \
             WHERE TABLE_SCHEMA = ?",
        )
        .bind(database)
        .fetch_all(pool)
        .await
        .map_err(map_query_error)?
    } else {
        sqlx::query(
            "SELECT GRANTEE, PRIVILEGE_TYPE FROM information_schema.TABLE_PRIVILEGES \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
        )
        .bind(database)
        .bind(name)
        .fetch_all(pool)
        .await
        .map_err(map_query_error)?
    };

    let mut merged: BTreeMap<(String, String), BTreeSet<Privilege>> = BTreeMap::new();
    for row in &rows {
        let (user, host) = parse_grantee(&text(row, "GRANTEE"));
        let Some(privilege) = Privilege::from_sql_name(&text(row, "PRIVILEGE_TYPE")) else {
            continue;
        };
        merged.entry((user, host)).or_default().insert(privilege);
    }

    Ok(merged
        .into_iter()
        .map(|((user, host), privileges)| ObjectPrivilegeRow {
            user,
            host,
            privileges,
        })
        .collect())
}

/// Replace the listed accounts' privileges on one object, leaving every other grant untouched.
pub(crate) async fn set_object_privileges(
    pool: &MySqlPool,
    database: &str,
    name: &str,
    rows: &[ObjectPrivilegeRow],
) -> Result<()> {
    let current: BTreeMap<(String, String), BTreeSet<Privilege>> =
        object_privilege_matrix(pool, database, name)
            .await?
            .into_iter()
            .map(|row| ((row.user, row.host), row.privileges))
            .collect();
    for statement in object_privilege_statements(database, name, &current, rows) {
        run(pool, statement).await?;
    }
    Ok(())
}

/// The statements that turn `current` into `rows` for one object.
pub(crate) fn object_privileges_sql(
    database: &str,
    name: &str,
    original: &[ObjectPrivilegeRow],
    rows: &[ObjectPrivilegeRow],
) -> String {
    let current: BTreeMap<(String, String), BTreeSet<Privilege>> = original
        .iter()
        .map(|row| ((row.user.clone(), row.host.clone()), row.privileges.clone()))
        .collect();
    let statements = object_privilege_statements(database, name, &current, rows);
    if statements.is_empty() {
        return String::new();
    }
    let mut script = statements.join(";\n");
    script.push(';');
    script
}

fn object_privilege_statements(
    database: &str,
    name: &str,
    current: &BTreeMap<(String, String), BTreeSet<Privilege>>,
    rows: &[ObjectPrivilegeRow],
) -> Vec<String> {
    let object = object_spec(database, name);
    let mut statements = Vec::new();

    for row in rows {
        let old = current
            .get(&(row.user.clone(), row.host.clone()))
            .cloned()
            .unwrap_or_default();
        let new = &row.privileges;
        let target = quote_account(&row.user, &row.host);

        if old.is_empty() && new.is_empty() {
            continue;
        }

        let removed: Vec<&str> = old
            .difference(new)
            .filter(|privilege| **privilege != Privilege::GrantOption)
            .map(|privilege| privilege.sql_name())
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {} FROM {}",
                removed.join(", "),
                object,
                target
            ));
        }
        if old.contains(&Privilege::GrantOption) && !new.contains(&Privilege::GrantOption) {
            statements.push(format!("REVOKE GRANT OPTION ON {} FROM {}", object, target));
        }

        let grant_option = new.contains(&Privilege::GrantOption);
        let mut added: Vec<&str> = new
            .difference(&old)
            .filter(|privilege| **privilege != Privilege::GrantOption)
            .map(|privilege| privilege.sql_name())
            .collect();
        let grant_option_added = grant_option && !old.contains(&Privilege::GrantOption);
        if added.is_empty() && !grant_option_added {
            continue;
        }
        if added.is_empty() {
            added.push("USAGE");
        }
        let mut statement = format!("GRANT {} ON {} TO {}", added.join(", "), object, target);
        if grant_option {
            statement.push_str(" WITH GRANT OPTION");
        }
        statements.push(statement);
    }

    // Accounts that are no longer listed at all (removed with 删除权限) lose every privilege they
    // held on this object.
    for ((user, host), old) in current {
        if rows
            .iter()
            .any(|row| row.user == *user && row.host == *host)
        {
            continue;
        }
        let removed: Vec<&str> = old
            .iter()
            .filter(|privilege| **privilege != Privilege::GrantOption)
            .map(|privilege| privilege.sql_name())
            .collect();
        let target = quote_account(user, host);
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {} FROM {}",
                removed.join(", "),
                object,
                target
            ));
        }
        if old.contains(&Privilege::GrantOption) {
            statements.push(format!("REVOKE GRANT OPTION ON {} FROM {}", object, target));
        }
    }
    statements
}

async fn run(pool: &MySqlPool, statement: String) -> Result<()> {
    sqlx::raw_sql(AssertSqlSafe(statement))
        .execute(pool)
        .await
        .map_err(map_query_error)?;
    Ok(())
}

/// Split an `information_schema` GRANTEE value (`` 'user'@'host' ``) into its parts.
fn parse_grantee(grantee: &str) -> (String, String) {
    let Some(at) = grantee.find("'@'") else {
        return (grantee.trim_matches('\'').to_string(), String::new());
    };
    let user = grantee[..at].trim_matches('\'').replace("''", "'");
    let host = grantee[at + 3..].trim_matches('\'').replace("''", "'");
    (user, host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(user: &str, host: &str, privileges: &[Privilege]) -> ObjectPrivilegeRow {
        ObjectPrivilegeRow {
            user: user.to_string(),
            host: host.to_string(),
            privileges: privileges.iter().copied().collect(),
        }
    }

    fn current(
        entries: &[(&str, &str, &[Privilege])],
    ) -> BTreeMap<(String, String), BTreeSet<Privilege>> {
        entries
            .iter()
            .map(|(user, host, privileges)| {
                (
                    (user.to_string(), host.to_string()),
                    privileges.iter().copied().collect(),
                )
            })
            .collect()
    }

    #[test]
    fn grants_new_privileges_on_a_database() {
        let statements = object_privilege_statements(
            "shop",
            "",
            &current(&[]),
            &[row("alice", "%", &[Privilege::Select, Privilege::Insert])],
        );
        assert_eq!(
            statements,
            vec!["GRANT INSERT, SELECT ON `shop`.* TO 'alice'@'%'"]
        );
    }

    #[test]
    fn revokes_only_the_deselected_privileges() {
        let statements = object_privilege_statements(
            "shop",
            "orders",
            &current(&[("alice", "%", &[Privilege::Select, Privilege::Insert])]),
            &[row("alice", "%", &[Privilege::Select])],
        );
        assert_eq!(
            statements,
            vec!["REVOKE INSERT ON `shop`.`orders` FROM 'alice'@'%'"]
        );
    }

    #[test]
    fn revokes_every_privilege_of_a_removed_account() {
        let statements = object_privilege_statements(
            "shop",
            "",
            &current(&[("alice", "%", &[Privilege::Select, Privilege::GrantOption])]),
            &[],
        );
        assert_eq!(
            statements,
            vec![
                "REVOKE SELECT ON `shop`.* FROM 'alice'@'%'",
                "REVOKE GRANT OPTION ON `shop`.* FROM 'alice'@'%'",
            ]
        );
    }

    #[test]
    fn leaves_untouched_accounts_alone() {
        let statements = object_privilege_statements(
            "shop",
            "",
            &current(&[
                ("alice", "%", &[Privilege::Select]),
                ("bob", "localhost", &[Privilege::Insert]),
            ]),
            &[row("alice", "%", &[Privilege::Select])],
        );
        assert_eq!(
            statements,
            vec!["REVOKE INSERT ON `shop`.* FROM 'bob'@'localhost'"]
        );
    }

    #[test]
    fn splits_a_grantee_into_user_and_host() {
        assert_eq!(
            parse_grantee("'root'@'localhost'"),
            ("root".to_string(), "localhost".to_string())
        );
        assert_eq!(
            parse_grantee("'o''brien'@'%'"),
            ("o'brien".to_string(), "%".to_string())
        );
    }

    #[test]
    fn account_options_are_emitted_in_mysql_order() {
        let original = UserDetails {
            account: UserAccount {
                user: "test".to_string(),
                host: "%".to_string(),
                password_lifetime: Some(30),
                ..Default::default()
            },
            ..Default::default()
        };
        let edit = UserEdit {
            original: Some(original),
            account: UserAccount {
                user: "test".to_string(),
                host: "%".to_string(),
                plugin: "mysql_native_password".to_string(),
                // A real REQUIRE change, a limit change and a lifetime change: every option below
                // is diffed, so each must actually differ from the loaded account to be emitted.
                ssl_type: "ANY".to_string(),
                max_questions: 10,
                password_lifetime: Some(90),
                ..Default::default()
            },
            password: None,
            server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
        };
        let alter = edit_statements(&edit)
            .into_iter()
            .find(|statement| statement.starts_with("ALTER USER"))
            .expect("an ALTER USER statement");
        let require = alter.find("REQUIRE").expect("REQUIRE");
        let with = alter.find("WITH").expect("WITH");
        let expire = alter.find("PASSWORD EXPIRE").expect("PASSWORD EXPIRE");
        assert!(
            require < with && with < expire,
            "clauses out of order: {alter}"
        );
    }

    #[test]
    fn an_unchanged_account_produces_no_statements() {
        let account = UserAccount {
            user: "test".to_string(),
            host: "%".to_string(),
            plugin: "caching_sha2_password".to_string(),
            max_questions: 5,
            password_lifetime: Some(90),
            account_locked: true,
            ..Default::default()
        };
        let original = UserDetails {
            account: account.clone(),
            ..Default::default()
        };
        let edit = UserEdit {
            original: Some(original),
            account,
            password: None,
            server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
        };
        assert!(edit_groups(&edit).is_empty(), "{:?}", edit_groups(&edit));
        assert!(edit_statements(&edit).is_empty());
        assert_eq!(edit_sql(&edit), "");
    }

    #[test]
    fn renaming_without_attribute_changes_only_renames() {
        let account = UserAccount {
            user: "test".to_string(),
            host: "%".to_string(),
            plugin: "caching_sha2_password".to_string(),
            max_questions: 5,
            password_lifetime: Some(90),
            ..Default::default()
        };
        let original = UserDetails {
            account: account.clone(),
            ..Default::default()
        };
        let edit = UserEdit {
            original: Some(original),
            account: UserAccount {
                user: "renamed".to_string(),
                ..account
            },
            password: None,
            server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
        };
        assert_eq!(
            edit_statements(&edit),
            vec![
                "RENAME USER 'test'@'%' TO 'renamed'@'%'",
                "FLUSH PRIVILEGES",
            ]
        );
    }

    #[test]
    fn emptying_an_object_revokes_it_without_the_on_less_all_form() {
        let original = UserDetails {
            account: UserAccount {
                user: "test".to_string(),
                host: "%".to_string(),
                ..Default::default()
            },
            grants: vec![ObjectGrant {
                database: "test".to_string(),
                name: "t".to_string(),
                privileges: [Privilege::Select, Privilege::GrantOption]
                    .into_iter()
                    .collect(),
            }],
            ..Default::default()
        };
        let edit = UserEdit {
            original: Some(original),
            account: UserAccount {
                user: "test".to_string(),
                host: "%".to_string(),
                plugin: "mysql_native_password".to_string(),
                ..Default::default()
            },
            password: None,
            server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
        };
        let statements = edit_statements(&edit);
        let joined = statements.join("\n");
        assert!(
            !joined.contains("REVOKE ALL PRIVILEGES, GRANT OPTION ON"),
            "the ON-less REVOKE ALL form is invalid for an object: {joined}"
        );
        assert!(
            statements
                .iter()
                .any(|s| s == "REVOKE SELECT ON `test`.`t` FROM 'test'@'%'"),
            "{joined}"
        );
        assert!(
            statements
                .iter()
                .any(|s| s == "REVOKE GRANT OPTION ON `test`.`t` FROM 'test'@'%'"),
            "{joined}"
        );
    }

    #[test]
    fn edit_groups_group_the_change_and_append_a_refresh() {
        let original = UserDetails {
            account: UserAccount {
                user: "test".to_string(),
                host: "%".to_string(),
                ..Default::default()
            },
            server_privileges: [Privilege::Select].into_iter().collect(),
            ..Default::default()
        };
        let edit = UserEdit {
            original: Some(original),
            account: UserAccount {
                user: "test".to_string(),
                host: "%".to_string(),
                plugin: "mysql_native_password".to_string(),
                max_questions: 10,
                ..Default::default()
            },
            password: None,
            server_privileges: [Privilege::Insert].into_iter().collect(),
            grants: Vec::new(),
            roles: Vec::new(),
            members: Vec::new(),
        };

        let groups = edit_groups(&edit);
        assert!(
            groups
                .iter()
                .any(|(section, _)| *section == UserEditSection::Account),
            "{groups:?}"
        );
        assert!(
            groups
                .iter()
                .any(|(section, _)| *section == UserEditSection::ServerPrivileges),
            "{groups:?}"
        );

        let statements = edit_statements(&edit);
        assert_eq!(
            statements.last().map(String::as_str),
            Some("FLUSH PRIVILEGES")
        );
    }
}

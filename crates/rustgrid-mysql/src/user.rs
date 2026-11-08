//! MySQL account (user/role) and privilege administration, backing the Users main tab and the
//! user editor.
//!
//! Accounts and their attributes live in `mysql.user`; the granted privileges are read from
//! `information_schema.*_PRIVILEGES` views (which report the effective grants as keywords) and
//! role memberships from `mysql.role_edges` (MySQL 8) or `mysql.roles_mapping` (MariaDB). The
//! account projection adapts to the columns the server actually has - the same idea as Navicat's
//! version-dependent account query, without the version arithmetic - so one reader serves every
//! MySQL and MariaDB release. Writes are replayed as a script of `CREATE USER` / `ALTER USER` /
//! `GRANT` / `REVOKE` statements through the text protocol.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    AccountKind, ObjectGrant, ObjectPrivilegeRow, PrivilegeCatalog, PrivilegeGroup, PrivilegeId,
    PrivilegeInfo, PrivilegePreset, Result, RoleMembership, UserAccount, UserDetails, UserEdit,
    UserEditSection,
};
use sqlx::mysql::MySqlRow;
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use crate::connection::map_query_error;

/// The `mysql.user` columns the account editor reads, in projection order. `plugin`,
/// `password_expired`, the resource limits and the TLS columns are the ones the 常规/高级 tabs edit.
/// Every one of them is optional: MySQL and MariaDB disagree about which exist, so a missing
/// column is projected as `NULL` rather than failing the whole Users tab.
const ACCOUNT_COLUMNS: [&str; 16] = [
    "User",
    "Host",
    "plugin",
    "authentication_string",
    "password_expired",
    "password_lifetime",
    "account_locked",
    "max_questions",
    "max_updates",
    "max_connections",
    "max_user_connections",
    "ssl_type",
    "ssl_cipher",
    "x509_issuer",
    "x509_subject",
    "super_priv",
];

/// Which `mysql.*` objects this server actually has: the columns of `mysql.user` and which of the
/// two role tables it keeps. Navicat builds its account query from the server version; reading the
/// catalog answers the same question directly and also copes with the columns MariaDB adds.
#[derive(Debug, Clone)]
struct CatalogShape {
    /// The columns present in `mysql.user`.
    user_columns: BTreeSet<String>,
    /// Whether `mysql.user` has MariaDB's `is_role` column, the only direct role marker. MySQL 8
    /// has no such column, so its roles are recognised from `mysql.role_edges` instead.
    is_role_column: bool,
    /// Whether `mysql.role_edges` exists (MySQL 8's role graph).
    role_edges: bool,
    /// Whether `mysql.roles_mapping` exists (MariaDB's role grants).
    roles_mapping: bool,
}

impl CatalogShape {
    /// Assume every optional column and the MySQL role table, so a server that refuses the catalog
    /// probe keeps exactly the behaviour it had before the probe existed.
    fn assumed_complete() -> Self {
        Self {
            user_columns: ACCOUNT_COLUMNS
                .iter()
                .map(|name| name.to_string())
                .collect(),
            is_role_column: false,
            role_edges: true,
            roles_mapping: false,
        }
    }

    /// The `SELECT` list for `mysql.user`: each column the editor reads projects as itself when the
    /// server has it, and as `NULL` when it does not. `User`/`Host` identify every account, so a
    /// server that reports neither is treated as unreadable rather than as a host-less account.
    ///
    /// `is_role` is appended last because it is not one of the editor's own columns: MariaDB has
    /// it, MySQL 8 does not, and there a role is recognised from the role graph instead.
    fn account_projection(&self) -> String {
        let mut projection = ACCOUNT_COLUMNS
            .iter()
            .map(|name| {
                if self.user_columns.contains(*name) {
                    (*name).to_string()
                } else {
                    format!("NULL AS {name}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        projection.push_str(if self.is_role_column {
            ", is_role"
        } else {
            ", NULL AS is_role"
        });
        projection
    }
}

/// Probe the `mysql` catalog for the account columns and role table this server has.
async fn catalog_shape(pool: &MySqlPool) -> CatalogShape {
    // No parameters: the text protocol also works on a MySQL-protocol proxy that rejects prepares.
    let rows = match sqlx::raw_sql(
        "SELECT TABLE_NAME, COLUMN_NAME FROM information_schema.COLUMNS \
         WHERE TABLE_SCHEMA = 'mysql' AND TABLE_NAME IN ('user', 'role_edges', 'roles_mapping')",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        // A server that will not report its own catalog still gets a working account query.
        Err(_) => return CatalogShape::assumed_complete(),
    };

    let mut shape = CatalogShape {
        user_columns: BTreeSet::new(),
        is_role_column: false,
        role_edges: false,
        roles_mapping: false,
    };
    for row in &rows {
        match text(row, "TABLE_NAME").as_str() {
            "user" => {
                let column = text(row, "COLUMN_NAME");
                if column.eq_ignore_ascii_case("is_role") {
                    shape.is_role_column = true;
                }
                shape.user_columns.insert(column);
            }
            "role_edges" => shape.role_edges = true,
            "roles_mapping" => shape.roles_mapping = true,
            _ => {}
        }
    }
    // The account's identity columns are the one thing every version has; seeing none of them means
    // the catalog is hidden from this account, not that the table is column-less.
    if !shape.user_columns.contains("User") || !shape.user_columns.contains("Host") {
        return CatalogShape::assumed_complete();
    }
    shape
}

/// Whether roles exist on this server. MySQL 8 keeps them in `mysql.role_edges` and MariaDB marks
/// them with `mysql.user.is_role` / `mysql.roles_mapping`; MySQL 5.7 and earlier have none of the
/// three, which is what hides the 用户 / 角色 switch there.
pub(crate) async fn roles_supported(pool: &MySqlPool) -> bool {
    let shape = catalog_shape(pool).await;
    shape.is_role_column || shape.role_edges || shape.roles_mapping
}

/// Every account on the server, ordered by user then host.
pub(crate) async fn list_users(pool: &MySqlPool) -> Result<Vec<UserAccount>> {
    let shape = catalog_shape(pool).await;
    let sql = format!(
        "SELECT {} FROM mysql.user ORDER BY User, Host",
        shape.account_projection()
    );
    let rows = sqlx::raw_sql(AssertSqlSafe(sql))
        .fetch_all(pool)
        .await
        .map_err(map_query_error)?;
    let mut accounts: Vec<UserAccount> = rows.iter().map(read_account).collect();
    // MySQL 8 keeps no role flag on the account row, so its roles are recognised from the role
    // graph: an account that is granted to another account is a role.
    let roles = inferred_roles(pool, &shape).await;
    if !roles.is_empty() {
        for account in &mut accounts {
            if roles.contains(&(account.user.clone(), account.host.clone())) {
                account.kind = AccountKind::Role;
            }
        }
    }
    Ok(accounts)
}

/// The roles a server without an `is_role` column still knows about, taken from its role graph. An
/// account that is granted to nobody cannot be told apart from a user and is reported as one.
async fn inferred_roles(pool: &MySqlPool, shape: &CatalogShape) -> BTreeSet<(String, String)> {
    if shape.is_role_column {
        return BTreeSet::new();
    }
    let pairs: Vec<(String, String)> = if shape.role_edges {
        match sqlx::raw_sql("SELECT FROM_USER, FROM_HOST FROM mysql.role_edges")
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows
                .iter()
                .map(|row| (text(row, "FROM_USER"), text(row, "FROM_HOST")))
                .collect(),
            Err(_) => Vec::new(),
        }
    } else if shape.roles_mapping {
        // MariaDB records no role host; an unqualified role resolves to `%`.
        match sqlx::raw_sql("SELECT Role FROM mysql.roles_mapping")
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows
                .iter()
                .map(|row| (text(row, "Role"), "%".to_string()))
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };
    pairs
        .into_iter()
        .filter(|(user, _)| !user.is_empty())
        .collect()
}

/// One account's attributes plus its granted server privileges, role edges and object grants.
pub(crate) async fn user_details(pool: &MySqlPool, user: &str, host: &str) -> Result<UserDetails> {
    let shape = catalog_shape(pool).await;
    let sql = format!(
        "SELECT {} FROM mysql.user WHERE User = {} AND Host = {}",
        shape.account_projection(),
        quote_string(user),
        quote_string(host)
    );
    let row = sqlx::raw_sql(AssertSqlSafe(sql))
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
    let (roles, members) = fetch_role_edges(pool, user, host, &shape).await?;

    Ok(UserDetails {
        account,
        server_privileges,
        denied_server_privileges: BTreeSet::new(),
        grant_option_server_privileges: BTreeSet::new(),
        grants,
        default_privileges: Vec::new(),
        roles,
        members,
    })
}

/// Drop one account. A role is dropped with `DROP ROLE` so its role-ness is preserved in the
/// server's own bookkeeping; everything else is a plain `DROP USER`.
pub(crate) async fn drop_user(pool: &MySqlPool, user: &str, host: &str) -> Result<()> {
    let verb = if account_is_role(pool, user, host).await {
        "DROP ROLE"
    } else {
        "DROP USER"
    };
    let sql = format!("{verb} {}", quote_account(user, host));
    sqlx::raw_sql(AssertSqlSafe(sql))
        .execute(pool)
        .await
        .map_err(map_query_error)?;
    Ok(())
}

/// Whether `user@host` is a role rather than a user. The list carries the kind, but drop only
/// receives a name, so the answer is looked up once here.
async fn account_is_role(pool: &MySqlPool, user: &str, host: &str) -> bool {
    let shape = catalog_shape(pool).await;
    if shape.is_role_column
        && let Ok(rows) = sqlx::raw_sql(AssertSqlSafe(format!(
            "SELECT is_role FROM mysql.user WHERE User = {} AND Host = {}",
            quote_string(user),
            quote_string(host)
        )))
        .fetch_all(pool)
        .await
    {
        return rows
            .first()
            .is_some_and(|row| text(row, "is_role").eq_ignore_ascii_case("Y"));
    }
    inferred_roles(pool, &shape)
        .await
        .contains(&(user.to_string(), host.to_string()))
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

/// `(keyword, i18n key, server group)` for the 28 MySQL server privileges, in the order the
/// 服务器权限 grid shows them.
const SERVER_PRIVILEGES: &[(&str, &str, &str)] = &[
    ("ALTER", "user.priv.alter", "ddl"),
    ("ALTER ROUTINE", "user.priv.alter_routine", "ddl"),
    ("CREATE", "user.priv.create", "ddl"),
    ("CREATE ROUTINE", "user.priv.create_routine", "ddl"),
    (
        "CREATE TEMPORARY TABLES",
        "user.priv.create_temporary_tables",
        "ddl",
    ),
    ("CREATE USER", "user.priv.create_user", "account"),
    ("CREATE VIEW", "user.priv.create_view", "ddl"),
    ("DELETE", "user.priv.delete", "dml"),
    ("DROP", "user.priv.drop", "ddl"),
    ("EVENT", "user.priv.event", "account"),
    ("EXECUTE", "user.priv.execute", "exec"),
    ("FILE", "user.priv.file", "admin"),
    ("GRANT OPTION", "user.priv.grant_option", "admin"),
    ("INDEX", "user.priv.index", "ddl"),
    ("INSERT", "user.priv.insert", "dml"),
    ("LOCK TABLES", "user.priv.lock_tables", "exec"),
    ("PROCESS", "user.priv.process", "admin"),
    ("REFERENCES", "user.priv.references", "ddl"),
    ("RELOAD", "user.priv.reload", "admin"),
    (
        "REPLICATION CLIENT",
        "user.priv.replication_client",
        "replication",
    ),
    (
        "REPLICATION SLAVE",
        "user.priv.replication_slave",
        "replication",
    ),
    ("SELECT", "user.priv.select", "dml"),
    ("SHOW DATABASES", "user.priv.show_databases", "exec"),
    ("SHOW VIEW", "user.priv.show_view", "exec"),
    ("SHUTDOWN", "user.priv.shutdown", "admin"),
    ("SUPER", "user.priv.super", "admin"),
    ("TRIGGER", "user.priv.trigger", "ddl"),
    ("UPDATE", "user.priv.update", "dml"),
];

/// `(keyword, object group)` for the subset of server privileges grantable on an object.
const OBJECT_PRIVILEGES: &[(&str, &str)] = &[
    ("ALTER", "ddl"),
    ("ALTER ROUTINE", "routines"),
    ("CREATE", "ddl"),
    ("CREATE ROUTINE", "routines"),
    ("CREATE TEMPORARY TABLES", "ddl"),
    ("CREATE VIEW", "ddl"),
    ("DELETE", "dml"),
    ("DROP", "ddl"),
    ("EXECUTE", "routines"),
    ("GRANT OPTION", "routines"),
    ("INDEX", "ddl"),
    ("INSERT", "dml"),
    ("REFERENCES", "routines"),
    ("SELECT", "dml"),
    ("SHOW VIEW", "routines"),
    ("TRIGGER", "routines"),
    ("UPDATE", "dml"),
];

/// The MySQL privilege catalog: the server and object privileges, their editor groups and the
/// quick presets. Backs the account editor and the object-privilege manager.
pub(crate) fn privilege_catalog() -> PrivilegeCatalog {
    let groups = [
        ("dml", "user.create.priv_group.dml"),
        ("ddl", "user.create.priv_group.ddl"),
        ("exec", "user.create.server_group.exec"),
        ("admin", "user.create.server_group.admin"),
        ("replication", "user.create.server_group.replication"),
        ("account", "user.create.server_group.account"),
        ("routines", "user.create.priv_group.routines"),
    ]
    .into_iter()
    .map(|(id, label_key)| PrivilegeGroup {
        id: id.to_string(),
        label_key: Some(label_key.to_string()),
        label: id.to_string(),
    })
    .collect();

    let privileges = SERVER_PRIVILEGES
        .iter()
        .map(|(keyword, label_key, server_group)| {
            let object_group = OBJECT_PRIVILEGES
                .iter()
                .find(|(name, _)| name == keyword)
                .map(|(_, group)| *group);
            match object_group {
                Some(object_group) => PrivilegeInfo::both(
                    *keyword,
                    server_group,
                    object_group,
                    Some(*label_key),
                    keyword,
                ),
                None => PrivilegeInfo::server(*keyword, server_group, Some(*label_key), keyword),
            }
        })
        .collect();

    PrivilegeCatalog {
        groups,
        privileges,
        server_presets: vec![
            preset("none", "user.create.template.none", &[]),
            preset(
                "read_only",
                "user.create.template.read_only",
                &["SELECT", "SHOW VIEW"],
            ),
            preset(
                "read_write",
                "user.create.template.read_write",
                &[
                    "SELECT",
                    "INSERT",
                    "UPDATE",
                    "DELETE",
                    "SHOW VIEW",
                    "EXECUTE",
                ],
            ),
            preset(
                "developer",
                "user.create.template.developer",
                &[
                    "SELECT",
                    "INSERT",
                    "UPDATE",
                    "DELETE",
                    "SHOW VIEW",
                    "EXECUTE",
                    "CREATE",
                    "ALTER",
                    "DROP",
                    "INDEX",
                    "CREATE VIEW",
                    "CREATE ROUTINE",
                    "ALTER ROUTINE",
                    "REFERENCES",
                    "TRIGGER",
                    "EVENT",
                    "CREATE TEMPORARY TABLES",
                    "LOCK TABLES",
                ],
            ),
            preset(
                "admin",
                "user.create.template.admin",
                &SERVER_PRIVILEGES
                    .iter()
                    .map(|(keyword, _, _)| *keyword)
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
                    .map(|(keyword, _)| *keyword)
                    .collect::<Vec<_>>(),
            ),
        ],
        default_privileges: Vec::new(),
        deny_supported: false,
        grant_option_supported: false,
        securable_classes: Vec::new(),
    }
}

fn preset(id: &str, label_key: &str, keywords: &[&str]) -> PrivilegePreset {
    PrivilegePreset {
        id: id.to_string(),
        label_key: Some(label_key.to_string()),
        label: id.to_string(),
        privileges: keywords
            .iter()
            .map(|keyword| PrivilegeId::new(*keyword))
            .collect(),
    }
}

/// Resolve a keyword the server reports to the canonical privilege id, when it is one of MySQL's
/// known privileges. Unknown (dynamic) privileges are dropped so they are left untouched on save.
fn privilege_id(keyword: &str) -> Option<PrivilegeId> {
    SERVER_PRIVILEGES
        .iter()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(keyword.trim()))
        .map(|(name, _, _)| PrivilegeId::new(*name))
}

/// Whether a privilege set carries the special `GRANT OPTION` privilege.
fn has_grant_option(privileges: &BTreeSet<PrivilegeId>) -> bool {
    privileges
        .iter()
        .any(|privilege| privilege.matches("GRANT OPTION"))
}

/// The privileges one `*_PRIVILEGES` row reports for its object, from the row's keyword and
/// `IS_GRANTABLE` flag.
///
/// A database the account holds an entry for but grants nothing on (`GRANT USAGE ON db.*`) is
/// reported as the keyword `USAGE`, which is not a privilege this catalog knows. `IS_GRANTABLE` is
/// the only sign such an entry exists (MySQL always keeps a grant option there, since a `mysql.db`
/// row with no privileges and no grant option is pruned), so reading it keeps that database in the
/// account's grant list — as Navicat does — and lets the editor revoke it again
/// (`REVOKE GRANT OPTION ON db.*` is what drops the entry).
fn row_privileges(keyword: &str, grantable: bool) -> Vec<PrivilegeId> {
    let mut privileges = Vec::new();
    if let Some(privilege) = privilege_id(keyword) {
        privileges.push(privilege);
    }
    if grantable {
        privileges.push(PrivilegeId::new("GRANT OPTION"));
    }
    privileges
}

// ----- Reading -------------------------------------------------------------------------------

fn read_account(row: &MySqlRow) -> UserAccount {
    UserAccount {
        user: text(row, "User"),
        host: text(row, "Host"),
        // MariaDB marks a role directly; MySQL 8 has no such column, so `list_users` fills the
        // kind in from the role graph afterwards.
        kind: if text(row, "is_role").eq_ignore_ascii_case("Y") {
            AccountKind::Role
        } else {
            AccountKind::User
        },
        plugin: text(row, "plugin"),
        password_set: !text(row, "authentication_string").is_empty(),
        password_expired: flag(row, "password_expired"),
        password_lifetime: opt_uint(row, "password_lifetime").map(|value| value as u32),
        password_valid_until: None,
        account_locked: flag(row, "account_locked"),
        max_questions: uint(row, "max_questions"),
        max_updates: uint(row, "max_updates"),
        max_connections: uint(row, "max_connections"),
        max_user_connections: uint(row, "max_user_connections"),
        ssl_type: text(row, "ssl_type"),
        ssl_cipher: text(row, "ssl_cipher"),
        x509_issuer: text(row, "x509_issuer"),
        x509_subject: text(row, "x509_subject"),
        default_tablespace: String::new(),
        profile: String::new(),
        tablespace_quota: String::new(),
        is_super_user: flag(row, "super_priv"),
        ..UserAccount::default()
    }
}

async fn fetch_server_privileges(pool: &MySqlPool, grantee: &str) -> Result<BTreeSet<PrivilegeId>> {
    // The view can be unavailable on some servers; report no granted privileges rather than
    // failing the account load.
    let sql = format!(
        "SELECT PRIVILEGE_TYPE FROM information_schema.USER_PRIVILEGES WHERE GRANTEE = {}",
        quote_string(grantee)
    );
    let rows = match sqlx::raw_sql(AssertSqlSafe(sql)).fetch_all(pool).await {
        Ok(rows) => rows,
        Err(_) => return Ok(BTreeSet::new()),
    };
    Ok(rows
        .iter()
        .filter_map(|row| privilege_id(&text(row, "PRIVILEGE_TYPE")))
        .collect())
}

/// The object-level grants of an account, merged into one row per `(database, object)`.
async fn fetch_object_grants(pool: &MySqlPool, grantee: &str) -> Result<Vec<ObjectGrant>> {
    let mut merged: BTreeMap<(String, String), BTreeSet<PrivilegeId>> = BTreeMap::new();
    for sql in [
        "SELECT TABLE_SCHEMA, '' AS object_name, PRIVILEGE_TYPE, IS_GRANTABLE \
         FROM information_schema.SCHEMA_PRIVILEGES WHERE GRANTEE = ",
        "SELECT TABLE_SCHEMA, TABLE_NAME AS object_name, PRIVILEGE_TYPE, IS_GRANTABLE \
         FROM information_schema.TABLE_PRIVILEGES WHERE GRANTEE = ",
        "SELECT ROUTINE_SCHEMA AS TABLE_SCHEMA, ROUTINE_NAME AS object_name, PRIVILEGE_TYPE, \
         IS_GRANTABLE \
         FROM information_schema.ROUTINE_PRIVILEGES WHERE GRANTEE = ",
    ] {
        // A view may be missing on some servers (e.g. `ROUTINE_PRIVILEGES` on MariaDB builds);
        // a missing scope simply contributes no grants rather than failing the whole load.
        let sql = format!("{sql}{}", quote_string(grantee));
        let rows = match sqlx::raw_sql(AssertSqlSafe(sql)).fetch_all(pool).await {
            Ok(rows) => rows,
            Err(_) => continue,
        };
        for row in &rows {
            let granted = row_privileges(
                &text(row, "PRIVILEGE_TYPE"),
                text(row, "IS_GRANTABLE").eq_ignore_ascii_case("YES"),
            );
            if granted.is_empty() {
                continue;
            }
            merged
                .entry((text(row, "TABLE_SCHEMA"), text(row, "object_name")))
                .or_default()
                .extend(granted);
        }
    }

    Ok(merged
        .into_iter()
        .map(|((database, name), privileges)| ObjectGrant {
            database,
            // MySQL has no schema level; its database-wide grant already lives in `database`.
            schema: String::new(),
            name,
            privileges,
        })
        .collect())
}

/// The role edges touching one account: the roles it is a member of, and the members of it (when
/// it is a role). MySQL 8 keeps them in `mysql.role_edges`; MariaDB keeps the same edges in
/// `mysql.roles_mapping`; MySQL 5.7 has neither, which yields no edges at all.
async fn fetch_role_edges(
    pool: &MySqlPool,
    user: &str,
    host: &str,
    shape: &CatalogShape,
) -> Result<(Vec<RoleMembership>, Vec<RoleMembership>)> {
    let edges = if shape.role_edges {
        role_edges_from_mysql(pool).await
    } else if shape.roles_mapping {
        role_edges_from_mariadb(pool).await
    } else {
        Vec::new()
    };

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

/// `mysql.role_edges` (MySQL 8): `FROM_*` is the role, `TO_*` the member it is granted to.
async fn role_edges_from_mysql(pool: &MySqlPool) -> Vec<RoleMembership> {
    let rows = match sqlx::raw_sql(
        "SELECT FROM_USER, FROM_HOST, TO_USER, TO_HOST, WITH_ADMIN_OPTION FROM mysql.role_edges",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };

    rows.iter()
        .map(|row| RoleMembership {
            role_user: text(row, "FROM_USER"),
            role_host: text(row, "FROM_HOST"),
            member_user: text(row, "TO_USER"),
            member_host: text(row, "TO_HOST"),
            admin_option: flag(row, "WITH_ADMIN_OPTION"),
        })
        .collect()
}

/// `mysql.roles_mapping` (MariaDB): `User`/`Host` is the member and `Role` the role it holds.
/// MariaDB records neither an admin option nor the role's own host, so the edge carries `%` - the
/// host an unqualified role resolves to in `GRANT <role> TO <member>`.
async fn role_edges_from_mariadb(pool: &MySqlPool) -> Vec<RoleMembership> {
    let rows = match sqlx::raw_sql("SELECT Host, User, Role FROM mysql.roles_mapping")
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };

    rows.iter()
        .filter_map(|row| {
            let role = text(row, "Role");
            if role.is_empty() {
                return None;
            }
            Some(RoleMembership {
                role_user: role,
                role_host: "%".to_string(),
                member_user: text(row, "User"),
                member_host: text(row, "Host"),
                admin_option: false,
            })
        })
        .collect()
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
    // A role has neither a password nor account options: creating it is the whole account group,
    // and every privilege/role change is written by the groups below.
    if account.kind == AccountKind::Role {
        return if is_new {
            vec![format!("CREATE ROLE {target}")]
        } else {
            Vec::new()
        };
    }
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
    let removed: Vec<String> = original
        .difference(current)
        .filter(|privilege| !privilege.matches("GRANT OPTION"))
        .map(|privilege| privilege.as_str().to_string())
        .collect();
    if !removed.is_empty() {
        statements.push(format!(
            "REVOKE {} ON *.* FROM {}",
            removed.join(", "),
            target
        ));
    }
    if has_grant_option(&original) && !has_grant_option(current) {
        statements.push(format!("REVOKE GRANT OPTION ON *.* FROM {target}"));
    }

    let grant_option = has_grant_option(current);
    let mut added: Vec<String> = current
        .difference(&original)
        .filter(|privilege| !privilege.matches("GRANT OPTION"))
        .map(|privilege| privilege.as_str().to_string())
        .collect();
    let grant_option_added = grant_option && !has_grant_option(&original);
    if added.is_empty() && !grant_option_added {
        return statements;
    }
    if added.is_empty() {
        added.push("USAGE".to_string());
    }
    let mut statement = format!("GRANT {} ON *.* TO {target}", added.join(", "));
    if grant_option {
        statement.push_str(" WITH GRANT OPTION");
    }
    statements.push(statement);
    statements
}

fn object_grant_statements(edit: &UserEdit, target: &str) -> Vec<String> {
    let original: BTreeMap<(String, String), BTreeSet<PrivilegeId>> = edit
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
    let current: BTreeMap<(String, String), BTreeSet<PrivilegeId>> = edit
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

        let removed: Vec<String> = old
            .difference(&new)
            .filter(|privilege| !privilege.matches("GRANT OPTION"))
            .map(|privilege| privilege.as_str().to_string())
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
            if has_grant_option(&old) {
                statements.push(format!("REVOKE GRANT OPTION ON {} FROM {}", object, target));
            }
            continue;
        }

        if has_grant_option(&old) && !has_grant_option(&new) {
            statements.push(format!("REVOKE GRANT OPTION ON {} FROM {}", object, target));
        }

        let grant_option = has_grant_option(&new);
        let mut added: Vec<String> = new
            .difference(&old)
            .filter(|privilege| !privilege.matches("GRANT OPTION"))
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        let grant_option_added = grant_option && !has_grant_option(&old);
        if added.is_empty() && !grant_option_added {
            continue;
        }
        if added.is_empty() {
            added.push("USAGE".to_string());
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

/// An unsigned nullable integer column. SQLx's checked `try_get::<i64>` rejects an unsigned
/// column, so the `unsigned`-aware decoders are tried first (see `decode_cell`).
fn opt_uint(row: &MySqlRow, name: &str) -> Option<u64> {
    if let Ok(value) = row.try_get::<Option<u64>, _>(name) {
        return value;
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(name) {
        return value.map(|value| value.max(0) as u64);
    }
    None
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
        let sql = format!(
            "SELECT GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM information_schema.SCHEMA_PRIVILEGES \
             WHERE TABLE_SCHEMA = {}",
            quote_string(database)
        );
        sqlx::raw_sql(AssertSqlSafe(sql))
            .fetch_all(pool)
            .await
            .map_err(map_query_error)?
    } else {
        let sql = format!(
            "SELECT GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM information_schema.TABLE_PRIVILEGES \
             WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {}",
            quote_string(database),
            quote_string(name)
        );
        sqlx::raw_sql(AssertSqlSafe(sql))
            .fetch_all(pool)
            .await
            .map_err(map_query_error)?
    };

    let mut merged: BTreeMap<(String, String), BTreeSet<PrivilegeId>> = BTreeMap::new();
    for row in &rows {
        let granted = row_privileges(
            &text(row, "PRIVILEGE_TYPE"),
            text(row, "IS_GRANTABLE").eq_ignore_ascii_case("YES"),
        );
        if granted.is_empty() {
            continue;
        }
        let (user, host) = parse_grantee(&text(row, "GRANTEE"));
        merged.entry((user, host)).or_default().extend(granted);
    }

    Ok(merged
        .into_iter()
        .map(|((user, host), privileges)| ObjectPrivilegeRow {
            user,
            host,
            privileges,
            // MySQL has no DENY concept.
            denied: BTreeSet::new(),
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
    let current: BTreeMap<(String, String), BTreeSet<PrivilegeId>> =
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
    let current: BTreeMap<(String, String), BTreeSet<PrivilegeId>> = original
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
    current: &BTreeMap<(String, String), BTreeSet<PrivilegeId>>,
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

        let removed: Vec<String> = old
            .difference(new)
            .filter(|privilege| !privilege.matches("GRANT OPTION"))
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {} FROM {}",
                removed.join(", "),
                object,
                target
            ));
        }
        if has_grant_option(&old) && !has_grant_option(new) {
            statements.push(format!("REVOKE GRANT OPTION ON {} FROM {}", object, target));
        }

        let grant_option = has_grant_option(new);
        let mut added: Vec<String> = new
            .difference(&old)
            .filter(|privilege| !privilege.matches("GRANT OPTION"))
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        let grant_option_added = grant_option && !has_grant_option(&old);
        if added.is_empty() && !grant_option_added {
            continue;
        }
        if added.is_empty() {
            added.push("USAGE".to_string());
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
        let removed: Vec<String> = old
            .iter()
            .filter(|privilege| !privilege.matches("GRANT OPTION"))
            .map(|privilege| privilege.as_str().to_string())
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
        if has_grant_option(old) {
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

    fn p(name: &str) -> PrivilegeId {
        PrivilegeId::new(name)
    }

    fn row(user: &str, host: &str, privileges: &[PrivilegeId]) -> ObjectPrivilegeRow {
        ObjectPrivilegeRow {
            user: user.to_string(),
            host: host.to_string(),
            privileges: privileges.iter().cloned().collect(),
            denied: BTreeSet::new(),
        }
    }

    fn current(
        entries: &[(&str, &str, &[PrivilegeId])],
    ) -> BTreeMap<(String, String), BTreeSet<PrivilegeId>> {
        entries
            .iter()
            .map(|(user, host, privileges)| {
                (
                    (user.to_string(), host.to_string()),
                    privileges.iter().cloned().collect(),
                )
            })
            .collect()
    }

    #[test]
    fn reads_a_privilege_with_its_grant_option() {
        assert_eq!(row_privileges("SELECT", false), vec![p("SELECT")]);
        assert_eq!(
            row_privileges("SELECT", true),
            vec![p("SELECT"), p("GRANT OPTION")]
        );
    }

    #[test]
    fn keeps_a_database_the_account_only_holds_an_entry_for() {
        // `GRANT USAGE ON db.* WITH GRANT OPTION` is the entry an account has on a database it
        // cannot read: MySQL reports the row as `USAGE` (no known privilege) plus a grant option.
        assert_eq!(row_privileges("USAGE", true), vec![p("GRANT OPTION")]);
    }

    #[test]
    fn drops_a_usage_row_that_carries_no_grant_option() {
        assert!(row_privileges("USAGE", false).is_empty());
    }

    #[test]
    fn drops_a_dynamic_privilege_the_catalog_does_not_know() {
        assert!(row_privileges("SYSTEM_VARIABLES_ADMIN", false).is_empty());
    }

    #[test]
    fn grants_new_privileges_on_a_database() {
        let statements = object_privilege_statements(
            "shop",
            "",
            &current(&[]),
            &[row("alice", "%", &[p("SELECT"), p("INSERT")])],
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
            &current(&[("alice", "%", &[p("SELECT"), p("INSERT")])]),
            &[row("alice", "%", &[p("SELECT")])],
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
            &current(&[("alice", "%", &[p("SELECT"), p("GRANT OPTION")])]),
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
                ("alice", "%", &[p("SELECT")]),
                ("bob", "localhost", &[p("INSERT")]),
            ]),
            &[row("alice", "%", &[p("SELECT")])],
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
                schema: String::new(),
                name: "t".to_string(),
                privileges: [p("SELECT"), p("GRANT OPTION")].into_iter().collect(),
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
            server_privileges: [p("SELECT")].into_iter().collect(),
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
            server_privileges: [p("INSERT")].into_iter().collect(),
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

    #[test]
    fn assumes_every_account_column_when_the_catalog_is_unreadable() {
        let shape = CatalogShape::assumed_complete();
        let projection = shape.account_projection();
        assert!(projection.starts_with("User, Host, plugin"));
        assert!(projection.contains("x509_subject"));
        // Every column is projected as itself, except `is_role`, which MySQL 8 simply lacks.
        assert_eq!(projection.matches("NULL AS").count(), 1);
        assert!(projection.ends_with(", NULL AS is_role"));
        assert!(shape.role_edges && !shape.roles_mapping);
    }

    #[test]
    fn projects_the_account_columns_a_server_lacks_as_null() {
        // MySQL 5.6 has neither `password_lifetime` nor `account_locked`; the read must still run
        // and report those attributes as absent instead of failing the whole Users tab.
        let shape = CatalogShape {
            user_columns: ["User", "Host", "plugin", "authentication_string"]
                .iter()
                .map(|name| name.to_string())
                .collect(),
            is_role_column: false,
            role_edges: false,
            roles_mapping: true,
        };
        let projection = shape.account_projection();
        assert!(projection.starts_with("User, Host, plugin, authentication_string, "));
        assert!(projection.contains("NULL AS password_lifetime"));
        assert!(projection.contains("NULL AS super_priv"));
        // Every column the editor reads is projected once, either as itself or as NULL, plus the
        // `is_role` marker the reader uses to recognise a role.
        assert_eq!(
            projection.matches("NULL AS").count(),
            ACCOUNT_COLUMNS.len() - 4 + 1
        );
    }

    #[test]
    fn a_server_with_is_role_reads_it_directly() {
        // MariaDB tells roles apart itself, so the projection selects the real column.
        let shape = CatalogShape {
            user_columns: ACCOUNT_COLUMNS
                .iter()
                .map(|name| name.to_string())
                .chain(std::iter::once("is_role".to_string()))
                .collect(),
            is_role_column: true,
            role_edges: false,
            roles_mapping: true,
        };
        assert!(shape.account_projection().ends_with(", is_role"));
        assert_eq!(shape.account_projection().matches("NULL AS").count(), 0);
    }

    /// A `UserEdit` for one account with nothing else set, so a test only states what it exercises.
    fn edit_for(original: Option<UserDetails>, account: UserAccount) -> UserEdit {
        UserEdit {
            original,
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

    #[test]
    fn a_new_role_is_created_with_create_role() {
        // A role carries no password and no account options, so `CREATE ROLE` is the whole account
        // group rather than the `CREATE USER … IDENTIFIED WITH …` a user gets.
        let edit = edit_for(
            None,
            UserAccount {
                user: "reporting".to_string(),
                host: "%".to_string(),
                kind: AccountKind::Role,
                ..Default::default()
            },
        );
        let statements = edit_statements(&edit);
        assert_eq!(
            statements.first().map(String::as_str),
            Some("CREATE ROLE 'reporting'@'%'")
        );
        assert_eq!(
            statements.last().map(String::as_str),
            Some("FLUSH PRIVILEGES")
        );
    }

    #[test]
    fn an_existing_role_keeps_its_account_statements_empty() {
        // Editing a role must not fall back to `ALTER USER`, which would rewrite it as an account.
        let account = UserAccount {
            user: "reporting".to_string(),
            host: "%".to_string(),
            kind: AccountKind::Role,
            ..Default::default()
        };
        let original = UserDetails {
            account: account.clone(),
            ..Default::default()
        };
        let edit = edit_for(Some(original), account);
        assert!(edit_groups(&edit).is_empty(), "{:?}", edit_groups(&edit));
    }
}

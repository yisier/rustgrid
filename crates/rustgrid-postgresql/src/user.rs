//! PostgreSQL role (account) catalog access and `CREATE/ALTER/DROP ROLE` generation.
//!
//! PostgreSQL has no `user@host` model: roles are server-wide. `UserAccount::host` is therefore
//! always empty. Object privileges are read from the ACL (`aclexplode`) rather than
//! `information_schema`, which only exposes grants related to the current user.

use std::collections::{BTreeMap, BTreeSet};

use rustgrid_core::{
    DefaultObjectType, DefaultPrivilege, DefaultPrivilegeInfo, Error, ObjectGrant,
    ObjectPrivilegeRow, PrivilegeCatalog, PrivilegeGroup, PrivilegeId, PrivilegeInfo,
    PrivilegePreset, Result, RoleMembership, UserAccount, UserDetails, UserEdit, UserEditSection,
};
use sqlx::Row;

use crate::connection::{PostgresConnection, map_query_error};
use crate::helpers::{qualify, quote_identifier, quote_literal, split_qualified};

/// The PostgreSQL role attributes surfaced by the account editor's 服务器权限 grid, as
/// `(attribute keyword, i18n key)`.
const ROLE_ATTRIBUTES: &[(&str, &str)] = &[
    ("SUPERUSER", "user.priv.pg.superuser"),
    ("CREATEDB", "user.priv.pg.createdb"),
    ("CREATEROLE", "user.priv.pg.createrole"),
    ("REPLICATION", "user.priv.pg.replication"),
    ("BYPASSRLS", "user.priv.pg.bypassrls"),
];

/// The PostgreSQL database-level privileges (`ON DATABASE db`), as `(keyword, i18n key)`. `CREATE`
/// is shared with the schema scope and is built separately.
const DATABASE_PRIVILEGES: &[(&str, &str)] = &[
    ("CONNECT", "user.priv.pg.connect"),
    ("TEMPORARY", "user.priv.temporary"),
];

/// The PostgreSQL schema-level privileges (`ON SCHEMA x`), as `(keyword, i18n key)`. `CREATE` is
/// shared with the database scope and is built separately.
const SCHEMA_PRIVILEGES: &[(&str, &str)] = &[("USAGE", "user.priv.usage")];

/// The PostgreSQL table/view privileges surfaced by the 权限 grid, as `(keyword, i18n key, group)`.
const OBJECT_PRIVILEGES: &[(&str, &str, &str)] = &[
    ("SELECT", "user.priv.select", "dml"),
    ("INSERT", "user.priv.insert", "dml"),
    ("UPDATE", "user.priv.update", "dml"),
    ("DELETE", "user.priv.delete", "dml"),
    ("TRUNCATE", "user.priv.truncate", "dml"),
    ("REFERENCES", "user.priv.references", "ddl"),
    ("TRIGGER", "user.priv.trigger", "ddl"),
];

/// The privileges `ALTER DEFAULT PRIVILEGES` offers per object kind, as `(kind, keyword)`. A
/// keyword may repeat across kinds (`SELECT` on tables and sequences), each as its own entry.
const DEFAULT_PRIVILEGES: &[(DefaultObjectType, &str)] = &[
    (DefaultObjectType::Tables, "SELECT"),
    (DefaultObjectType::Tables, "INSERT"),
    (DefaultObjectType::Tables, "UPDATE"),
    (DefaultObjectType::Tables, "DELETE"),
    (DefaultObjectType::Tables, "TRUNCATE"),
    (DefaultObjectType::Tables, "REFERENCES"),
    (DefaultObjectType::Tables, "TRIGGER"),
    (DefaultObjectType::Sequences, "USAGE"),
    (DefaultObjectType::Sequences, "SELECT"),
    (DefaultObjectType::Sequences, "UPDATE"),
    (DefaultObjectType::Functions, "EXECUTE"),
    (DefaultObjectType::Types, "USAGE"),
    (DefaultObjectType::Schemas, "CREATE"),
    (DefaultObjectType::Schemas, "USAGE"),
];

/// The app i18n key for a privilege keyword, when one exists (falling back to the keyword itself).
fn privilege_label_key(keyword: &str) -> Option<&'static str> {
    match keyword {
        "SELECT" => Some("user.priv.select"),
        "INSERT" => Some("user.priv.insert"),
        "UPDATE" => Some("user.priv.update"),
        "DELETE" => Some("user.priv.delete"),
        "TRUNCATE" => Some("user.priv.truncate"),
        "REFERENCES" => Some("user.priv.references"),
        "TRIGGER" => Some("user.priv.trigger"),
        "USAGE" => Some("user.priv.usage"),
        "EXECUTE" => Some("user.priv.execute"),
        "CREATE" => Some("user.priv.create"),
        _ => None,
    }
}

/// The object-kind `"char"` PostgreSQL stores in `pg_default_acl.defaclobjtype`.
fn default_object_type(code: &str) -> Option<DefaultObjectType> {
    match code {
        "r" => Some(DefaultObjectType::Tables),
        "S" => Some(DefaultObjectType::Sequences),
        "f" => Some(DefaultObjectType::Functions),
        "T" => Some(DefaultObjectType::Types),
        "n" => Some(DefaultObjectType::Schemas),
        _ => None,
    }
}

/// The PostgreSQL privilege catalog: role attributes (server), database/schema/table privileges,
/// their groups and the quick presets.
pub(crate) fn privilege_catalog() -> PrivilegeCatalog {
    let groups = [
        ("attributes", "user.create.server_group.attributes"),
        ("database", "user.create.priv_group.database"),
        ("schema", "user.create.priv_group.schema"),
        ("dml", "user.create.priv_group.dml"),
        ("ddl", "user.create.priv_group.ddl"),
    ]
    .into_iter()
    .map(|(id, label_key)| PrivilegeGroup {
        id: id.to_string(),
        label_key: Some(label_key.to_string()),
        label: id.to_string(),
    })
    .collect();

    let mut privileges: Vec<PrivilegeInfo> = ROLE_ATTRIBUTES
        .iter()
        .map(|(keyword, label_key)| {
            PrivilegeInfo::server(*keyword, "attributes", Some(*label_key), keyword)
        })
        .collect();
    privileges.extend(DATABASE_PRIVILEGES.iter().map(|(keyword, label_key)| {
        PrivilegeInfo::database(*keyword, "database", Some(*label_key), keyword)
    }));
    privileges.push(PrivilegeInfo::database_and_schema(
        "CREATE",
        "database",
        "schema",
        Some("user.priv.create"),
        "CREATE",
    ));
    privileges.extend(SCHEMA_PRIVILEGES.iter().map(|(keyword, label_key)| {
        PrivilegeInfo::schema(*keyword, "schema", Some(*label_key), keyword)
    }));
    privileges.extend(OBJECT_PRIVILEGES.iter().map(|(keyword, label_key, group)| {
        PrivilegeInfo::object(*keyword, group, Some(*label_key), keyword)
    }));

    PrivilegeCatalog {
        groups,
        privileges,
        server_presets: vec![
            preset("none", "user.create.template.none", &[]),
            preset(
                "admin",
                "user.create.template.admin",
                &ROLE_ATTRIBUTES
                    .iter()
                    .map(|(keyword, _)| *keyword)
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
                    .map(|(keyword, _, _)| *keyword)
                    .collect::<Vec<_>>(),
            ),
        ],
        default_privileges: DEFAULT_PRIVILEGES
            .iter()
            .map(|(object_type, keyword)| {
                DefaultPrivilegeInfo::new(
                    *object_type,
                    *keyword,
                    privilege_label_key(keyword),
                    keyword,
                )
            })
            .collect(),
        deny_supported: false,
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

/// Whether a privilege set contains the role attribute `name`.
fn has(privileges: &BTreeSet<PrivilegeId>, name: &str) -> bool {
    privileges.iter().any(|privilege| privilege.matches(name))
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
        password_valid_until: row.try_get("valid_until").ok().flatten(),
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
        default_tablespace: String::new(),
        profile: String::new(),
        tablespace_quota: String::new(),
        is_super_user: row.try_get("rolsuper").unwrap_or(false),
        ..UserAccount::default()
    }
}

/// `rolvaliduntil` is cast to text so it can be shown and re-rendered as a `VALID UNTIL` literal.
const ACCOUNT_COLUMNS: &str = "rolname, rolsuper, rolcreaterole, rolcreatedb, rolcanlogin, \
     rolreplication, rolbypassrls, rolconnlimit, rolvaliduntil::text AS valid_until, \
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

    let mut server_privileges: BTreeSet<PrivilegeId> = BTreeSet::new();
    if row.try_get::<bool, _>("rolsuper").unwrap_or(false) {
        server_privileges.insert(PrivilegeId::new("SUPERUSER"));
    }
    if row.try_get::<bool, _>("rolcreaterole").unwrap_or(false) {
        server_privileges.insert(PrivilegeId::new("CREATEROLE"));
    }
    if row.try_get::<bool, _>("rolcreatedb").unwrap_or(false) {
        server_privileges.insert(PrivilegeId::new("CREATEDB"));
    }
    if row.try_get::<bool, _>("rolreplication").unwrap_or(false) {
        server_privileges.insert(PrivilegeId::new("REPLICATION"));
    }
    if row.try_get::<bool, _>("rolbypassrls").unwrap_or(false) {
        server_privileges.insert(PrivilegeId::new("BYPASSRLS"));
    }

    let roles = role_edges(&pool, "am.member", user).await?;
    let members = role_edges(&pool, "am.roleid", user).await?;
    let grants = object_grants(&pool, connection.default_database_name(), user).await?;
    let default_privileges = default_privileges(&pool, user).await?;

    Ok(UserDetails {
        account,
        server_privileges,
        denied_server_privileges: BTreeSet::new(),
        grants,
        default_privileges,
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

/// Object grants visible in the relation ACLs for `user`, grouped by object. `database` is the
/// database the ACLs live in (PostgreSQL grants are per-database), `schema` the object's namespace.
async fn object_grants(
    pool: &sqlx::PgPool,
    database: &str,
    user: &str,
) -> Result<Vec<ObjectGrant>> {
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
        let privilege = PrivilegeId::new(privilege_type.trim().to_ascii_uppercase());
        if privilege.as_str().is_empty() {
            continue;
        }
        if let Some(existing) = grants
            .iter_mut()
            .find(|grant| grant.schema == schema && grant.name == name)
        {
            existing.privileges.insert(privilege);
        } else {
            let mut grant = ObjectGrant {
                database: database.to_string(),
                schema,
                name,
                privileges: BTreeSet::new(),
            };
            grant.privileges.insert(privilege);
            grants.push(grant);
        }
    }
    Ok(grants)
}

/// The account's `ALTER DEFAULT PRIVILEGES` rules, read from `pg_default_acl`. `PUBLIC` grants
/// (`grantee = 0`) are skipped: the editor grants to roles only, and leaving them out means an
/// untouched `PUBLIC` rule is never revoked on save.
async fn default_privileges(pool: &sqlx::PgPool, user: &str) -> Result<Vec<DefaultPrivilege>> {
    let rows = sqlx::query(
        "SELECT COALESCE(n.nspname, ''), d.defaclobjtype::text, a.grantee::regrole::text, \
                a.privilege_type \
         FROM pg_default_acl d \
         LEFT JOIN pg_namespace n ON n.oid = d.defaclnamespace \
         CROSS JOIN LATERAL aclexplode(d.defaclacl) AS a \
         WHERE d.defaclrole = (SELECT oid FROM pg_roles WHERE rolname = $1) \
           AND a.grantee <> 0 \
         ORDER BY 1, 2, 3",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .map_err(map_query_error)?;

    let mut rules: Vec<DefaultPrivilege> = Vec::new();
    for row in &rows {
        let schema: String = row.try_get(0).unwrap_or_default();
        let code: String = row.try_get(1).unwrap_or_default();
        let Some(object_type) = default_object_type(&code) else {
            continue;
        };
        let grantee: String = row.try_get(2).unwrap_or_default();
        let privilege_type: String = row.try_get(3).unwrap_or_default();
        let privilege = PrivilegeId::new(privilege_type.trim().to_ascii_uppercase());
        if grantee.is_empty() || privilege.as_str().is_empty() {
            continue;
        }
        if let Some(existing) = rules.iter_mut().find(|rule| {
            rule.schema == schema && rule.object_type == object_type && rule.grantee == grantee
        }) {
            existing.privileges.insert(privilege);
        } else {
            let mut rule = DefaultPrivilege::new(schema, object_type, grantee);
            rule.privileges.insert(privilege);
            rules.push(rule);
        }
    }
    Ok(rules)
}

/// The schemas the account editor's 默认权限 section lists: the user schemas of the database the
/// driver reads and writes default privileges in.
pub(crate) async fn default_privilege_schemas(
    connection: &PostgresConnection,
) -> Result<Vec<String>> {
    let pool = connection
        .pool_for(connection.default_database_name())
        .await?;
    let rows = sqlx::query(
        "SELECT nspname FROM pg_namespace \
         WHERE nspname NOT LIKE 'pg\\_%' AND nspname <> 'information_schema' \
         ORDER BY nspname",
    )
    .fetch_all(&pool)
    .await
    .map_err(map_query_error)?;
    rows.iter()
        .map(|row| row.try_get(0).map_err(map_query_error))
        .collect()
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

    if !edit.default_privileges.is_empty()
        || edit
            .original
            .as_ref()
            .is_some_and(|o| !o.default_privileges.is_empty())
    {
        let statements = default_privilege_statements(edit);
        if !statements.is_empty() {
            groups.push((UserEditSection::DefaultPrivileges, statements));
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
        if has(&edit.server_privileges, "CREATEROLE") {
            "CREATEROLE"
        } else {
            "NOCREATEROLE"
        }
        .to_string(),
    );
    parts.push(
        if has(&edit.server_privileges, "CREATEDB") {
            "CREATEDB"
        } else {
            "NOCREATEDB"
        }
        .to_string(),
    );
    parts.push(
        if has(&edit.server_privileges, "REPLICATION") {
            "REPLICATION"
        } else {
            "NOREPLICATION"
        }
        .to_string(),
    );
    parts.push(
        if has(&edit.server_privileges, "BYPASSRLS") {
            "BYPASSRLS"
        } else {
            "NOBYPASSRLS"
        }
        .to_string(),
    );
    // PostgreSQL's password expiry. Only emit it when it changes: setting a value, or clearing a
    // previously-set one with `infinity`.
    let valid_until = account
        .password_valid_until
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let had_valid_until = edit
        .original
        .as_ref()
        .and_then(|original| original.account.password_valid_until.as_deref())
        .is_some_and(|value| !value.trim().is_empty());
    if let Some(value) = valid_until {
        parts.push(format!("VALID UNTIL {}", quote_literal(value)));
    } else if had_valid_until {
        parts.push("VALID UNTIL 'infinity'".to_string());
    }
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
    for grant in &edit.grants {
        let before = original
            .get(&grant.object_name())
            .copied()
            .unwrap_or(&empty);
        let (schema, name) = if grant.schema.is_empty() {
            object_parts(&grant.name)
        } else {
            (grant.schema.clone(), grant.name.clone())
        };
        let qualified = qualify(&schema, &name);
        let added: Vec<String> = grant
            .privileges
            .difference(before)
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        if !added.is_empty() {
            statements.push(format!(
                "GRANT {} ON TABLE {qualified} TO {user}",
                added.join(", ")
            ));
        }
        let removed: Vec<String> = before
            .difference(&grant.privileges)
            .map(|privilege| privilege.as_str().to_string())
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

/// The `ALTER DEFAULT PRIVILEGES` statements for the rules that changed, keyed by
/// `(schema, object kind, grantee)`.
fn default_privilege_statements(edit: &UserEdit) -> Vec<String> {
    let role = quote_identifier(&edit.account.user);
    let mut before: BTreeMap<(String, DefaultObjectType, String), BTreeSet<PrivilegeId>> =
        BTreeMap::new();
    if let Some(original) = &edit.original {
        for rule in &original.default_privileges {
            before
                .entry(rule.key())
                .or_default()
                .extend(rule.privileges.iter().cloned());
        }
    }
    let mut after: BTreeMap<(String, DefaultObjectType, String), BTreeSet<PrivilegeId>> =
        BTreeMap::new();
    for rule in &edit.default_privileges {
        after
            .entry(rule.key())
            .or_default()
            .extend(rule.privileges.iter().cloned());
    }

    let keys: BTreeSet<(String, DefaultObjectType, String)> =
        before.keys().chain(after.keys()).cloned().collect();
    let mut statements = Vec::new();
    for (schema, object_type, grantee) in keys {
        let empty = BTreeSet::new();
        let old = before
            .get(&(schema.clone(), object_type, grantee.clone()))
            .unwrap_or(&empty);
        let new = after
            .get(&(schema.clone(), object_type, grantee.clone()))
            .unwrap_or(&empty);
        let added: Vec<String> = new
            .difference(old)
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        let removed: Vec<String> = old
            .difference(new)
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        if added.is_empty() && removed.is_empty() {
            continue;
        }
        // `ALTER DEFAULT PRIVILEGES FOR ROLE x [IN SCHEMA s] …` targets the edited role's future
        // objects; the connection's own role must not be used (the editor usually runs as an admin).
        let mut target = format!("ALTER DEFAULT PRIVILEGES FOR ROLE {role}");
        if !schema.is_empty() {
            target.push_str(&format!(" IN SCHEMA {}", quote_identifier(&schema)));
        }
        if !added.is_empty() {
            statements.push(format!(
                "{target} GRANT {} ON {} TO {}",
                added.join(", "),
                object_type.keyword(),
                quote_identifier(&grantee)
            ));
        }
        if !removed.is_empty() {
            statements.push(format!(
                "{target} REVOKE {} ON {} FROM {}",
                removed.join(", "),
                object_type.keyword(),
                quote_identifier(&grantee)
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

/// The privilege manager's selected target: the whole database, a schema (`name` = `schema.*`), or
/// a table/view (`name` = `schema.table`).
enum Target {
    Database,
    Schema(String),
    Table(String, String),
}

fn classify(name: &str) -> Target {
    if name.is_empty() {
        Target::Database
    } else if let Some(schema) = name.strip_suffix(".*") {
        Target::Schema(schema.to_string())
    } else {
        let (schema, table) = object_parts(name);
        Target::Table(schema, table)
    }
}

/// The `ON <target>` clause: `DATABASE db`, `SCHEMA s`, or `TABLE s.t`.
fn grant_target(database: &str, name: &str) -> String {
    match classify(name) {
        Target::Database => format!("DATABASE {}", quote_identifier(database)),
        Target::Schema(schema) => format!("SCHEMA {}", quote_identifier(&schema)),
        Target::Table(schema, table) => format!("TABLE {}", qualify(&schema, &table)),
    }
}

pub(crate) async fn object_privilege_matrix(
    connection: &PostgresConnection,
    database: &str,
    name: &str,
) -> Result<Vec<ObjectPrivilegeRow>> {
    let pool = connection.pool_for(database).await?;
    let rows = match classify(name) {
        Target::Database => sqlx::query(
            "SELECT r.rolname, a.privilege_type \
             FROM pg_database d \
             CROSS JOIN LATERAL aclexplode(d.datacl) AS a \
             JOIN pg_roles r ON r.oid = a.grantee \
             WHERE d.datname = $1",
        )
        .bind(database)
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?,
        Target::Schema(schema) => sqlx::query(
            "SELECT r.rolname, a.privilege_type \
             FROM pg_namespace n \
             CROSS JOIN LATERAL aclexplode(n.nspacl) AS a \
             JOIN pg_roles r ON r.oid = a.grantee \
             WHERE n.nspname = $1",
        )
        .bind(&schema)
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?,
        Target::Table(schema, table) => sqlx::query(
            "SELECT r.rolname, a.privilege_type \
             FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             CROSS JOIN LATERAL aclexplode(c.relacl) AS a \
             JOIN pg_roles r ON r.oid = a.grantee \
             WHERE n.nspname = $1 AND c.relname = $2",
        )
        .bind(&schema)
        .bind(&table)
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?,
    };

    let mut result: Vec<ObjectPrivilegeRow> = Vec::new();
    for row in &rows {
        let role: String = row.try_get(0).unwrap_or_default();
        let privilege_type: String = row.try_get(1).unwrap_or_default();
        let privilege = PrivilegeId::new(privilege_type.trim().to_ascii_uppercase());
        if privilege.as_str().is_empty() {
            continue;
        }
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
    database: &str,
    name: &str,
    original: &[ObjectPrivilegeRow],
    rows: &[ObjectPrivilegeRow],
) -> String {
    let target = grant_target(database, name);
    let mut statements = Vec::new();
    for row in rows {
        let before = original
            .iter()
            .find(|entry| entry.user == row.user)
            .map(|entry| &entry.privileges)
            .cloned()
            .unwrap_or_default();
        let role = quote_identifier(&row.user);
        let added: Vec<String> = row
            .privileges
            .difference(&before)
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        if !added.is_empty() {
            statements.push(format!("GRANT {} ON {target} TO {role}", added.join(", ")));
        }
        let removed: Vec<String> = before
            .difference(&row.privileges)
            .map(|privilege| privilege.as_str().to_string())
            .collect();
        if !removed.is_empty() {
            statements.push(format!(
                "REVOKE {} ON {target} FROM {role}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_targets_cover_database_schema_and_table() {
        assert_eq!(grant_target("shop", ""), "DATABASE \"shop\"");
        assert_eq!(grant_target("shop", "public.*"), "SCHEMA \"public\"");
        assert_eq!(
            grant_target("shop", "public.users"),
            "TABLE \"public\".\"users\""
        );
        // A bare table name defaults to the `public` schema.
        assert_eq!(grant_target("shop", "users"), "TABLE \"public\".\"users\"");
    }

    fn rule(
        schema: &str,
        object_type: DefaultObjectType,
        grantee: &str,
        privileges: &[&str],
    ) -> DefaultPrivilege {
        DefaultPrivilege {
            schema: schema.to_string(),
            object_type,
            grantee: grantee.to_string(),
            privileges: privileges.iter().map(|p| PrivilegeId::new(*p)).collect(),
        }
    }

    fn account(name: &str) -> UserAccount {
        UserAccount {
            user: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn default_privilege_rules_diff_into_grant_and_revoke() {
        let original = UserDetails {
            account: account("alice"),
            default_privileges: vec![
                rule("public", DefaultObjectType::Tables, "bob", &["SELECT"]),
                rule("", DefaultObjectType::Sequences, "bob", &["USAGE"]),
            ],
            ..Default::default()
        };
        let edit = UserEdit {
            original: Some(original),
            account: account("alice"),
            password: None,
            server_privileges: BTreeSet::new(),
            denied_server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            default_privileges: vec![
                rule(
                    "public",
                    DefaultObjectType::Tables,
                    "bob",
                    &["SELECT", "INSERT"],
                ),
                // The sequence rule is dropped: its USAGE is revoked. A second grantee is added.
                rule("public", DefaultObjectType::Tables, "carol", &["SELECT"]),
            ],
            roles: Vec::new(),
            members: Vec::new(),
            mappings: Vec::new(),
            original_mappings: Vec::new(),
            old_password: None,
            securables: Vec::new(),
            original_securables: Vec::new(),
        };
        assert_eq!(
            default_privilege_statements(&edit),
            vec![
                "ALTER DEFAULT PRIVILEGES FOR ROLE \"alice\" REVOKE USAGE ON SEQUENCES FROM \"bob\"",
                "ALTER DEFAULT PRIVILEGES FOR ROLE \"alice\" IN SCHEMA \"public\" GRANT INSERT ON TABLES TO \"bob\"",
                "ALTER DEFAULT PRIVILEGES FOR ROLE \"alice\" IN SCHEMA \"public\" GRANT SELECT ON TABLES TO \"carol\"",
            ]
        );
    }
}

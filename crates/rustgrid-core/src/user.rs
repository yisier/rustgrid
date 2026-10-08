//! Engine-agnostic models for account (user/role) administration and the privileges that can be
//! granted to an account. The UI only ever sees these types; each driver exposes a
//! [`PrivilegeCatalog`] describing its own privileges and maps them to its own grant syntax.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A privilege's identity: the engine keyword used in a `GRANT` statement (e.g. `"SELECT"`,
/// `"CONTROL SERVER"`, `"CREATE ANY TABLE"`). Compared case-insensitively.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PrivilegeId(String);

impl PrivilegeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The engine keyword.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `name` is this privilege, ignoring case and surrounding whitespace.
    pub fn matches(&self, name: &str) -> bool {
        self.0.eq_ignore_ascii_case(name.trim())
    }
}

impl From<&str> for PrivilegeId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for PrivilegeId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for PrivilegeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a privilege can be granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeScope {
    /// Server-wide (`*.*` in MySQL, `SERVER` in SQL Server, system privileges in Oracle).
    Server,
    /// On a whole database (`ON DATABASE::db` in SQL Server, `ON DATABASE db` in PostgreSQL).
    /// Engines whose database-wide grants reuse their object privileges (MySQL's `db.*`) leave
    /// this empty.
    Database,
    /// On a schema (`ON SCHEMA public` in PostgreSQL).
    Schema,
    /// On a table/view/routine.
    Object,
}

/// The object kind a default-privileges rule applies to (PostgreSQL's `ALTER DEFAULT PRIVILEGES
/// ... ON TABLES|SEQUENCES|FUNCTIONS|TYPES|SCHEMAS`). The engine keyword is the variant's
/// [`DefaultObjectType::keyword`], so the model needs no per-engine knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DefaultObjectType {
    Tables,
    Sequences,
    Functions,
    Types,
    Schemas,
}

impl DefaultObjectType {
    /// Every kind, in catalog display order.
    pub const ALL: [DefaultObjectType; 5] = [
        DefaultObjectType::Tables,
        DefaultObjectType::Sequences,
        DefaultObjectType::Functions,
        DefaultObjectType::Types,
        DefaultObjectType::Schemas,
    ];

    /// The `ON <keyword>` object kind.
    pub fn keyword(self) -> &'static str {
        match self {
            DefaultObjectType::Tables => "TABLES",
            DefaultObjectType::Sequences => "SEQUENCES",
            DefaultObjectType::Functions => "FUNCTIONS",
            DefaultObjectType::Types => "TYPES",
            DefaultObjectType::Schemas => "SCHEMAS",
        }
    }
}

/// One privilege a default-privileges rule can grant, for one object kind. A privilege may apply to
/// several kinds (`SELECT` is valid on tables and sequences), each as its own entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultPrivilegeInfo {
    pub object_type: DefaultObjectType,
    pub id: PrivilegeId,
    /// The app i18n key, when the app knows one.
    pub label_key: Option<String>,
    /// Fallback display label (usually the engine keyword).
    pub label: String,
}

impl DefaultPrivilegeInfo {
    pub fn new(
        object_type: DefaultObjectType,
        id: impl Into<PrivilegeId>,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            object_type,
            id: id.into(),
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }
}

/// One privilege in a driver's catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegeInfo {
    pub id: PrivilegeId,
    /// The scopes it can be granted at (a privilege may be both server- and object-level).
    pub scopes: Vec<PrivilegeScope>,
    /// The `(scope, group id)` pairs this privilege belongs to. The grouping can differ per scope
    /// (MySQL puts `ALTER ROUTINE` under 定义 for server privileges but under 例程 for objects).
    pub groups: Vec<(PrivilegeScope, String)>,
    /// The app i18n key, when the app knows one.
    pub label_key: Option<String>,
    /// Fallback display label (usually the engine keyword).
    pub label: String,
}

impl PrivilegeInfo {
    /// A server-only privilege in `group`.
    pub fn server(
        id: impl Into<PrivilegeId>,
        group: &str,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            id: id.into(),
            scopes: vec![PrivilegeScope::Server],
            groups: vec![(PrivilegeScope::Server, group.to_string())],
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }

    /// An object-only privilege in `group`.
    pub fn object(
        id: impl Into<PrivilegeId>,
        group: &str,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            id: id.into(),
            scopes: vec![PrivilegeScope::Object],
            groups: vec![(PrivilegeScope::Object, group.to_string())],
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }

    /// A database-only privilege in `group` (SQL Server's `ON DATABASE::db` permissions).
    pub fn database(
        id: impl Into<PrivilegeId>,
        group: &str,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            id: id.into(),
            scopes: vec![PrivilegeScope::Database],
            groups: vec![(PrivilegeScope::Database, group.to_string())],
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }

    /// A schema-only privilege in `group` (PostgreSQL's `ON SCHEMA`).
    pub fn schema(
        id: impl Into<PrivilegeId>,
        group: &str,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            id: id.into(),
            scopes: vec![PrivilegeScope::Schema],
            groups: vec![(PrivilegeScope::Schema, group.to_string())],
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }

    /// A privilege grantable on both a database and a schema, each in its own group.
    pub fn database_and_schema(
        id: impl Into<PrivilegeId>,
        database_group: &str,
        schema_group: &str,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            id: id.into(),
            scopes: vec![PrivilegeScope::Database, PrivilegeScope::Schema],
            groups: vec![
                (PrivilegeScope::Database, database_group.to_string()),
                (PrivilegeScope::Schema, schema_group.to_string()),
            ],
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }

    /// A privilege grantable on both a whole database and an object, each in its own group.
    pub fn database_and_object(
        id: impl Into<PrivilegeId>,
        database_group: &str,
        object_group: &str,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            id: id.into(),
            scopes: vec![PrivilegeScope::Database, PrivilegeScope::Object],
            groups: vec![
                (PrivilegeScope::Database, database_group.to_string()),
                (PrivilegeScope::Object, object_group.to_string()),
            ],
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }

    /// A privilege grantable at both scopes, each in its own group.
    pub fn both(
        id: impl Into<PrivilegeId>,
        server_group: &str,
        object_group: &str,
        label_key: Option<&str>,
        label: &str,
    ) -> Self {
        Self {
            id: id.into(),
            scopes: vec![PrivilegeScope::Server, PrivilegeScope::Object],
            groups: vec![
                (PrivilegeScope::Server, server_group.to_string()),
                (PrivilegeScope::Object, object_group.to_string()),
            ],
            label_key: label_key.map(str::to_string),
            label: label.to_string(),
        }
    }

    /// Whether the privilege can be granted server-wide.
    pub fn is_server(&self) -> bool {
        self.scopes.contains(&PrivilegeScope::Server)
    }

    /// Whether the privilege can be granted on a whole database.
    pub fn is_database(&self) -> bool {
        self.scopes.contains(&PrivilegeScope::Database)
    }

    /// Whether the privilege can be granted on a schema.
    pub fn is_schema(&self) -> bool {
        self.scopes.contains(&PrivilegeScope::Schema)
    }

    /// Whether the privilege can be granted on an object.
    pub fn is_object(&self) -> bool {
        self.scopes.contains(&PrivilegeScope::Object)
    }
}

/// A named group of privileges, shown together in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegeGroup {
    pub id: String,
    pub label_key: Option<String>,
    pub label: String,
}

/// A one-click preset: a named set of privileges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegePreset {
    pub id: String,
    pub label_key: Option<String>,
    pub label: String,
    pub privileges: Vec<PrivilegeId>,
}

/// An engine's privilege catalog: the groups, privileges and presets the account editor and the
/// object-privilege manager render. Drivers return their own; the UI never matches on engine id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivilegeCatalog {
    pub groups: Vec<PrivilegeGroup>,
    pub privileges: Vec<PrivilegeInfo>,
    pub server_presets: Vec<PrivilegePreset>,
    pub object_presets: Vec<PrivilegePreset>,
    /// The privileges offered by the default-privileges editor (PostgreSQL's
    /// `ALTER DEFAULT PRIVILEGES`), per object kind. Empty for engines without default privileges.
    pub default_privileges: Vec<DefaultPrivilegeInfo>,
    /// Whether the engine has an explicit deny (`DENY` in SQL Server), which the privilege manager
    /// offers as a per-privilege toggle.
    pub deny_supported: bool,
    /// The server-level securable classes the account editor offers (SQL Server's endpoints and
    /// logins), each with its grantable permissions. Empty for engines without such securables.
    pub securable_classes: Vec<SecurableClass>,
}

impl PrivilegeCatalog {
    /// The privileges offered for one object kind of a default-privileges rule, in catalog order.
    pub fn default_privileges_for(
        &self,
        object_type: DefaultObjectType,
    ) -> Vec<&DefaultPrivilegeInfo> {
        self.default_privileges
            .iter()
            .filter(|info| info.object_type == object_type)
            .collect()
    }

    /// Whether the catalog offers default privileges at all.
    pub fn has_default_privileges(&self) -> bool {
        !self.default_privileges.is_empty()
    }

    /// The privileges that can be granted server-wide.
    pub fn server(&self) -> impl Iterator<Item = &PrivilegeInfo> {
        self.privileges.iter().filter(|info| info.is_server())
    }

    /// The privileges that can be granted on a whole database.
    pub fn database(&self) -> impl Iterator<Item = &PrivilegeInfo> {
        self.privileges.iter().filter(|info| info.is_database())
    }

    /// The privileges that can be granted on a schema.
    pub fn schema(&self) -> impl Iterator<Item = &PrivilegeInfo> {
        self.privileges.iter().filter(|info| info.is_schema())
    }

    /// The privileges that can be granted on an object.
    pub fn object(&self) -> impl Iterator<Item = &PrivilegeInfo> {
        self.privileges.iter().filter(|info| info.is_object())
    }

    /// Every privilege grantable at `scope`, in catalog order.
    pub fn at(&self, scope: PrivilegeScope) -> impl Iterator<Item = &PrivilegeInfo> {
        self.privileges
            .iter()
            .filter(move |info| info.scopes.contains(&scope))
    }

    /// Whether the catalog has any privilege at `scope`.
    pub fn has_scope(&self, scope: PrivilegeScope) -> bool {
        self.privileges
            .iter()
            .any(|info| info.scopes.contains(&scope))
    }

    /// The catalog entry for `id`.
    pub fn info(&self, id: &PrivilegeId) -> Option<&PrivilegeInfo> {
        self.privileges.iter().find(|info| &info.id == id)
    }

    /// The group `id` names.
    pub fn group(&self, id: &str) -> Option<&PrivilegeGroup> {
        self.groups.iter().find(|group| group.id == id)
    }

    /// Whether `id` can be granted server-wide.
    pub fn is_server(&self, id: &PrivilegeId) -> bool {
        self.info(id).is_some_and(PrivilegeInfo::is_server)
    }

    /// Whether `id` can be granted on an object.
    pub fn is_object(&self, id: &PrivilegeId) -> bool {
        self.info(id).is_some_and(PrivilegeInfo::is_object)
    }

    /// The ids of one group at one scope, in catalog order.
    pub fn group_privileges(&self, group: &str, scope: PrivilegeScope) -> Vec<PrivilegeId> {
        self.privileges
            .iter()
            .filter(|info| {
                info.groups
                    .iter()
                    .any(|(candidate, id)| *candidate == scope && id == group)
            })
            .map(|info| info.id.clone())
            .collect()
    }
}

/// One account (a MySQL user or role) as listed by the Users tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UserAccount {
    pub user: String,
    pub host: String,
    /// The authentication plugin, e.g. `caching_sha2_password`, `mysql_native_password`, `auth_socket`.
    pub plugin: String,
    /// Whether the account has a password set (`authentication_string` is non-empty). The server
    /// only stores the hash, so the plaintext cannot be shown.
    pub password_set: bool,
    pub password_expired: bool,
    /// `password_lifetime` in days; `None` means the server default (`DEFAULT`).
    pub password_lifetime: Option<u32>,
    /// PostgreSQL's `rolvaliduntil`: the moment the password stops being valid. `None` means no
    /// expiry. Stored as the value the engine reports (`YYYY-MM-DD HH:MM:SS+TZ`).
    #[serde(default)]
    pub password_valid_until: Option<String>,
    pub account_locked: bool,
    pub max_questions: u64,
    pub max_updates: u64,
    pub max_connections: u64,
    pub max_user_connections: u64,
    pub ssl_type: String,
    pub ssl_cipher: String,
    pub x509_issuer: String,
    pub x509_subject: String,
    /// Oracle's default tablespace.
    #[serde(default)]
    pub default_tablespace: String,
    /// Oracle's resource/tuning profile.
    #[serde(default)]
    pub profile: String,
    /// The account's quota on its default tablespace: `UNLIMITED`, a size like `100M`, or empty
    /// for none. Oracle reports/accepts it per tablespace; the editor manages the default one.
    #[serde(default)]
    pub tablespace_quota: String,
    /// SQL Server's login verification type (`SQL Server`, `Windows`). Empty for other engines.
    #[serde(default)]
    pub login_type: String,
    /// SQL Server's default database for the login.
    #[serde(default)]
    pub default_database: String,
    /// SQL Server's default language for the login.
    #[serde(default)]
    pub default_language: String,
    /// SQL Server's `CHECK_POLICY`.
    #[serde(default)]
    pub check_policy: bool,
    /// SQL Server's `CHECK_EXPIRATION`.
    #[serde(default)]
    pub check_expiration: bool,
    /// SQL Server's `MUST_CHANGE`: the password must change at the next login.
    #[serde(default)]
    pub must_change: bool,
    /// SQL Server's certificate the login is mapped from.
    #[serde(default)]
    pub certificate: String,
    /// SQL Server's asymmetric key the login is mapped from.
    #[serde(default)]
    pub asymmetric_key: String,
    /// SQL Server's credential attached to the login.
    #[serde(default)]
    pub credential: String,
    /// Whether the account holds the global `SUPER` privilege.
    pub is_super_user: bool,
}

impl UserAccount {
    /// The `user@host` label used by the user list. Engines without a host concept (PostgreSQL,
    /// SQL Server) leave `host` empty and the label is just the user name.
    pub fn label(&self) -> String {
        if self.host.is_empty() {
            self.user.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }

    /// Whether the account has a super-user (`SUPER`) grant on the server.
    pub fn is_super_user(&self) -> bool {
        self.is_super_user
    }
}

/// One row of `mysql.role_edges`: a role granted to (or, for the Members tab, a member of) an
/// account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleMembership {
    pub role_user: String,
    pub role_host: String,
    pub member_user: String,
    pub member_host: String,
    pub admin_option: bool,
}

impl RoleMembership {
    /// The role side of the edge as `user@host` (or just `user` when the engine has no host).
    pub fn role_label(&self) -> String {
        label(&self.role_user, &self.role_host)
    }

    /// The member side of the edge as `user@host` (or just `user` when the engine has no host).
    pub fn member_label(&self) -> String {
        label(&self.member_user, &self.member_host)
    }
}

/// One database a login is mapped into (SQL Server's 用户映射): whether the login has a database
/// user there, that user's name and default schema, and the database roles it belongs to.
///
/// Engines whose accounts are not database-scoped (MySQL, MariaDB, SQLite, PostgreSQL, Oracle)
/// have no mappings; the editor's 用户映射 section is hidden for them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserMapping {
    pub database: String,
    /// Whether the login has a user in this database.
    pub mapped: bool,
    /// The database user's name. Empty or the login name means the login's own name.
    pub user_name: String,
    /// The database user's default schema.
    pub default_schema: String,
    /// The database roles the user belongs to.
    pub roles: BTreeSet<String>,
    /// The database roles the editor offers for this database (fixed and user-defined roles).
    pub available_roles: Vec<String>,
}

/// One server-level securable's permissions for one account (SQL Server's endpoints and logins).
/// The editor's 终端节点权限 / 登录权限 sections render one row per available securable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerSecurableGrant {
    /// The securable class keyword (`ENDPOINT`, `LOGIN`), as used in `ON <class>::<name>`.
    pub class: String,
    pub name: String,
    pub privileges: BTreeSet<PrivilegeId>,
    /// Permissions explicitly denied (SQL Server's `DENY`, which overrides a grant).
    pub denied: BTreeSet<PrivilegeId>,
}

/// One class of server-level securables the account editor offers (SQL Server's endpoints, logins),
/// with the permissions grantable on it. The editor renders one section per class; `id` is the
/// stable key it matches its section against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurableClass {
    /// A stable id the editor uses to pick its section (`endpoint`, `login`).
    pub id: String,
    /// The class keyword used in `ON <class>::<name>`.
    pub class: String,
    pub label_key: Option<String>,
    pub label: String,
    /// The permissions offered, in display order.
    pub privileges: Vec<PrivilegeId>,
}

/// One object-level grant shown by the 权限 tab: a database plus (optionally) a table or routine
/// and the privileges granted on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectGrant {
    pub database: String,
    /// The schema/owner the object lives in; empty for engines without schemas.
    pub schema: String,
    /// Empty for a database-wide grant; otherwise the bare table/routine name.
    pub name: String,
    pub privileges: BTreeSet<PrivilegeId>,
}

impl ObjectGrant {
    pub fn new(database: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            database: database.into(),
            schema: String::new(),
            name: name.into(),
            privileges: BTreeSet::new(),
        }
    }

    /// The `schema.name` display key of the object, or `name` when there is no schema. Empty for a
    /// database-wide grant.
    pub fn object_name(&self) -> String {
        if self.schema.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }

    /// Split a schema-qualified object name (`schema.name`) into `(schema, name)`. A bare name
    /// yields an empty schema, so engines without schemas pass their name through unchanged.
    pub fn split_object_name(qualified: &str) -> (String, String) {
        match qualified.split_once('.') {
            Some((schema, name)) if !schema.is_empty() && !name.is_empty() => {
                (schema.to_string(), name.to_string())
            }
            _ => (String::new(), qualified.to_string()),
        }
    }
}

/// One default-privileges rule: the privileges `for_role` automatically grants on the objects it
/// creates, in one schema (or every schema) and for one object kind (PostgreSQL's
/// `ALTER DEFAULT PRIVILEGES`). The `FOR ROLE` is the account the editor is editing, so it is not
/// stored per rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultPrivilege {
    /// The schema the rule applies in; empty means every schema (no `IN SCHEMA`).
    pub schema: String,
    pub object_type: DefaultObjectType,
    /// The grantee account. PostgreSQL has no host part, so this is the bare role name.
    pub grantee: String,
    /// The privileges granted.
    pub privileges: BTreeSet<PrivilegeId>,
}

impl DefaultPrivilege {
    pub fn new(
        schema: impl Into<String>,
        object_type: DefaultObjectType,
        grantee: impl Into<String>,
    ) -> Self {
        Self {
            schema: schema.into(),
            object_type,
            grantee: grantee.into(),
            privileges: BTreeSet::new(),
        }
    }

    /// The key that identifies the rule: its schema, object kind and grantee.
    pub fn key(&self) -> (String, DefaultObjectType, String) {
        (self.schema.clone(), self.object_type, self.grantee.clone())
    }
}

/// One account's privileges on a single object, for the privilege manager's matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPrivilegeRow {
    pub user: String,
    pub host: String,
    pub privileges: BTreeSet<PrivilegeId>,
    /// Privileges explicitly denied (`DENY` in SQL Server, which overrides a grant). Engines
    /// without a deny concept leave it empty.
    pub denied: BTreeSet<PrivilegeId>,
}

impl ObjectPrivilegeRow {
    pub fn new(user: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            host: host.into(),
            privileges: BTreeSet::new(),
            denied: BTreeSet::new(),
        }
    }

    pub fn label(&self) -> String {
        label(&self.user, &self.host)
    }
}

/// The `user@host` label, or just `user` when the engine has no host concept.
fn label(user: &str, host: &str) -> String {
    if host.is_empty() {
        user.to_string()
    } else {
        format!("{user}@{host}")
    }
}

/// The loaded, editable state of one account.
#[derive(Debug, Clone, Default)]
pub struct UserDetails {
    pub account: UserAccount,
    /// The server-wide privileges currently granted.
    pub server_privileges: BTreeSet<PrivilegeId>,
    /// Server-wide privileges explicitly denied (SQL Server's `DENY`, which overrides a grant).
    pub denied_server_privileges: BTreeSet<PrivilegeId>,
    /// The object-level grants of the 权限 tab.
    pub grants: Vec<ObjectGrant>,
    /// The account's default-privileges rules (PostgreSQL's `ALTER DEFAULT PRIVILEGES`).
    pub default_privileges: Vec<DefaultPrivilege>,
    /// The role edges where this account is the member (the 成员属于 tab).
    pub roles: Vec<RoleMembership>,
    /// The role edges where this account is the role (the 成员 tab).
    pub members: Vec<RoleMembership>,
}

/// The edits the user editor applies on Save.
#[derive(Debug, Clone)]
pub struct UserEdit {
    /// The account as loaded, used to diff privileges and role memberships; `None` when creating a
    /// new account.
    pub original: Option<UserDetails>,
    pub account: UserAccount,
    /// `Some` when the password should be (re)set.
    pub password: Option<String>,
    pub server_privileges: BTreeSet<PrivilegeId>,
    /// Server-wide privileges to explicitly deny (SQL Server).
    pub denied_server_privileges: BTreeSet<PrivilegeId>,
    pub grants: Vec<ObjectGrant>,
    /// The account's default-privileges rules to write (PostgreSQL's `ALTER DEFAULT PRIVILEGES`).
    pub default_privileges: Vec<DefaultPrivilege>,
    /// Role memberships of this account as `(role_user, role_host, admin_option)`: roles this
    /// account is granted (the 成员属于 tab).
    pub roles: Vec<(String, String, bool)>,
    /// Accounts that are members of this account as `(member_user, member_host, admin_option)`
    /// (the 成员 tab, meaningful when the account is a role).
    pub members: Vec<(String, String, bool)>,
    /// The account's per-database mappings to write (SQL Server's 用户映射).
    pub mappings: Vec<UserMapping>,
    /// The mappings as loaded, for the diff. Empty when creating a new account.
    pub original_mappings: Vec<UserMapping>,
    /// The password to verify against the old one (`ALTER LOGIN ... OLD_PASSWORD`, SQL Server).
    pub old_password: Option<String>,
    /// The account's server-level securable grants to write (SQL Server's endpoints/logins).
    pub securables: Vec<ServerSecurableGrant>,
    /// The securable grants as loaded, for the diff.
    pub original_securables: Vec<ServerSecurableGrant>,
}

/// The kinds of change one account save makes. The editor groups the generated statements by this
/// so its confirmation dialog can annotate each group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserEditSection {
    /// Identity and attributes: rename, authentication, password, resource limits, lock/expiry.
    Account,
    /// Global (`*.*`) privileges.
    ServerPrivileges,
    /// Database/table/routine grants.
    ObjectGrants,
    /// Default privileges (`ALTER DEFAULT PRIVILEGES`).
    DefaultPrivileges,
    /// Role memberships.
    Roles,
    /// Per-database mappings (SQL Server's 用户映射).
    UserMapping,
    /// Server-level securable grants (SQL Server's 终端节点权限 / 登录权限).
    Securables,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_names_split_and_rejoin() {
        assert_eq!(
            ObjectGrant::split_object_name("dbo.users"),
            ("dbo".to_string(), "users".to_string())
        );
        assert_eq!(
            ObjectGrant::split_object_name("users"),
            (String::new(), "users".to_string())
        );
        let grant = ObjectGrant {
            database: "shop".to_string(),
            schema: "dbo".to_string(),
            name: "users".to_string(),
            privileges: BTreeSet::new(),
        };
        assert_eq!(grant.object_name(), "dbo.users");
        // A bare grant's object name is the bare name, not a schema-qualified one.
        assert_eq!(ObjectGrant::new("shop", "orders").object_name(), "orders");
    }

    #[test]
    fn default_privileges_are_filtered_by_object_type() {
        let catalog = PrivilegeCatalog {
            default_privileges: vec![
                DefaultPrivilegeInfo::new(
                    DefaultObjectType::Tables,
                    "SELECT",
                    Some("user.priv.select"),
                    "SELECT",
                ),
                DefaultPrivilegeInfo::new(
                    DefaultObjectType::Sequences,
                    "USAGE",
                    Some("user.priv.usage"),
                    "USAGE",
                ),
            ],
            ..Default::default()
        };
        assert!(catalog.has_default_privileges());
        let tables: Vec<&str> = catalog
            .default_privileges_for(DefaultObjectType::Tables)
            .iter()
            .map(|info| info.id.as_str())
            .collect();
        assert_eq!(tables, vec!["SELECT"]);
        assert!(
            catalog
                .default_privileges_for(DefaultObjectType::Functions)
                .is_empty()
        );
        assert_eq!(DefaultObjectType::Schemas.keyword(), "SCHEMAS");
    }
}

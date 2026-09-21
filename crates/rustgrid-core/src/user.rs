//! Engine-agnostic models for account (user/role) administration and the privileges that can be
//! granted to an account. The UI only ever sees these types; each driver maps them to its own
//! catalog and grant syntax.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A privilege that can be granted to an account.
///
/// MySQL has two levels: the server-wide privileges (`mysql.user`'s `*_priv` columns) and the
/// subset that can also be granted on a database/table/routine/column ([`Privilege::OBJECT`]).
/// The `sql_name` is the keyword used in a `GRANT` statement and is also the value MySQL reports
/// through `information_schema.*_PRIVILEGES.PRIVILEGE_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Privilege {
    Alter,
    AlterRoutine,
    Create,
    CreateRoutine,
    CreateTemporaryTables,
    CreateUser,
    CreateView,
    Delete,
    Drop,
    Event,
    Execute,
    File,
    GrantOption,
    Index,
    Insert,
    LockTables,
    Process,
    References,
    Reload,
    ReplicationClient,
    ReplicationSlave,
    Select,
    ShowDatabases,
    ShowView,
    Shutdown,
    Super,
    Trigger,
    Update,
}

impl Privilege {
    /// Every server-wide privilege, in the order the UI presents them.
    pub const ALL: [Privilege; 28] = [
        Privilege::Alter,
        Privilege::AlterRoutine,
        Privilege::Create,
        Privilege::CreateRoutine,
        Privilege::CreateTemporaryTables,
        Privilege::CreateUser,
        Privilege::CreateView,
        Privilege::Delete,
        Privilege::Drop,
        Privilege::Event,
        Privilege::Execute,
        Privilege::File,
        Privilege::GrantOption,
        Privilege::Index,
        Privilege::Insert,
        Privilege::LockTables,
        Privilege::Process,
        Privilege::References,
        Privilege::Reload,
        Privilege::ReplicationClient,
        Privilege::ReplicationSlave,
        Privilege::Select,
        Privilege::ShowDatabases,
        Privilege::ShowView,
        Privilege::Shutdown,
        Privilege::Super,
        Privilege::Trigger,
        Privilege::Update,
    ];

    /// The privileges that can be granted on an object (database/table/routine), in the order the
    /// privilege dialog presents them.
    pub const OBJECT: [Privilege; 16] = [
        Privilege::Alter,
        Privilege::AlterRoutine,
        Privilege::Create,
        Privilege::CreateRoutine,
        Privilege::CreateTemporaryTables,
        Privilege::CreateView,
        Privilege::Delete,
        Privilege::Drop,
        Privilege::GrantOption,
        Privilege::Index,
        Privilege::Insert,
        Privilege::References,
        Privilege::Select,
        Privilege::ShowView,
        Privilege::Trigger,
        Privilege::Update,
    ];

    /// The i18n key of the privilege's label.
    pub fn label_key(self) -> &'static str {
        match self {
            Privilege::Alter => "user.priv.alter",
            Privilege::AlterRoutine => "user.priv.alter_routine",
            Privilege::Create => "user.priv.create",
            Privilege::CreateRoutine => "user.priv.create_routine",
            Privilege::CreateTemporaryTables => "user.priv.create_temporary_tables",
            Privilege::CreateUser => "user.priv.create_user",
            Privilege::CreateView => "user.priv.create_view",
            Privilege::Delete => "user.priv.delete",
            Privilege::Drop => "user.priv.drop",
            Privilege::Event => "user.priv.event",
            Privilege::Execute => "user.priv.execute",
            Privilege::File => "user.priv.file",
            Privilege::GrantOption => "user.priv.grant_option",
            Privilege::Index => "user.priv.index",
            Privilege::Insert => "user.priv.insert",
            Privilege::LockTables => "user.priv.lock_tables",
            Privilege::Process => "user.priv.process",
            Privilege::References => "user.priv.references",
            Privilege::Reload => "user.priv.reload",
            Privilege::ReplicationClient => "user.priv.replication_client",
            Privilege::ReplicationSlave => "user.priv.replication_slave",
            Privilege::Select => "user.priv.select",
            Privilege::ShowDatabases => "user.priv.show_databases",
            Privilege::ShowView => "user.priv.show_view",
            Privilege::Shutdown => "user.priv.shutdown",
            Privilege::Super => "user.priv.super",
            Privilege::Trigger => "user.priv.trigger",
            Privilege::Update => "user.priv.update",
        }
    }

    /// The keyword MySQL uses in `GRANT`, and the value it reports through
    /// `information_schema.*_PRIVILEGES`.
    pub fn sql_name(self) -> &'static str {
        match self {
            Privilege::Alter => "ALTER",
            Privilege::AlterRoutine => "ALTER ROUTINE",
            Privilege::Create => "CREATE",
            Privilege::CreateRoutine => "CREATE ROUTINE",
            Privilege::CreateTemporaryTables => "CREATE TEMPORARY TABLES",
            Privilege::CreateUser => "CREATE USER",
            Privilege::CreateView => "CREATE VIEW",
            Privilege::Delete => "DELETE",
            Privilege::Drop => "DROP",
            Privilege::Event => "EVENT",
            Privilege::Execute => "EXECUTE",
            Privilege::File => "FILE",
            Privilege::GrantOption => "GRANT OPTION",
            Privilege::Index => "INDEX",
            Privilege::Insert => "INSERT",
            Privilege::LockTables => "LOCK TABLES",
            Privilege::Process => "PROCESS",
            Privilege::References => "REFERENCES",
            Privilege::Reload => "RELOAD",
            Privilege::ReplicationClient => "REPLICATION CLIENT",
            Privilege::ReplicationSlave => "REPLICATION SLAVE",
            Privilege::Select => "SELECT",
            Privilege::ShowDatabases => "SHOW DATABASES",
            Privilege::ShowView => "SHOW VIEW",
            Privilege::Shutdown => "SHUTDOWN",
            Privilege::Super => "SUPER",
            Privilege::Trigger => "TRIGGER",
            Privilege::Update => "UPDATE",
        }
    }

    /// Whether an object-level grant can carry this privilege.
    pub fn is_object(self) -> bool {
        Privilege::OBJECT.contains(&self)
    }

    /// Resolve a privilege from the keyword MySQL reports (case-insensitive), e.g. `"ALTER"`.
    pub fn from_sql_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_uppercase();
        Privilege::ALL
            .into_iter()
            .find(|privilege| privilege.sql_name() == name)
    }
}

/// The columns of a privilege list, in [`Privilege::ALL`] order. Used by the UI grids.
pub fn privilege_columns(privileges: &[Privilege]) -> Vec<Privilege> {
    let mut all = privileges.to_vec();
    all.sort();
    all
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
    pub account_locked: bool,
    pub max_questions: u64,
    pub max_updates: u64,
    pub max_connections: u64,
    pub max_user_connections: u64,
    pub ssl_type: String,
    pub ssl_cipher: String,
    pub x509_issuer: String,
    pub x509_subject: String,
}

impl UserAccount {
    /// The `user@host` label used by the user list.
    pub fn label(&self) -> String {
        format!("{}@{}", self.user, self.host)
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
    /// The role side of the edge as `user@host`.
    pub fn role_label(&self) -> String {
        format!("{}@{}", self.role_user, self.role_host)
    }

    /// The member side of the edge as `user@host`.
    pub fn member_label(&self) -> String {
        format!("{}@{}", self.member_user, self.member_host)
    }
}

/// One object-level grant shown by the 权限 tab: a database plus (optionally) a table or routine
/// and the privileges granted on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectGrant {
    pub database: String,
    /// Empty for a database-wide grant; otherwise the table/routine name.
    pub name: String,
    pub privileges: BTreeSet<Privilege>,
}

impl ObjectGrant {
    pub fn new(database: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            database: database.into(),
            name: name.into(),
            privileges: BTreeSet::new(),
        }
    }
}

/// One account's privileges on a single object, for the privilege manager's matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPrivilegeRow {
    pub user: String,
    pub host: String,
    pub privileges: BTreeSet<Privilege>,
}

impl ObjectPrivilegeRow {
    pub fn new(user: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            host: host.into(),
            privileges: BTreeSet::new(),
        }
    }

    pub fn label(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }
}

/// The loaded, editable state of one account.
#[derive(Debug, Clone, Default)]
pub struct UserDetails {
    pub account: UserAccount,
    /// The server-wide privileges currently granted.
    pub server_privileges: BTreeSet<Privilege>,
    /// The object-level grants of the 权限 tab.
    pub grants: Vec<ObjectGrant>,
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
    pub server_privileges: BTreeSet<Privilege>,
    pub grants: Vec<ObjectGrant>,
    /// Role memberships of this account as `(role_user, role_host, admin_option)`: roles this
    /// account is granted (the 成员属于 tab).
    pub roles: Vec<(String, String, bool)>,
    /// Accounts that are members of this account as `(member_user, member_host, admin_option)`
    /// (the 成员 tab, meaningful when the account is a role).
    pub members: Vec<(String, String, bool)>,
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
    /// Role memberships.
    Roles,
}

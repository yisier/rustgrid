//! The user-account window (Navicat's 新建用户 / 编辑用户): one unified editor for creating and
//! editing a MySQL account.
//!
//! There is no separate "quick" and "full" view any more. A single window shows a left navigation
//! with five sections — 常规 (identity, authentication, attributes, resource limits), 服务器权限
//! (global `*.*` privileges), 权限 (database/table grants), 角色 (role memberships) and SQL 预览 —
//! and the same surface creates a new account or edits an existing one. A rename (changing the
//! username/host) is applied with `RENAME USER` on save.
//!
//! Like the Export/Import wizards and the Backup window, this is a **separate OS window**, not a
//! `Root` modal: `AppView` owns the state ([`UserCreateDialog`]) and this module renders it, so
//! `AppView::sync_dialog` is not involved.

use std::collections::{BTreeMap, BTreeSet};

use super::*;

/// Height of one row in the identity form and the grant/role lists.
const CREATE_ROW_HEIGHT: f32 = 26.0;
/// Width of the identity form's label column; every label ends at the same x.
const CREATE_LABEL_WIDTH: f32 = 150.0;
/// Gap between a label and its control.
const CREATE_LABEL_GAP: f32 = 16.0;
/// Width of the identity form's control column.
const CREATE_FIELD_WIDTH: f32 = 300.0;
/// Width of the narrower resource-limit fields.
const CREATE_LIMIT_WIDTH: f32 = 120.0;
/// Width of the left section navigation.
const CREATE_NAV_WIDTH: f32 = 150.0;
/// Width of the SQL preview's monospace box.
const CREATE_PREVIEW_HEIGHT: f32 = 260.0;

/// The 密码过期策略 choice, mapped to [`UserAccount::password_lifetime`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateExpiry {
    /// The server default (`NULL`).
    Default,
    /// The password never expires (`0`).
    Never,
    /// Expire after [`UserCreateDialog::expiry_days`] days.
    Interval,
}

impl CreateExpiry {
    fn from_value(value: &str) -> Self {
        match value {
            "never" => CreateExpiry::Never,
            "interval" => CreateExpiry::Interval,
            _ => CreateExpiry::Default,
        }
    }
}

/// The combo value for an expiry choice (the inverse of [`CreateExpiry::from_value`]).
fn expiry_value(expiry: CreateExpiry) -> &'static str {
    match expiry {
        CreateExpiry::Default => "default",
        CreateExpiry::Never => "never",
        CreateExpiry::Interval => "interval",
    }
}

/// A one-click preset for the 服务器权限 section. Picking one replaces the ticked set, which can
/// then be adjusted by hand.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerTemplate {
    None,
    ReadOnly,
    ReadWrite,
    Developer,
    Admin,
}

impl ServerTemplate {
    const ALL: [ServerTemplate; 5] = [
        ServerTemplate::None,
        ServerTemplate::ReadOnly,
        ServerTemplate::ReadWrite,
        ServerTemplate::Developer,
        ServerTemplate::Admin,
    ];

    fn label_key(self) -> &'static str {
        match self {
            ServerTemplate::None => "user.create.template.none",
            ServerTemplate::ReadOnly => "user.create.template.read_only",
            ServerTemplate::ReadWrite => "user.create.template.read_write",
            ServerTemplate::Developer => "user.create.template.developer",
            ServerTemplate::Admin => "user.create.template.admin",
        }
    }

    /// The global privileges this template grants.
    pub(super) fn privileges(self) -> BTreeSet<Privilege> {
        let mut set = BTreeSet::new();
        match self {
            ServerTemplate::None => {}
            ServerTemplate::ReadOnly => {
                set.extend([Privilege::Select, Privilege::ShowView]);
            }
            ServerTemplate::ReadWrite => {
                set.extend([
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                    Privilege::ShowView,
                    Privilege::Execute,
                ]);
            }
            ServerTemplate::Developer => {
                set.extend([
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                    Privilege::ShowView,
                    Privilege::Execute,
                    Privilege::Create,
                    Privilege::Alter,
                    Privilege::Drop,
                    Privilege::Index,
                    Privilege::CreateView,
                    Privilege::CreateRoutine,
                    Privilege::AlterRoutine,
                    Privilege::References,
                    Privilege::Trigger,
                    Privilege::Event,
                    Privilege::CreateTemporaryTables,
                    Privilege::LockTables,
                ]);
            }
            ServerTemplate::Admin => set.extend(Privilege::ALL),
        }
        set
    }
}

/// The window's left-navigation sections.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UserSection {
    General,
    ServerPrivileges,
    ObjectPrivileges,
    Roles,
    Sql,
}

impl UserSection {
    const ALL: [UserSection; 5] = [
        UserSection::General,
        UserSection::ServerPrivileges,
        UserSection::ObjectPrivileges,
        UserSection::Roles,
        UserSection::Sql,
    ];

    fn label_key(self) -> &'static str {
        match self {
            UserSection::General => "user.tab.general",
            UserSection::ServerPrivileges => "user.tab.server_privileges",
            UserSection::ObjectPrivileges => "user.tab.privileges",
            UserSection::Roles => "user.tab.roles",
            UserSection::Sql => "user.tab.sql",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            UserSection::General => "icons/user.svg",
            UserSection::ServerPrivileges => "icons/gear.svg",
            UserSection::ObjectPrivileges => "icons/database.svg",
            UserSection::Roles => "icons/user.svg",
            UserSection::Sql => "icons/queries.svg",
        }
    }

    fn id(self) -> &'static str {
        match self {
            UserSection::General => "user-create-nav-general",
            UserSection::ServerPrivileges => "user-create-nav-server",
            UserSection::ObjectPrivileges => "user-create-nav-object",
            UserSection::Roles => "user-create-nav-roles",
            UserSection::Sql => "user-create-nav-sql",
        }
    }
}

/// Whether a database's grant covers the whole database or only picked tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GrantScope {
    AllTables,
    SpecificTables,
}

/// A one-click database-level privilege preset, matching the reference layout's 快捷权限预设.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DbTemplate {
    None,
    ReadOnly,
    ReadWrite,
    Full,
}

impl DbTemplate {
    const ALL: [DbTemplate; 4] = [
        DbTemplate::None,
        DbTemplate::ReadOnly,
        DbTemplate::ReadWrite,
        DbTemplate::Full,
    ];

    fn label_key(self) -> &'static str {
        match self {
            DbTemplate::None => "user.create.db_template.none",
            DbTemplate::ReadOnly => "user.create.db_template.read_only",
            DbTemplate::ReadWrite => "user.create.db_template.read_write",
            DbTemplate::Full => "user.create.db_template.full",
        }
    }

    pub(super) fn privileges(self) -> BTreeSet<Privilege> {
        let mut set = BTreeSet::new();
        match self {
            DbTemplate::None => {}
            DbTemplate::ReadOnly => {
                set.insert(Privilege::Select);
            }
            DbTemplate::ReadWrite => {
                set.extend([
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                ]);
            }
            DbTemplate::Full => set.extend(Privilege::OBJECT),
        }
        set
    }
}

/// The grouped object privileges the 权限 detail pane shows as check boxes.
fn db_privilege_groups() -> [(&'static str, Vec<Privilege>); 3] {
    [
        (
            "user.create.priv_group.dml",
            vec![
                Privilege::Select,
                Privilege::Insert,
                Privilege::Update,
                Privilege::Delete,
            ],
        ),
        (
            "user.create.priv_group.ddl",
            vec![
                Privilege::Create,
                Privilege::Alter,
                Privilege::Drop,
                Privilege::Index,
                Privilege::CreateView,
                Privilege::CreateTemporaryTables,
            ],
        ),
        (
            "user.create.priv_group.routines",
            vec![
                Privilege::Execute,
                Privilege::AlterRoutine,
                Privilege::CreateRoutine,
                Privilege::Trigger,
                Privilege::References,
                Privilege::LockTables,
                Privilege::ShowView,
                Privilege::GrantOption,
            ],
        ),
    ]
}

/// The global (`*.*`) privileges grouped for the 服务器权限 section. Covers all of
/// [`Privilege::ALL`], so the templates and the grid always agree.
fn server_privilege_groups() -> [(&'static str, Vec<Privilege>); 6] {
    [
        (
            "user.create.priv_group.dml",
            vec![
                Privilege::Select,
                Privilege::Insert,
                Privilege::Update,
                Privilege::Delete,
            ],
        ),
        (
            "user.create.priv_group.ddl",
            vec![
                Privilege::Create,
                Privilege::Alter,
                Privilege::Drop,
                Privilege::Index,
                Privilege::CreateView,
                Privilege::CreateTemporaryTables,
                Privilege::CreateRoutine,
                Privilege::AlterRoutine,
                Privilege::References,
                Privilege::Trigger,
            ],
        ),
        (
            "user.create.server_group.exec",
            vec![
                Privilege::Execute,
                Privilege::ShowView,
                Privilege::ShowDatabases,
                Privilege::LockTables,
            ],
        ),
        (
            "user.create.server_group.admin",
            vec![
                Privilege::Process,
                Privilege::Reload,
                Privilege::Shutdown,
                Privilege::Super,
                Privilege::File,
                Privilege::GrantOption,
            ],
        ),
        (
            "user.create.server_group.replication",
            vec![Privilege::ReplicationClient, Privilege::ReplicationSlave],
        ),
        (
            "user.create.server_group.account",
            vec![Privilege::CreateUser, Privilege::Event],
        ),
    ]
}

/// One database row of the 权限 section: whether it is granted, its scope, and its privileges.
pub(super) struct DbGrant {
    pub(super) name: String,
    pub(super) enabled: bool,
    pub(super) scope: GrantScope,
    /// The tables picked for the 指定具体表 scope.
    pub(super) tables: Vec<String>,
    /// The table whose privileges the detail pane edits (an index into `tables` by name).
    pub(super) active_table: Option<String>,
    /// The privileges granted on the whole database (the 全部表 scope).
    pub(super) privileges: BTreeSet<Privilege>,
    /// The privileges granted per table (the 指定具体表 scope), keyed by table name.
    pub(super) table_privileges: BTreeMap<String, BTreeSet<Privilege>>,
}

/// The i18n key naming a privilege set: the matching quick preset, or 自定义.
fn privilege_preset_key(privileges: &BTreeSet<Privilege>) -> &'static str {
    if *privileges == DbTemplate::ReadOnly.privileges() {
        "user.create.db_template.read_only"
    } else if *privileges == DbTemplate::ReadWrite.privileges() {
        "user.create.db_template.read_write"
    } else if *privileges == DbTemplate::Full.privileges() {
        "user.create.db_template.full"
    } else {
        "user.create.custom"
    }
}

/// The badge for one privilege set: the preset name (with its count) or 未授权.
fn privilege_summary(privileges: &BTreeSet<Privilege>) -> (String, bool) {
    if privileges.is_empty() {
        return (t!("user.create.unauthorized").to_string(), false);
    }
    (
        format!(
            "{} ({})",
            t!(privilege_preset_key(privileges)),
            privileges.len()
        ),
        true,
    )
}

impl DbGrant {
    /// The privilege set the detail pane edits: the whole database for 全部表, otherwise the active
    /// table's own set (empty when no table is active).
    fn active_privileges(&self) -> BTreeSet<Privilege> {
        match self.scope {
            GrantScope::AllTables => self.privileges.clone(),
            GrantScope::SpecificTables => self
                .active_table
                .as_ref()
                .and_then(|table| self.table_privileges.get(table))
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The set the detail pane mutates, for the active scope. `None` for 指定具体表 with no active
    /// table, so a stray toggle cannot silently change the wrong grant.
    fn active_privileges_mut(&mut self) -> Option<&mut BTreeSet<Privilege>> {
        match self.scope {
            GrantScope::AllTables => Some(&mut self.privileges),
            GrantScope::SpecificTables => {
                let table = self.active_table.clone()?;
                Some(self.table_privileges.entry(table).or_default())
            }
        }
    }

    /// Make `table` the active 指定具体表 row, selecting it if it is not picked yet.
    fn activate_table(&mut self, table: &str) {
        self.scope = GrantScope::SpecificTables;
        self.enabled = true;
        if !self.tables.iter().any(|picked| picked == table) {
            self.tables.push(table.to_string());
        }
        self.active_table = Some(table.to_string());
        self.table_privileges.entry(table.to_string()).or_default();
    }

    /// Every privilege set this row actually applies, for the list badge: one per picked table for
    /// 指定具体表, else the single whole-database set.
    fn effective_sets(&self) -> Vec<BTreeSet<Privilege>> {
        if self.scope == GrantScope::SpecificTables && !self.tables.is_empty() {
            self.tables
                .iter()
                .map(|table| {
                    self.table_privileges
                        .get(table)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect()
        } else {
            vec![self.privileges.clone()]
        }
    }

    /// The badge shown in the database list: the preset name (with its privilege count) or 未授权,
    /// plus whether the database is granted (for its colour). Per-table differences read 自定义.
    fn summary(&self) -> (String, bool) {
        if !self.enabled {
            return (t!("user.create.unauthorized").to_string(), false);
        }
        let sets = self.effective_sets();
        let union: BTreeSet<Privilege> = sets.iter().flat_map(|set| set.iter().copied()).collect();
        if union.is_empty() {
            return (t!("user.create.unauthorized").to_string(), false);
        }
        if sets.iter().all(|set| *set == sets[0]) {
            privilege_summary(&sets[0])
        } else {
            (
                format!("{} ({})", t!("user.create.custom"), union.len()),
                true,
            )
        }
    }
}

/// State of the user-account window.
pub(super) struct UserCreateDialog {
    /// The connection the account lives on.
    pub(super) connection_index: usize,
    /// The active navigation section.
    pub(super) section: UserSection,
    /// The account being described. Owned here so create and edit share one source of truth.
    pub(super) editor: UserEditorState,
    pub(super) expiry: CreateExpiry,
    pub(super) expiry_days: u32,
    /// The server's databases, in server order.
    pub(super) databases: Vec<String>,
    /// One row per database, driving the 权限 section's list/detail.
    pub(super) db_grants: Vec<DbGrant>,
    /// The database shown in the 权限 detail pane (index into `db_grants`).
    pub(super) active_database: Option<usize>,
    /// The 权限 database filter text.
    pub(super) database_search: String,
    pub(super) db_scroll: ScrollHandle,
    pub(super) table_scroll: ScrollHandle,
    /// Lazily-loaded tables per database (the 指定具体表 picker).
    pub(super) tables: BTreeMap<String, Vec<String>>,
    pub(super) loading_tables: BTreeSet<String>,
    /// The account's role/member edges, loaded with the account.
    pub(super) context: Option<UserAccountContext>,
    /// Every account on the server, for the role membership lists.
    pub(super) accounts: Vec<(String, String)>,
    pub(super) sql_scroll: ScrollHandle,
    /// Whether the account's details are still loading (Save is disabled until they are).
    pub(super) loading: bool,
    /// The window's inline validation error.
    pub(super) error: Option<String>,
    pub(super) saving: bool,
    /// Set briefly after a successful save so the footer can confirm it.
    pub(super) saved: bool,
    /// Whether the password fields are active while editing (the 修改登录密码 checkbox). Always
    /// true for a new account.
    pub(super) change_password: bool,
    /// Whether the 确认并执行 dialog is open.
    pub(super) confirm_open: bool,
}

impl UserCreateDialog {
    /// A fresh window state for a connection supporting `plugins`. `account` is `Some((user,
    /// host))` when editing an existing account, `None` when creating one.
    pub(super) fn new(
        connection_index: usize,
        plugin: String,
        account: Option<(String, String)>,
    ) -> Self {
        let user = account
            .as_ref()
            .map(|(user, _)| user.clone())
            .unwrap_or_default();
        let host = account
            .as_ref()
            .map(|(_, host)| host.clone())
            .unwrap_or_else(|| "%".to_string());
        let editing = account.is_some();
        let mut editor = UserEditorState::new(plugin.clone());
        editor.account.user = user;
        editor.account.host = host;
        // The identity is known up front, so the title and 新建/编辑 state are correct before the
        // account's details finish loading.
        editor.original_account = account.clone();
        let context = Some(UserAccountContext {
            user: editor.account.user.clone(),
            host: editor.account.host.clone(),
            roles: BTreeMap::new(),
            members: BTreeMap::new(),
        });
        Self {
            connection_index,
            section: UserSection::General,
            editor,
            expiry: CreateExpiry::Default,
            expiry_days: 30,
            databases: Vec::new(),
            db_grants: Vec::new(),
            active_database: None,
            database_search: String::new(),
            db_scroll: ScrollHandle::new(),
            table_scroll: ScrollHandle::new(),
            tables: BTreeMap::new(),
            loading_tables: BTreeSet::new(),
            context,
            accounts: Vec::new(),
            sql_scroll: ScrollHandle::new(),
            loading: editing,
            error: None,
            saving: false,
            saved: false,
            change_password: !editing,
            confirm_open: false,
        }
    }

    /// Load an account's details into the editor and mirror its role edges into the 角色 lists'
    /// edit state, so the 成员属于 / 成员 check boxes reflect what the account already holds.
    pub(super) fn apply_details(&mut self, details: UserDetails) {
        self.editor.apply_details(details);
        let (roles, members) = self
            .editor
            .original
            .as_ref()
            .map(|details| {
                let roles = details
                    .roles
                    .iter()
                    .map(|edge| {
                        (
                            (edge.role_user.clone(), edge.role_host.clone()),
                            edge.admin_option,
                        )
                    })
                    .collect();
                let members = details
                    .members
                    .iter()
                    .map(|edge| {
                        (
                            (edge.member_user.clone(), edge.member_host.clone()),
                            edge.admin_option,
                        )
                    })
                    .collect();
                (roles, members)
            })
            .unwrap_or_default();
        self.context = Some(UserAccountContext {
            user: self.editor.account.user.clone(),
            host: self.editor.account.host.clone(),
            roles,
            members,
        });
    }

    /// `(user, host)` of the account being edited, for the window title.
    pub(super) fn original_account(&self) -> Option<&(String, String)> {
        self.editor.original_account.as_ref()
    }

    /// Whether an existing account is open.
    pub(super) fn is_edit(&self) -> bool {
        self.editor.original_account.is_some()
    }

    /// The user/host pairs the role lists pick from.
    pub(super) fn membership_candidates(&self) -> &[(String, String)] {
        &self.accounts
    }

    /// Rebuild the 权限 rows from the server database list and the account's loaded grants. Called
    /// once the databases and (for an edit) the details are both in.
    fn rebuild_db_grants(&mut self) {
        let grants = &self.editor.grants;
        // The server's database list plus any database that only appears in the account's grants,
        // so a granted database the listing omits is never silently dropped (and revoked) on save.
        let mut names: Vec<String> = self.databases.clone();
        for grant in grants {
            if !names.iter().any(|name| name == &grant.database) {
                names.push(grant.database.clone());
            }
        }
        let mut rows: Vec<DbGrant> = Vec::with_capacity(names.len());
        for database in &names {
            let db_grants: Vec<&ObjectGrant> = grants
                .iter()
                .filter(|grant| &grant.database == database)
                .collect();
            if let Some(whole) = db_grants.iter().find(|grant| grant.name.is_empty()) {
                rows.push(DbGrant {
                    name: database.clone(),
                    enabled: true,
                    scope: GrantScope::AllTables,
                    tables: Vec::new(),
                    active_table: None,
                    privileges: whole.privileges.clone(),
                    table_privileges: BTreeMap::new(),
                });
                continue;
            }
            let table_grants: Vec<&ObjectGrant> = db_grants
                .iter()
                .copied()
                .filter(|grant| !grant.name.is_empty())
                .collect();
            if table_grants.is_empty() {
                rows.push(DbGrant {
                    name: database.clone(),
                    enabled: false,
                    scope: GrantScope::AllTables,
                    tables: Vec::new(),
                    active_table: None,
                    privileges: BTreeSet::new(),
                    table_privileges: BTreeMap::new(),
                });
            } else {
                // Keep each table's own privileges (they may differ), rather than flattening them
                // into one set shared by the whole scope.
                let tables: Vec<String> = table_grants
                    .iter()
                    .map(|grant| grant.name.clone())
                    .collect();
                let mut table_privileges: BTreeMap<String, BTreeSet<Privilege>> = BTreeMap::new();
                for grant in &table_grants {
                    table_privileges
                        .entry(grant.name.clone())
                        .or_default()
                        .extend(grant.privileges.iter().copied());
                }
                rows.push(DbGrant {
                    name: database.clone(),
                    enabled: true,
                    scope: GrantScope::SpecificTables,
                    active_table: tables.first().cloned(),
                    tables,
                    privileges: BTreeSet::new(),
                    table_privileges,
                });
            }
        }
        self.db_grants = rows;
        self.active_database = (!self.db_grants.is_empty()).then_some(0);
    }

    /// Project the 权限 rows into the object grants `UserEdit` carries. A 指定具体表 scope with no
    /// tables picked falls back to a whole-database grant so Save is never a silent no-op.
    fn object_grants(&self) -> Vec<ObjectGrant> {
        let mut grants = Vec::new();
        for row in &self.db_grants {
            if !row.enabled {
                continue;
            }
            if row.scope == GrantScope::SpecificTables && !row.tables.is_empty() {
                for table in &row.tables {
                    grants.push(ObjectGrant {
                        database: row.name.clone(),
                        name: table.clone(),
                        privileges: row.table_privileges.get(table).cloned().unwrap_or_default(),
                    });
                }
            } else {
                grants.push(ObjectGrant {
                    database: row.name.clone(),
                    name: String::new(),
                    privileges: row.privileges.clone(),
                });
            }
        }
        grants
    }
}

/// Which identity/limit field a change came from, used by the window's text inputs.
#[derive(Clone, Copy)]
pub(super) enum CreateField {
    User,
    Host,
    Password,
    Confirm,
    ExpiryDays,
    MaxQuestions,
    MaxUpdates,
    MaxConnections,
    MaxUserConnections,
    DatabaseSearch,
}

/// Which dropdown a change came from.
#[derive(Clone, Copy)]
enum CreateCombo {
    Plugin,
    Expiry,
}

/// The account-level state the window edits: the attributes, the server privileges, the individual
/// object grants and the role edges.
pub(super) struct UserEditorState {
    /// The account as loaded, or `None` while creating a new one.
    pub(super) original: Option<UserDetails>,
    /// `(user, host)` of the account being edited, used by the window title.
    pub(super) original_account: Option<(String, String)>,
    pub(super) account: UserAccount,
    pub(super) password: String,
    pub(super) guard_password: String,
    pub(super) server_privileges: BTreeSet<Privilege>,
    pub(super) grants: Vec<ObjectGrant>,
}

impl UserEditorState {
    fn new(plugin: String) -> Self {
        Self {
            original: None,
            original_account: None,
            account: UserAccount {
                plugin,
                host: "%".to_string(),
                ..UserAccount::default()
            },
            password: String::new(),
            guard_password: String::new(),
            server_privileges: BTreeSet::new(),
            grants: Vec::new(),
        }
    }

    fn apply_details(&mut self, details: UserDetails) {
        self.account = details.account.clone();
        self.server_privileges = details.server_privileges.clone();
        self.grants = details.grants.clone();
        self.original_account = Some((details.account.user.clone(), details.account.host.clone()));
        self.original = Some(details);
    }
}

/// The role/member edges of the account being edited.
pub(super) struct UserAccountContext {
    pub(super) user: String,
    pub(super) host: String,
    /// Role edges where this account is the member: `(role user, role host) -> admin option`.
    pub(super) roles: BTreeMap<(String, String), bool>,
    /// Role edges where this account is the role: `(member user, member host) -> admin option`.
    pub(super) members: BTreeMap<(String, String), bool>,
}

// ----- Layout helpers ------------------------------------------------------------------------

/// A titled block: a small heading above its content.
fn section(title: String, hint: Option<String>, content: AnyElement, theme: Theme) -> Div {
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(theme.text))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
    if let Some(hint) = hint {
        head = head.child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(theme.text_muted))
                .child(hint),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .child(head)
        .child(content)
}

/// One labeled identity-form row: a fixed-width label then its control.
fn create_row(label: String, control: AnyElement, theme: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(CREATE_LABEL_GAP))
        .min_h(px(CREATE_ROW_HEIGHT))
        .child(
            div()
                .w(px(CREATE_LABEL_WIDTH))
                .flex_none()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(label),
        )
        .child(control)
}

/// A text field of the given width (or its empty placeholder while the entity is missing).
fn sized_text_w(input: Option<&Entity<TextInput>>, theme: Theme, width: f32) -> AnyElement {
    match input {
        Some(input) => div()
            .w(px(width))
            .h(px(24.0))
            .flex_none()
            .child(input.clone())
            .into_any_element(),
        None => div()
            .w(px(width))
            .h(px(24.0))
            .flex_none()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .into_any_element(),
    }
}

/// A `CREATE_FIELD_WIDTH` text field.
fn sized_text(input: Option<&Entity<TextInput>>, theme: Theme) -> AnyElement {
    sized_text_w(input, theme, CREATE_FIELD_WIDTH)
}

/// A `CREATE_FIELD_WIDTH` combo box (or its empty placeholder while the entity is missing).
fn sized_combo(combo: Option<&Entity<ComboBox>>, theme: Theme) -> AnyElement {
    match combo {
        Some(combo) => div()
            .w(px(CREATE_FIELD_WIDTH))
            .h(px(20.0))
            .flex_none()
            .child(combo.clone())
            .into_any_element(),
        None => div()
            .w(px(CREATE_FIELD_WIDTH))
            .h(px(20.0))
            .flex_none()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .into_any_element(),
    }
}

/// A clickable checkbox row: a box then its label.
fn check_row(
    id: &'static str,
    label: String,
    checked: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(CREATE_ROW_HEIGHT))
        .cursor_pointer()
        .on_click(on_click)
        .child(checkbox_box(checked, theme))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(label),
        )
}

// ----- Window --------------------------------------------------------------------------------

/// The root view of the user-account OS window. It re-renders whenever `AppView` changes, so the
/// sections and the SQL preview stay live while the user edits them.
pub(super) struct UserCreateWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl UserCreateWindow {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        Self {
            app,
            _subscription: subscription,
        }
    }
}

impl Render for UserCreateWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.create_user_window_contents(cx))
    }
}

// ----- Actions -------------------------------------------------------------------------------

impl AppView {
    /// Open the account window for a new account on the connection the Users tab is scoped to.
    pub(super) fn open_create_user(&mut self, cx: &mut Context<'_, Self>) {
        self.open_user_window(None, cx);
    }

    /// Open an existing account in the window.
    pub(super) fn open_edit_user(&mut self, account: (String, String), cx: &mut Context<'_, Self>) {
        self.open_user_window(Some(account), cx);
    }

    /// Open the account window — creating an account when `account` is `None`, editing it
    /// otherwise. Both flows share this bootstrap so there is exactly one place that builds the
    /// window state and the field entities.
    fn open_user_window(&mut self, account: Option<(String, String)>, cx: &mut Context<'_, Self>) {
        if self.create_user_dialog.is_some() {
            self.focus_create_user_window(cx);
            return;
        }
        let Some(connection_index) = self.users_connection_index(cx) else {
            self.error_dialog = Some(t!("user.create.no_connection").to_string());
            cx.notify();
            return;
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            self.error_dialog = Some(t!("info.not_connected").to_string());
            cx.notify();
            return;
        };
        let plugins = connection.authentication_plugins();
        let plugin = plugins
            .first()
            .copied()
            .unwrap_or("caching_sha2_password")
            .to_string();
        let editing = account.is_some();
        let theme = self.theme;
        let weak = cx.weak_entity();
        let initial_user = account
            .as_ref()
            .map(|(user, _)| user.clone())
            .unwrap_or_default();
        let initial_host = account
            .as_ref()
            .map(|(_, host)| host.clone())
            .unwrap_or_else(|| "%".to_string());

        self.create_user_user = Some(make_create_field_input(
            theme,
            initial_user,
            false,
            CreateField::User,
            &weak,
            cx,
        ));
        self.create_user_host = Some(make_create_field_input(
            theme,
            initial_host,
            false,
            CreateField::Host,
            &weak,
            cx,
        ));
        self.create_user_password = Some(make_create_field_input(
            theme,
            String::new(),
            true,
            CreateField::Password,
            &weak,
            cx,
        ));
        self.create_user_confirm = Some(make_create_field_input(
            theme,
            String::new(),
            true,
            CreateField::Confirm,
            &weak,
            cx,
        ));
        self.create_user_expiry_days = Some(make_create_field_input(
            theme,
            "30".to_string(),
            false,
            CreateField::ExpiryDays,
            &weak,
            cx,
        ));
        self.create_user_max_questions = Some(make_create_field_input(
            theme,
            "0".to_string(),
            false,
            CreateField::MaxQuestions,
            &weak,
            cx,
        ));
        self.create_user_max_updates = Some(make_create_field_input(
            theme,
            "0".to_string(),
            false,
            CreateField::MaxUpdates,
            &weak,
            cx,
        ));
        self.create_user_max_connections = Some(make_create_field_input(
            theme,
            "0".to_string(),
            false,
            CreateField::MaxConnections,
            &weak,
            cx,
        ));
        self.create_user_max_user_connections = Some(make_create_field_input(
            theme,
            "0".to_string(),
            false,
            CreateField::MaxUserConnections,
            &weak,
            cx,
        ));
        self.create_user_database_search = Some(make_create_search_input(
            theme,
            t!("user.create.search_database").to_string(),
            CreateField::DatabaseSearch,
            &weak,
            cx,
        ));

        let plugin_options = plugins
            .iter()
            .map(|plugin| ComboOption::plain(*plugin))
            .collect();
        self.create_user_plugin_combo = Some(make_create_combo(
            theme,
            plugin_options,
            plugin.clone(),
            CreateCombo::Plugin,
            &weak,
            cx,
        ));
        self.create_user_expiry_combo = Some(make_create_combo(
            theme,
            vec![
                ComboOption::new("default", t!("user.expiry.default").to_string()),
                ComboOption::new("never", t!("user.expiry.never").to_string()),
                ComboOption::new("interval", t!("user.expiry.interval").to_string()),
            ],
            "default".to_string(),
            CreateCombo::Expiry,
            &weak,
            cx,
        ));

        let dialog = UserCreateDialog::new(connection_index, plugin, account);
        self.create_user_dialog = Some(dialog);
        if editing {
            self.load_edit_account(cx);
        } else {
            self.load_create_reference_data(cx);
        }
        self.open_create_user_window(cx);
        cx.notify();
    }

    /// Load the account being edited, plus the databases and accounts its sections show.
    fn load_edit_account(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let Some((user, host)) = dialog.original_account().cloned() else {
            self.load_create_reference_data(cx);
            return;
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let joined = {
                let connection = connection.clone();
                runtime
                    .spawn(async move {
                        let details = connection.user_details(&user, &host).await;
                        let accounts = connection.list_users().await;
                        let databases = connection.list_databases().await;
                        (details, accounts, databases)
                    })
                    .await
            };
            let (details, accounts, databases) = match joined {
                Ok(joined) => joined,
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        if let Some(dialog) = app.create_user_dialog.as_mut() {
                            dialog.loading = false;
                            dialog.error = Some(error.to_string());
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                dialog.loading = false;
                match details {
                    Ok(details) => dialog.apply_details(details),
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                if let Ok(accounts) = accounts {
                    dialog.accounts = accounts
                        .into_iter()
                        .map(|account| (account.user, account.host))
                        .collect();
                }
                match databases {
                    Ok(databases) => {
                        dialog.databases = databases.into_iter().map(|db| db.name).collect();
                    }
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                dialog.rebuild_db_grants();
                app.sync_create_editor_fields(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Load the databases (for the grant picker) and accounts (for the role lists).
    pub(super) fn load_create_reference_data(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.loading = false;
                dialog.error = Some(t!("info.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let joined = {
                let connection = connection.clone();
                runtime
                    .spawn(async move {
                        let databases = connection.list_databases().await;
                        let accounts = connection.list_users().await;
                        (databases, accounts)
                    })
                    .await
            };
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                match joined {
                    Ok((databases, accounts)) => {
                        match databases {
                            Ok(databases) => {
                                dialog.databases =
                                    databases.into_iter().map(|db| db.name).collect();
                            }
                            Err(error) => dialog.error = Some(error.to_string()),
                        }
                        if let Ok(accounts) = accounts
                            && dialog.accounts.is_empty()
                        {
                            dialog.accounts = accounts
                                .into_iter()
                                .map(|account| (account.user, account.host))
                                .collect();
                        }
                    }
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                dialog.rebuild_db_grants();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Load one database's tables for the 添加权限 picker's second level.
    pub(super) fn load_create_tables(&mut self, database: String, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        if dialog.tables.contains_key(&database) || dialog.loading_tables.contains(&database) {
            return;
        }
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return;
        };
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.loading_tables.insert(database.clone());
        }
        let runtime = self.runtime.clone();
        let query = database.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.list_tables(&query).await })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                dialog.loading_tables.remove(&database);
                match result {
                    Ok(Ok(tables)) => {
                        dialog.tables.insert(
                            database,
                            tables.into_iter().map(|table| table.name).collect(),
                        );
                    }
                    Ok(Err(error)) => dialog.error = Some(error.to_string()),
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Open the OS window hosting the account dialog (mirrors the Export/Import windows).
    fn open_create_user_window(&mut self, cx: &mut Context<'_, Self>) {
        if self.create_user_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let title = match self
            .create_user_dialog
            .as_ref()
            .and_then(|dialog| dialog.original_account())
        {
            Some((user, host)) => format!("{user}@{host} - {}", t!("user.create.edit_title")),
            None => t!("user.create.title").to_string(),
        };
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(900.0), px(720.0)), cx);
            let view_weak = weak.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.clone().into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    #[cfg(target_os = "windows")]
                    crate::win_resize::install(window);
                    window.activate_window();
                    let view =
                        cx.new(|cx| UserCreateWindow::new(view_weak.clone(), &app_entity, cx));
                    cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
                },
            );
            match opened {
                Ok(handle) => app.update(cx, |app, cx| {
                    app.create_user_window = Some(handle);
                    cx.notify();
                }),
                Err(error) => app.update(cx, |app, cx| {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }),
            }
        });
    }

    /// Raise the already-open account window.
    fn focus_create_user_window(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(handle) = self.create_user_window {
            let _ = handle.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            });
        }
    }

    /// Close the window and drop its entities.
    pub(super) fn create_user_close(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.cancel_create_user(cx);
        window.remove_window();
    }

    /// Close the window and drop its entities.
    pub(super) fn cancel_create_user(&mut self, cx: &mut Context<'_, Self>) {
        self.create_user_dialog = None;
        self.create_user_user = None;
        self.create_user_host = None;
        self.create_user_password = None;
        self.create_user_confirm = None;
        self.create_user_plugin_combo = None;
        self.create_user_expiry_combo = None;
        self.create_user_expiry_days = None;
        self.create_user_max_questions = None;
        self.create_user_max_updates = None;
        self.create_user_max_connections = None;
        self.create_user_max_user_connections = None;
        self.create_user_database_search = None;
        self.create_user_window = None;
        cx.notify();
    }

    /// Switch the window's active navigation section.
    pub(super) fn set_create_section(&mut self, section: UserSection, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.section = section;
        }
        cx.notify();
    }

    /// Write one of the window's text fields.
    pub(super) fn set_create_field(
        &mut self,
        field: CreateField,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            match field {
                CreateField::User => dialog.editor.account.user = text.to_string(),
                CreateField::Host => dialog.editor.account.host = text.to_string(),
                CreateField::Password => dialog.editor.password = text.to_string(),
                CreateField::Confirm => dialog.editor.guard_password = text.to_string(),
                CreateField::ExpiryDays => {
                    if let Ok(days) = text.trim().parse::<u32>() {
                        dialog.expiry_days = days;
                    }
                }
                CreateField::MaxQuestions => {
                    dialog.editor.account.max_questions = parse_limit(text)
                }
                CreateField::MaxUpdates => dialog.editor.account.max_updates = parse_limit(text),
                CreateField::MaxConnections => {
                    dialog.editor.account.max_connections = parse_limit(text)
                }
                CreateField::MaxUserConnections => {
                    dialog.editor.account.max_user_connections = parse_limit(text)
                }
                CreateField::DatabaseSearch => dialog.database_search = text.to_string(),
            }
        }
        cx.notify();
    }

    /// Apply a plugin / password-expiry dropdown pick.
    fn create_combo_selected(
        &mut self,
        field: CreateCombo,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            match field {
                CreateCombo::Plugin => dialog.editor.account.plugin = value.to_string(),
                CreateCombo::Expiry => dialog.expiry = CreateExpiry::from_value(value),
            }
        }
        cx.notify();
    }

    /// Toggle the account-locked flag.
    pub(super) fn toggle_create_locked(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.editor.account.account_locked = !dialog.editor.account.account_locked;
        }
        cx.notify();
    }

    /// Toggle the 修改登录密码 checkbox; clearing it also drops any typed password.
    pub(super) fn toggle_create_change_password(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.change_password = !dialog.change_password;
            if !dialog.change_password {
                dialog.editor.password.clear();
                dialog.editor.guard_password.clear();
            }
        }
        if let Some(input) = self.create_user_password.clone() {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        if let Some(input) = self.create_user_confirm.clone() {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        cx.notify();
    }

    /// Set the host from one of the 主机地址 quick chips.
    pub(super) fn set_create_host(&mut self, host: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.editor.account.host = host.clone();
        }
        if let Some(input) = self.create_user_host.clone() {
            input.update(cx, |input, cx| input.set_text(host, cx));
        }
        cx.notify();
    }

    /// Toggle one server privilege.
    pub(super) fn toggle_create_server_privilege(
        &mut self,
        privilege: Privilege,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && !dialog.editor.server_privileges.remove(&privilege)
        {
            dialog.editor.server_privileges.insert(privilege);
        }
        cx.notify();
    }

    /// Replace the server privileges with a template's set.
    pub(super) fn apply_create_server_template(
        &mut self,
        template: ServerTemplate,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.editor.server_privileges = template.privileges();
        }
        cx.notify();
    }

    /// Select the database shown in the 权限 detail pane, loading its tables for 指定具体表.
    pub(super) fn activate_create_database(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let load = {
            let Some(dialog) = self.create_user_dialog.as_mut() else {
                return;
            };
            if index >= dialog.db_grants.len() {
                return;
            }
            dialog.active_database = Some(index);
            let row = &mut dialog.db_grants[index];
            if row.scope == GrantScope::SpecificTables && row.active_table.is_none() {
                row.active_table = row.tables.first().cloned();
            }
            (row.scope == GrantScope::SpecificTables).then(|| row.name.clone())
        };
        if let Some(database) = load {
            self.load_create_tables(database, cx);
        } else {
            cx.notify();
        }
    }

    /// Grant or revoke one database.
    pub(super) fn toggle_create_database(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(row) = dialog.db_grants.get_mut(index)
        {
            row.enabled = !row.enabled;
        }
        cx.notify();
    }

    /// Clear every database's grant.
    pub(super) fn clear_create_databases(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            for row in &mut dialog.db_grants {
                row.enabled = false;
                row.scope = GrantScope::AllTables;
                row.tables.clear();
                row.active_table = None;
                row.privileges.clear();
                row.table_privileges.clear();
            }
        }
        cx.notify();
    }

    /// Set the active database's table scope (全部表 / 指定具体表).
    pub(super) fn set_create_grant_scope(&mut self, scope: GrantScope, cx: &mut Context<'_, Self>) {
        let load = {
            let Some(dialog) = self.create_user_dialog.as_mut() else {
                return;
            };
            let Some(index) = dialog.active_database else {
                return;
            };
            let Some(row) = dialog.db_grants.get_mut(index) else {
                return;
            };
            row.scope = scope;
            if scope == GrantScope::SpecificTables {
                row.enabled = true;
                if row.active_table.is_none() {
                    row.active_table = row.tables.first().cloned();
                }
                Some(row.name.clone())
            } else {
                row.tables.clear();
                row.active_table = None;
                row.table_privileges.clear();
                None
            }
        };
        if let Some(database) = load {
            self.load_create_tables(database, cx);
        } else {
            cx.notify();
        }
    }

    /// Apply a quick preset to the active target: the whole database for 全部表, else the active
    /// table.
    pub(super) fn apply_create_database_template(
        &mut self,
        template: DbTemplate,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(index) = dialog.active_database
            && let Some(row) = dialog.db_grants.get_mut(index)
        {
            if row.scope == GrantScope::SpecificTables && row.active_table.is_none() {
                // A preset needs a table to land on; default to the first picked one.
                if let Some(first) = row.tables.first().cloned() {
                    row.activate_table(&first);
                }
            }
            let privileges = template.privileges();
            let is_all_tables = row.scope == GrantScope::AllTables;
            if let Some(target) = row.active_privileges_mut() {
                *target = privileges.clone();
            }
            if is_all_tables {
                row.enabled = !privileges.is_empty();
            }
        }
        cx.notify();
    }

    /// Toggle one privilege on the active target (whole database or active table).
    pub(super) fn toggle_create_database_privilege(
        &mut self,
        privilege: Privilege,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(index) = dialog.active_database
            && let Some(row) = dialog.db_grants.get_mut(index)
        {
            if let Some(target) = row.active_privileges_mut()
                && !target.remove(&privilege)
            {
                target.insert(privilege);
            }
            // Keep the row participating even at zero privileges so clearing a table revokes it.
            row.enabled = true;
        }
        cx.notify();
    }

    /// Select every object privilege on the active target, or clear them if all are already set.
    pub(super) fn toggle_create_database_privileges_all(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(index) = dialog.active_database
            && let Some(row) = dialog.db_grants.get_mut(index)
        {
            let is_all_tables = row.scope == GrantScope::AllTables;
            let mut cleared = false;
            if let Some(target) = row.active_privileges_mut() {
                let all: BTreeSet<Privilege> = Privilege::OBJECT.into_iter().collect();
                if *target == all {
                    target.clear();
                    cleared = true;
                } else {
                    *target = all;
                }
            }
            // Clearing a whole-database grant leaves the database unconfigured; a table scope stays
            // selected so its revoke is still emitted.
            row.enabled = !(cleared && is_all_tables);
        }
        cx.notify();
    }

    /// Make one table the active 指定具体表 row the detail pane edits, selecting it if needed.
    pub(super) fn activate_create_table(&mut self, name: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(index) = dialog.active_database
            && let Some(row) = dialog.db_grants.get_mut(index)
        {
            row.activate_table(&name);
        }
        cx.notify();
    }

    /// Toggle one table in the active database's 指定具体表 list.
    pub(super) fn toggle_create_table(&mut self, name: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(index) = dialog.active_database
            && let Some(row) = dialog.db_grants.get_mut(index)
        {
            if let Some(position) = row.tables.iter().position(|table| *table == name) {
                row.tables.remove(position);
                row.table_privileges.remove(&name);
                if row.active_table.as_deref() == Some(name.as_str()) {
                    row.active_table = row.tables.first().cloned();
                }
            } else {
                row.activate_table(&name);
            }
        }
        cx.notify();
    }

    /// Select or clear every table of the active database's 指定具体表 list.
    pub(super) fn set_create_tables_all(&mut self, all: bool, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_mut() else {
            return;
        };
        let Some(index) = dialog.active_database else {
            return;
        };
        let Some(database) = dialog.db_grants.get(index).map(|row| row.name.clone()) else {
            return;
        };
        let tables = dialog.tables.get(&database).cloned().unwrap_or_default();
        if let Some(row) = dialog.db_grants.get_mut(index) {
            if all {
                row.tables = tables;
                if row.active_table.is_none() {
                    row.active_table = row.tables.first().cloned();
                }
                row.enabled = true;
            } else {
                row.tables.clear();
                row.active_table = None;
                row.table_privileges.clear();
                row.enabled = false;
            }
        }
        cx.notify();
    }

    /// Toggle one role/member edge. Turning a grant off also drops its admin option.
    pub(super) fn toggle_create_membership(
        &mut self,
        member_of: bool,
        key: (String, String),
        cx: &mut Context<'_, Self>,
    ) {
        let Some(dialog) = self.create_user_dialog.as_mut() else {
            return;
        };
        let Some(context) = dialog.context.as_mut() else {
            return;
        };
        let edges = if member_of {
            &mut context.roles
        } else {
            &mut context.members
        };
        if edges.remove(&key).is_none() {
            edges.insert(key, false);
        }
        cx.notify();
    }

    /// Toggle the admin option of one role/member edge, granting it first if needed.
    pub(super) fn toggle_create_role_admin(
        &mut self,
        member_of: bool,
        key: (String, String),
        cx: &mut Context<'_, Self>,
    ) {
        let Some(dialog) = self.create_user_dialog.as_mut() else {
            return;
        };
        let Some(context) = dialog.context.as_mut() else {
            return;
        };
        let edges = if member_of {
            &mut context.roles
        } else {
            &mut context.members
        };
        match edges.get_mut(&key) {
            Some(admin) => *admin = !*admin,
            None => {
                edges.insert(key, true);
            }
        }
        cx.notify();
    }

    /// Push the loaded account into the window's managed text fields and combos.
    pub(super) fn sync_create_editor_fields(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let account = dialog.editor.account.clone();
        let editing = dialog.is_edit();
        let password_cleared = dialog.editor.password.is_empty();
        let expiry = match account.password_lifetime {
            None => CreateExpiry::Default,
            Some(0) => CreateExpiry::Never,
            Some(_) => CreateExpiry::Interval,
        };
        let expiry_days = account.password_lifetime.unwrap_or(30).max(1);
        if let Some(input) = self.create_user_user.clone() {
            input.update(cx, |input, cx| input.set_text(account.user.clone(), cx));
        }
        if let Some(input) = self.create_user_host.clone() {
            input.update(cx, |input, cx| input.set_text(account.host.clone(), cx));
        }
        if password_cleared {
            for input in [
                self.create_user_password.clone(),
                self.create_user_confirm.clone(),
            ]
            .into_iter()
            .flatten()
            {
                input.update(cx, |input, cx| input.set_text(String::new(), cx));
            }
        }
        // Like Navicat: an existing password shows as a masked hint (never the plaintext, which the
        // server does not store) so the field does not look empty.
        let password_hint = if editing && account.password_set {
            SharedString::from("••••••••••")
        } else {
            SharedString::default()
        };
        for input in [
            self.create_user_password.clone(),
            self.create_user_confirm.clone(),
        ]
        .into_iter()
        .flatten()
        {
            input.update(cx, |input, cx| {
                input.set_placeholder(password_hint.clone(), cx)
            });
        }
        if let Some(input) = self.create_user_expiry_days.clone() {
            input.update(cx, |input, cx| input.set_text(expiry_days.to_string(), cx));
        }
        for (input, value) in [
            (&self.create_user_max_questions, account.max_questions),
            (&self.create_user_max_updates, account.max_updates),
            (&self.create_user_max_connections, account.max_connections),
            (
                &self.create_user_max_user_connections,
                account.max_user_connections,
            ),
        ] {
            if let Some(input) = input.clone() {
                input.update(cx, |input, cx| input.set_text(value.to_string(), cx));
            }
        }
        if let Some(combo) = self.create_user_plugin_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_selected(account.plugin.clone(), cx)
            });
        }
        if let Some(combo) = self.create_user_expiry_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_selected(expiry_value(expiry).to_string(), cx)
            });
        }
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.expiry = expiry;
            dialog.expiry_days = expiry_days;
        }
        cx.notify();
    }

    /// Reload the account being edited (after a save) and re-sync the window's fields.
    fn reload_create_account(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let connection_index = dialog.connection_index;
        let Some((user, host)) = dialog.original_account().cloned() else {
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.loading = false;
            }
            cx.notify();
            return;
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.loading = false;
                dialog.error = Some(t!("info.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.user_details(&user, &host).await })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                dialog.loading = false;
                match result {
                    Ok(Ok(details)) => {
                        dialog.apply_details(details);
                        app.sync_create_editor_fields(cx);
                    }
                    Ok(Err(error)) => dialog.error = Some(error.to_string()),
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The edit the window currently describes. `None` while the window is closed.
    pub(super) fn create_user_edit(&self) -> Option<UserEdit> {
        let dialog = self.create_user_dialog.as_ref()?;
        let (roles, members) = match dialog.context.as_ref() {
            Some(context) => (
                context
                    .roles
                    .iter()
                    .map(|((user, host), admin)| (user.clone(), host.clone(), *admin))
                    .collect(),
                context
                    .members
                    .iter()
                    .map(|((user, host), admin)| (user.clone(), host.clone(), *admin))
                    .collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        Some(UserEdit {
            original: dialog.editor.original.clone(),
            account: UserAccount {
                password_lifetime: match dialog.expiry {
                    CreateExpiry::Default => dialog.editor.account.password_lifetime,
                    CreateExpiry::Never => Some(0),
                    CreateExpiry::Interval => Some(dialog.expiry_days.max(1)),
                },
                ..dialog.editor.account.clone()
            },
            password: if dialog.change_password && !dialog.editor.password.is_empty() {
                Some(dialog.editor.password.clone())
            } else {
                None
            },
            server_privileges: dialog.editor.server_privileges.clone(),
            grants: dialog.object_grants(),
            roles,
            members,
        })
    }

    /// The SQL the window would run, for the SQL 预览 section.
    pub(super) fn create_user_sql(&self) -> String {
        let Some(connection_index) = self
            .create_user_dialog
            .as_ref()
            .map(|dialog| dialog.connection_index)
        else {
            return String::new();
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            return String::new();
        };
        match self.create_user_edit() {
            Some(edit) => connection.user_edit_sql(&edit),
            None => String::new(),
        }
    }

    /// Whether Save would change anything: a new account always would, an edit only when its diff
    /// is non-empty. Drives the footer's disabled Save button.
    pub(super) fn create_user_has_changes(&self) -> bool {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return false;
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return false;
        };
        self.create_user_edit()
            .map(|edit| !connection.user_edit_groups(&edit).is_empty())
            .unwrap_or(false)
    }

    /// Copy the SQL 预览 script to the clipboard.
    pub(super) fn copy_create_user_sql(&mut self, cx: &mut Context<'_, Self>) {
        let sql = self.create_user_sql();
        if !sql.trim().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(sql));
        }
    }

    /// Validate and save the account, then reload it in place.
    pub(super) fn submit_create_user(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        if dialog.saving || dialog.loading {
            return;
        }
        if dialog.editor.account.user.trim().is_empty() {
            let message = t!("user.name_required").to_string();
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.error = Some(message);
            }
            cx.notify();
            return;
        }
        if dialog.is_edit() && dialog.change_password && dialog.editor.password.is_empty() {
            let message = t!("user.create.password_required").to_string();
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.error = Some(message);
            }
            cx.notify();
            return;
        }
        if dialog.editor.password != dialog.editor.guard_password {
            let message = t!("user.password_mismatch").to_string();
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.error = Some(message);
            }
            cx.notify();
            return;
        }
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.confirm_open = true;
            dialog.error = None;
        }
        cx.notify();
    }

    /// Close the 确认并执行 dialog without saving.
    pub(super) fn close_create_confirm(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.confirm_open = false;
        }
        cx.notify();
    }

    /// Run the save the 确认并执行 dialog previewed. Split from [`Self::submit_create_user`] so the
    /// user always sees the change diff before anything is executed.
    pub(super) fn execute_create_user(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        if dialog.saving {
            return;
        }
        let Some(edit) = self.create_user_edit() else {
            return;
        };
        let connection_index = dialog.connection_index;
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.saving = true;
            dialog.saved = false;
            dialog.confirm_open = false;
            dialog.error = None;
        }
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.saving = false;
                dialog.error = Some(t!("info.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.save_user(&edit).await })
                .await;
            let _ = this.update(cx, |app, cx| {
                let succeeded = matches!(result, Ok(Ok(())));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        if let Some(dialog) = app.create_user_dialog.as_mut() {
                            dialog.saving = false;
                            dialog.error = Some(error.to_string());
                        }
                    }
                    Err(error) => {
                        if let Some(dialog) = app.create_user_dialog.as_mut() {
                            dialog.saving = false;
                            dialog.error = Some(error.to_string());
                        }
                    }
                }
                if succeeded {
                    app.refresh_users(cx);
                    {
                        let Some(dialog) = app.create_user_dialog.as_mut() else {
                            return;
                        };
                        dialog.saving = false;
                        dialog.loading = true;
                        dialog.saved = true;
                        dialog.editor.password.clear();
                        dialog.editor.guard_password.clear();
                        // Adopt the saved identity so a re-save targets it (and a rename sticks).
                        let new_user = dialog.editor.account.user.clone();
                        let new_host = dialog.editor.account.host.clone();
                        dialog.editor.original_account = Some((new_user.clone(), new_host.clone()));
                        if let Some(context) = dialog.context.as_mut() {
                            context.user = new_user;
                            context.host = new_host;
                        }
                    }
                    app.reload_create_account(cx);
                    // Confirm the save in the footer, then fade it after a moment.
                    let executor = cx.background_executor().clone();
                    cx.spawn(async move |this, cx| {
                        executor.timer(std::time::Duration::from_millis(2500)).await;
                        let _ = this.update(cx, |app, cx| {
                            if let Some(dialog) = app.create_user_dialog.as_mut() {
                                dialog.saved = false;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The annotated SQL the 确认并执行 dialog shows: each group of statements is preceded by a
    /// localized comment, like Navicat's change diff.
    pub(super) fn create_user_preview(&self) -> String {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return String::new();
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return String::new();
        };
        let Some(edit) = self.create_user_edit() else {
            return String::new();
        };
        let subject = match dialog.original_account() {
            Some((user, host)) => format!("{user}@{host}"),
            None => format!(
                "{}@{}",
                dialog.editor.account.user, dialog.editor.account.host
            ),
        };
        let mut out = String::new();
        out.push_str(&format!(
            "-- {}\n",
            t!("user.create.preview_header", account = subject)
        ));
        if dialog.is_edit() {
            let note = if dialog.change_password && !dialog.editor.password.is_empty() {
                t!("user.create.preview_password_changed")
            } else {
                t!("user.create.preview_password_kept")
            };
            out.push_str(&format!("-- {note}\n"));
        }
        let groups = connection.user_edit_groups(&edit);
        if groups.is_empty() {
            out.push_str(&format!("-- {}\n", t!("design.no_changes")));
            return out;
        }
        for (index, (section, statements)) in groups.into_iter().enumerate() {
            out.push('\n');
            out.push_str(&format!(
                "-- {} {}: {}\n",
                t!("user.create.preview_change"),
                index + 1,
                t!(user_edit_section_key(section))
            ));
            for statement in statements {
                out.push_str(&statement);
                out.push_str(";\n");
            }
        }
        // `edit_statements` refreshes the grant tables once after a non-empty script; mirror it so
        // the preview matches exactly what Save runs.
        out.push('\n');
        out.push_str(&format!("-- {}\n", t!("user.create.preview_refresh")));
        out.push_str("FLUSH PRIVILEGES;\n");
        out
    }
}

// ----- Window field entities -----------------------------------------------------------------

/// Parse a resource-limit field; an empty or invalid value means "no limit" (0).
fn parse_limit(text: &str) -> u64 {
    text.trim().parse::<u64>().unwrap_or(0)
}

/// The i18n key naming one group of the account-change diff.
fn user_edit_section_key(section: UserEditSection) -> &'static str {
    match section {
        UserEditSection::Account => "user.create.preview.account",
        UserEditSection::ServerPrivileges => "user.tab.server_privileges",
        UserEditSection::ObjectGrants => "user.tab.privileges",
        UserEditSection::Roles => "user.tab.roles",
    }
}

/// Build one identity/limit field of the window. Edits write straight into the window state so the
/// SQL preview and the save path always see the latest text.
fn make_create_field_input(
    theme: Theme,
    value: String,
    masked: bool,
    field: CreateField,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    let cancel = app.clone();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            value,
            TextInputOptions {
                masked,
                placeholder: SharedString::default(),
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| app.set_create_field(field, text, cx));
        }))
        .on_submit(Rc::new(move |_window, cx| {
            let _ = submit.update(cx, |app, cx| app.submit_create_user(cx));
        }))
        .on_cancel(Rc::new(move |_window, cx| {
            let _ = cancel.update(cx, |app, cx| app.cancel_create_user(cx));
        }))
    })
}

/// Build one of the window's search fields (compact, with a leading magnifier icon).
fn make_create_search_input(
    theme: Theme,
    placeholder: String,
    field: CreateField,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            "",
            TextInputOptions {
                placeholder: placeholder.into(),
                icon: Some("icons/search.svg"),
                size: Some(gpui_kit::component::Size::XSmall),
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| app.set_create_field(field, text, cx));
        }))
    })
}

/// Build one of the window's dropdowns.
fn make_create_combo(
    theme: Theme,
    options: Vec<ComboOption>,
    selected: String,
    field: CreateCombo,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<ComboBox> {
    let weak = app.clone();
    cx.new(move |cx| {
        ComboBox::new(theme, options, selected, CREATE_FIELD_WIDTH, cx)
            .field_width(CREATE_FIELD_WIDTH)
            .on_select(Rc::new(move |value, _window, cx| {
                let _ = weak.update(cx, |app, cx| app.create_combo_selected(field, value, cx));
            }))
    })
}

// ----- Rendering -----------------------------------------------------------------------------

impl AppView {
    /// The window's contents: the child titlebar, the section navigation + content, and the footer.
    pub(super) fn create_user_window_contents(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let title = match dialog.original_account() {
            Some((user, host)) => format!("{user}@{host} - {}", t!("user.create.edit_title")),
            None => t!("user.create.title").to_string(),
        };

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .child(export::child_window_titlebar(title, theme))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .child(self.render_create_nav(cx))
                    .child(
                        div()
                            .id("user-create-content")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_y_scroll()
                            .p_4()
                            .child(self.create_user_body(cx)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .px_4()
                    .bg(rgb(theme.dialog_bg))
                    .border_t_1()
                    .border_color(rgb(theme.border))
                    .child(self.create_user_footer(cx)),
            );

        if dialog.confirm_open {
            root = root.child(self.render_create_confirm(cx));
        }
        root.into_any_element()
    }

    /// The 确认并执行 dialog: the annotated change diff plus 关闭 / 确认并执行.
    fn render_create_confirm(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let preview = self.create_user_preview();

        let mut code = div()
            .id("user-create-confirm-sql")
            .flex()
            .flex_col()
            .max_h(px(360.0))
            .overflow_y_scroll()
            .rounded(px(6.0))
            .bg(rgb(0x0d1117))
            .p_3();
        for line in preview.lines() {
            let comment = line.trim_start().starts_with("--");
            code = code.child(
                div()
                    .font_family("Consolas")
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(if comment { 0x8b949e } else { 0xc9d1d9 }))
                    .child(line.to_string()),
            );
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("user.create.confirm_title").to_string()),
            )
            .child(
                div()
                    .id("user-create-confirm-copy")
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.primary))
                    .hover(move |style| style.text_color(rgb(theme.text)))
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.copy_create_user_sql(cx)),
                    )
                    .child(t!("user.create.copy_sql").to_string()),
            );

        let panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .w(px(720.0))
            .max_h(px(560.0))
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded(px(8.0))
            .shadow(ui::dialog_shadow())
            .child(header)
            .child(code)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.create.confirm_hint").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .w_full()
                    .child(self.dialog_button(
                        "user-create-confirm-close",
                        t!("user.create.confirm_close").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| this.close_create_confirm(cx)),
                    ))
                    .child(self.dialog_button(
                        "user-create-confirm-execute",
                        t!("user.create.confirm_execute").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| this.execute_create_user(cx)),
                    )),
            )
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            });

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .bg(rgba(0x00000040))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.close_create_confirm(cx)),
            )
            .child(panel)
            .into_any_element()
    }

    /// The left section navigation.
    fn render_create_nav(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let mut nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(CREATE_NAV_WIDTH))
            .h_full()
            .py_2()
            .bg(rgb(theme.sidebar_bg))
            .border_r_1()
            .border_color(rgb(theme.border));
        for section in UserSection::ALL {
            let active = dialog.section == section;
            let icon_color = if active {
                theme.tree_selected_text
            } else {
                theme.text_muted
            };
            nav = nav.child(
                div()
                    .id(section.id())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(30.0))
                    .px_3()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(rgb(if active {
                        theme.tree_selected_text
                    } else {
                        theme.text
                    }))
                    .when(active, move |style| style.bg(rgb(theme.tree_selected_bg)))
                    .when(!active, move |style| {
                        style
                            .cursor_pointer()
                            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_create_section(section, cx)
                    }))
                    .child(tree_icon(section.icon(), icon_color))
                    .child(t!(section.label_key()).to_string()),
            );
        }
        nav.into_any_element()
    }

    /// The body of the active section.
    fn create_user_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        match dialog.section {
            UserSection::General => self.render_create_general(cx),
            UserSection::ServerPrivileges => self.render_create_server_privileges(cx),
            UserSection::ObjectPrivileges => self.render_create_grants(cx),
            UserSection::Roles => self.render_create_roles(cx),
            UserSection::Sql => self.render_create_sql(cx),
        }
    }

    /// 常规: identity, authentication, attributes and resource limits.
    fn render_create_general(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let account = &dialog.editor.account;

        let mut identity = div().flex().flex_col().gap_2().w_full();
        identity = identity
            .child(create_row(
                t!("user.field.username").to_string(),
                sized_text(self.create_user_user.as_ref(), theme),
                theme,
            ))
            .child(create_row(
                t!("user.field.host").to_string(),
                sized_text(self.create_user_host.as_ref(), theme),
                theme,
            ));

        // 主机地址 quick chips, like the prototype's `% (任意网络)` / `localhost` shortcuts.
        let mut host_choices = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().w(px(CREATE_LABEL_WIDTH)).flex_none());
        for (label_key, value) in [
            ("user.create.host_any", "%"),
            ("user.create.host_local", "localhost"),
            ("user.create.host_lan", "192.168.1.%"),
        ] {
            let host = value.to_string();
            host_choices = host_choices.child(
                div()
                    .id(SharedString::from(format!("user-create-host-{value}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(20.0))
                    .px_2()
                    .rounded(px(4.0))
                    .text_size(px(11.0))
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text_muted))
                    .cursor_pointer()
                    .hover(move |style| style.text_color(rgb(theme.text)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_create_host(host.clone(), cx)
                    }))
                    .child(t!(label_key).to_string()),
            );
        }
        identity = identity.child(host_choices).child(create_row(
            t!("user.field.plugin").to_string(),
            sized_combo(self.create_user_plugin_combo.as_ref(), theme),
            theme,
        ));

        if dialog.is_edit() {
            identity = identity.child(create_row(
                String::new(),
                check_row(
                    "user-create-change-password",
                    t!("user.create.change_password").to_string(),
                    dialog.change_password,
                    theme,
                    cx.listener(|this, _event, _window, cx| this.toggle_create_change_password(cx)),
                )
                .into_any_element(),
                theme,
            ));
        }
        if !dialog.is_edit() || dialog.change_password {
            identity = identity
                .child(create_row(
                    t!("user.field.password").to_string(),
                    sized_text(self.create_user_password.as_ref(), theme),
                    theme,
                ))
                .child(create_row(
                    t!("user.field.confirm_password").to_string(),
                    sized_text(self.create_user_confirm.as_ref(), theme),
                    theme,
                ));
            let hint: String = if dialog.is_edit() {
                if account.password_set {
                    t!("user.create.password_set_hint").to_string()
                } else {
                    t!("user.create.password_unset_hint").to_string()
                }
            } else {
                t!("user.create.password_hint").to_string()
            };
            identity = identity.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(CREATE_LABEL_GAP))
                    .child(div().w(px(CREATE_LABEL_WIDTH)).flex_none())
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(hint),
                    ),
            );
        }

        let mut status = div().flex().flex_col().gap_2().w_full();
        // 锁定该账号 sits beside the expiry dropdown, like the prototype.
        status = status.child(create_row(
            t!("user.field.password_expiry").to_string(),
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .child(sized_combo(self.create_user_expiry_combo.as_ref(), theme))
                .child(
                    check_row(
                        "user-create-locked",
                        t!("user.field.locked_account").to_string(),
                        account.account_locked,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.toggle_create_locked(cx)),
                    )
                    .into_any_element(),
                )
                .into_any_element(),
            theme,
        ));
        if dialog.expiry == CreateExpiry::Interval {
            status = status.child(create_row(
                String::new(),
                sized_text(self.create_user_expiry_days.as_ref(), theme),
                theme,
            ));
        }

        let mut limits = div().flex().flex_col().gap_2().w_full();
        for (label_key, input) in [
            ("user.field.max_questions", &self.create_user_max_questions),
            ("user.field.max_updates", &self.create_user_max_updates),
            (
                "user.field.max_connections",
                &self.create_user_max_connections,
            ),
            (
                "user.field.max_user_connections",
                &self.create_user_max_user_connections,
            ),
        ] {
            limits = limits.child(create_row(
                t!(label_key).to_string(),
                sized_text_w(input.as_ref(), theme, CREATE_LIMIT_WIDTH),
                theme,
            ));
        }

        div()
            .id("user-create-general")
            .flex()
            .flex_col()
            .gap_5()
            .w_full()
            .child(section(
                t!("user.create.login").to_string(),
                None,
                identity.into_any_element(),
                theme,
            ))
            .child(section(
                t!("user.create.status").to_string(),
                None,
                status.into_any_element(),
                theme,
            ))
            .child(section(
                t!("user.create.limits").to_string(),
                Some(t!("user.create.limits_hint").to_string()),
                limits.into_any_element(),
                theme,
            ))
            .into_any_element()
    }

    /// 服务器权限: the global privilege grid with one-click templates.
    fn render_create_server_privileges(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let checked_count = dialog.editor.server_privileges.len();

        let mut templates = div().flex().flex_row().items_center().gap_2().w_full();
        for template in ServerTemplate::ALL {
            let active = dialog.editor.server_privileges == template.privileges();
            templates = templates.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-template-{}",
                        template.label_key()
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .px_3()
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.primary))
                            .text_color(rgb(if theme.is_dark() {
                                theme.window_bg
                            } else {
                                0xffffff
                            }))
                    })
                    .when(!active, move |style| {
                        style
                            .bg(rgb(theme.button_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text_muted))
                            .hover(move |style| style.text_color(rgb(theme.text)))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.apply_create_server_template(template, cx)
                    }))
                    .child(t!(template.label_key()).to_string()),
            );
        }

        // Grouped like the prototype, so the global set reads as categories instead of one long grid.
        let mut groups = div().flex().flex_col().gap_3().w_full();
        for (group_key, privileges) in server_privilege_groups() {
            let mut grid = div().flex().flex_row().flex_wrap().w_full();
            for (position, privilege) in privileges.into_iter().enumerate() {
                let checked = dialog.editor.server_privileges.contains(&privilege);
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!(
                            "user-create-server-priv-{group_key}-{position}"
                        )))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w(px(210.0))
                        .h(px(CREATE_ROW_HEIGHT))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_create_server_privilege(privilege, cx)
                        }))
                        .child(checkbox_box(checked, theme))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(theme.text))
                                .child(t!(privilege.label_key()).to_string()),
                        ),
                );
            }
            groups = groups.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w_full()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!(group_key).to_string()),
                    )
                    .child(grid),
            );
        }

        div()
            .id("user-create-server")
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .child(section(
                t!("user.create.template").to_string(),
                None,
                templates.into_any_element(),
                theme,
            ))
            .child(section(
                t!("user.tab.server_privileges").to_string(),
                Some(format!(
                    "{} · {}",
                    t!("user.create.server_privileges_hint", count = checked_count),
                    t!("user.create.server_danger")
                )),
                groups.into_any_element(),
                theme,
            ))
            .into_any_element()
    }

    /// 权限: a database list on the left and the selected database's privilege detail on the right.
    fn render_create_grants(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let search = dialog.database_search.to_lowercase();
        let visible: Vec<usize> = dialog
            .db_grants
            .iter()
            .enumerate()
            .filter(|(_, row)| search.is_empty() || row.name.to_lowercase().contains(&search))
            .map(|(index, _)| index)
            .collect();

        div()
            .id("user-create-grants")
            .flex()
            .flex_row()
            .gap_4()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .child(self.render_create_database_list(&visible, cx))
            .child(self.render_create_database_detail(cx))
            .into_any_element()
    }

    /// The 权限 section's left list: the filter, the count / clear-all row, then every database.
    fn render_create_database_list(
        &self,
        visible: &[usize],
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let search = self
            .create_user_database_search
            .clone()
            .map(|input| div().w_full().h(px(24.0)).child(input).into_any_element())
            .unwrap_or_else(|| div().into_any_element());

        let mut list = div()
            .id("user-create-db-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&dialog.db_scroll);
        for &index in visible {
            let Some(row) = dialog.db_grants.get(index) else {
                continue;
            };
            let active = dialog.active_database == Some(index);
            let enabled = row.enabled;
            let name = row.name.clone();
            let (badge, authorized) = row.summary();
            let mut entry = div()
                .id(SharedString::from(format!("user-create-db-{index}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(34.0))
                .px_2()
                .flex_none()
                .rounded(px(4.0))
                .cursor_pointer()
                .when(active, move |style| style.bg(rgb(theme.tree_selected_bg)))
                .when(!active, move |style| {
                    style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                })
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.activate_create_database(index, cx)
                }))
                .child(
                    div()
                        .id(SharedString::from(format!("user-create-db-check-{index}")))
                        .flex()
                        .items_center()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_create_database(index, cx);
                        }))
                        .child(checkbox_box(enabled, theme)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .text_color(rgb(if active {
                            theme.tree_selected_text
                        } else {
                            theme.text
                        }))
                        .child(name),
                );
            if authorized {
                entry = entry.child(
                    div()
                        .flex_none()
                        .px_2()
                        .py_0p5()
                        .rounded(px(9.0))
                        .text_size(px(10.5))
                        .bg(rgb(theme.tree_hover_bg))
                        .text_color(rgb(theme.primary))
                        .child(badge),
                );
            } else {
                entry = entry.child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(rgb(theme.text_muted))
                        .child(badge),
                );
            }
            list = list.child(entry);
        }

        let header = div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .pb_2()
            .child(search)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(
                        t!("user.create.database_count", count = dialog.db_grants.len())
                            .to_string(),
                    )
                    .child(
                        div()
                            .id("user-create-db-clear")
                            .cursor_pointer()
                            .text_color(rgb(theme.primary))
                            .hover(move |style| style.text_color(rgb(theme.text)))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.clear_create_databases(cx)
                            }))
                            .child(t!("user.create.clear_all").to_string()),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .w(px(270.0))
            .flex_none()
            .h_full()
            .min_h(px(0.0))
            .pr_3()
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(header)
            .child(list)
            .into_any_element()
    }

    /// The 权限 section's right detail pane for the active database.
    fn render_create_database_detail(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let Some(index) = dialog.active_database else {
            return tree_message(
                t!("user.create.select_database").to_string(),
                8.0,
                theme.text_muted,
            )
            .into_any_element();
        };
        let Some(row) = dialog.db_grants.get(index) else {
            return div().into_any_element();
        };
        let name = row.name.clone();
        let scope = row.scope;
        let tables = row.tables.clone();
        let active_table = row.active_table.clone();
        let table_privileges = row.table_privileges.clone();
        let privileges = row.active_privileges();

        // The scope selector (全部表 / 指定具体表).
        let mut scopes = div().flex().flex_row().items_center().gap_1().flex_none();
        for (option, label_key, active) in [
            (
                GrantScope::AllTables,
                "user.create.scope.all",
                scope == GrantScope::AllTables,
            ),
            (
                GrantScope::SpecificTables,
                "user.create.scope.specific",
                scope == GrantScope::SpecificTables,
            ),
        ] {
            scopes = scopes.child(
                div()
                    .id(SharedString::from(format!("user-create-scope-{label_key}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .px_3()
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.primary))
                            .text_color(rgb(if theme.is_dark() {
                                theme.window_bg
                            } else {
                                0xffffff
                            }))
                    })
                    .when(!active, move |style| {
                        style
                            .bg(rgb(theme.button_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text_muted))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_create_grant_scope(option, cx)
                    }))
                    .child(t!(label_key).to_string()),
            );
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(tree_icon("icons/database.svg", theme.icon_database_active))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("user.create.database_level").to_string()),
                            ),
                    ),
            )
            .child(scopes);
        let target = div()
            .text_size(px(11.0))
            .text_color(rgb(theme.text_muted))
            .child(format!("{} {}.*", t!("user.create.grant_applies_to"), name));

        // The quick presets, matched against the active target's own set.
        let mut presets = div().flex().flex_row().items_center().gap_2().w_full();
        for template in DbTemplate::ALL {
            let active = if template == DbTemplate::None {
                privileges.is_empty()
            } else {
                !privileges.is_empty() && privileges == template.privileges()
            };
            presets = presets.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-db-template-{}",
                        template.label_key()
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.0))
                    .px_3()
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.primary))
                            .text_color(rgb(if theme.is_dark() {
                                theme.window_bg
                            } else {
                                0xffffff
                            }))
                    })
                    .when(!active, move |style| {
                        style
                            .bg(rgb(theme.button_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text_muted))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.apply_create_database_template(template, cx)
                    }))
                    .child(t!(template.label_key()).to_string()),
            );
        }

        // The grouped fine-grained privileges.
        let mut groups = div().flex().flex_col().gap_3().w_full();
        for (group_key, group_privileges) in db_privilege_groups() {
            let mut grid = div().flex().flex_row().flex_wrap().w_full();
            for (position, privilege) in group_privileges.into_iter().enumerate() {
                let checked = privileges.contains(&privilege);
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!(
                            "user-create-db-priv-{group_key}-{position}"
                        )))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w(px(210.0))
                        .h(px(CREATE_ROW_HEIGHT))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_create_database_privilege(privilege, cx)
                        }))
                        .child(checkbox_box(checked, theme))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(theme.text))
                                .child(t!(privilege.label_key()).to_string()),
                        ),
                );
            }
            groups = groups.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w_full()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!(group_key).to_string()),
                    )
                    .child(grid),
            );
        }
        let fine_grained = div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(
                div().flex().flex_row().justify_end().w_full().child(
                    div()
                        .id("user-create-db-priv-toggle-all")
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.primary))
                        .hover(move |style| style.text_color(rgb(theme.text)))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.toggle_create_database_privileges_all(cx)
                        }))
                        .child(t!("user.create.toggle_all").to_string()),
                ),
            )
            .child(groups);

        let has_target = scope == GrantScope::AllTables || active_table.is_some();
        let fine_grained_hint = if scope == GrantScope::SpecificTables {
            active_table
                .as_ref()
                .map(|table| t!("user.create.fine_grained_for", table = table.clone()).to_string())
        } else {
            None
        };
        let fine_grained_body: AnyElement = if has_target {
            fine_grained.into_any_element()
        } else {
            tree_message(
                t!("user.create.select_table_first").to_string(),
                8.0,
                theme.text_muted,
            )
            .into_any_element()
        };

        let mut column = div()
            .id("user-create-db-detail")
            .flex()
            .flex_col()
            .gap_4()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .child(header)
            .child(target)
            .child(section(
                t!("user.create.db_template").to_string(),
                Some(t!("user.create.db_template_hint").to_string()),
                presets.into_any_element(),
                theme,
            ));
        if scope == GrantScope::SpecificTables {
            column = column.child(section(
                t!("user.create.specific_tables").to_string(),
                Some(t!("user.create.tables_selected", count = tables.len()).to_string()),
                self.render_create_table_picker(
                    &name,
                    &tables,
                    active_table.as_deref(),
                    &table_privileges,
                    cx,
                ),
                theme,
            ));
        }
        column = column
            .child(section(
                t!("user.create.fine_grained").to_string(),
                fine_grained_hint,
                fine_grained_body,
                theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.create.batch_hint").to_string())
                    .child(
                        t!("user.create.selected_privileges", count = privileges.len()).to_string(),
                    ),
            );
        column.into_any_element()
    }

    /// The 指定具体表 picker of the active database. Clicking a table's name makes it active; its
    /// check box picks it into the scope, and each selected table shows its own privilege summary.
    fn render_create_table_picker(
        &self,
        database: &str,
        selected: &[String],
        active: Option<&str>,
        privileges: &BTreeMap<String, BTreeSet<Privilege>>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let loading = dialog.loading_tables.contains(database);
        let tables = dialog.tables.get(database).cloned().unwrap_or_default();
        let mut list = div()
            .id("user-create-table-list")
            .flex()
            .flex_col()
            .max_h(px(160.0))
            .overflow_y_scroll()
            .track_scroll(&dialog.table_scroll)
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg));
        if loading {
            list = list.child(tree_message(
                t!("common.loading").to_string(),
                8.0,
                theme.text_muted,
            ));
        }
        for (index, table) in tables.iter().enumerate() {
            let checked = selected.iter().any(|picked| picked == table);
            let is_active = active == Some(table.as_str());
            let empty = BTreeSet::new();
            let (badge, authorized) = privilege_summary(privileges.get(table).unwrap_or(&empty));
            let activate_name = table.clone();
            let toggle_name = table.clone();
            let mut entry = div()
                .id(SharedString::from(format!("user-create-table-{index}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(CREATE_ROW_HEIGHT))
                .px_2()
                .flex_none()
                .rounded(px(4.0))
                .cursor_pointer()
                .when(is_active, move |style| {
                    style.bg(rgb(theme.tree_selected_bg))
                })
                .when(!is_active, move |style| {
                    style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                })
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.activate_create_table(activate_name.clone(), cx)
                }))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "user-create-table-check-{index}"
                        )))
                        .flex()
                        .items_center()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_create_table(toggle_name.clone(), cx);
                        }))
                        .child(checkbox_box(checked, theme)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .text_color(rgb(if is_active {
                            theme.tree_selected_text
                        } else {
                            theme.text
                        }))
                        .child(table.clone()),
                );
            if authorized {
                entry = entry.child(
                    div()
                        .flex_none()
                        .px_2()
                        .py_0p5()
                        .rounded(px(9.0))
                        .text_size(px(10.5))
                        .bg(rgb(theme.tree_hover_bg))
                        .text_color(rgb(theme.primary))
                        .child(badge),
                );
            } else {
                entry = entry.child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(rgb(theme.text_muted))
                        .child(badge),
                );
            }
            list = list.child(entry);
        }
        let actions =
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("user-create-table-all")
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.primary))
                        .hover(move |style| style.text_color(rgb(theme.text)))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.set_create_tables_all(true, cx)
                        }))
                        .child(t!("user.create.select_all").to_string()),
                )
                .child(
                    div()
                        .id("user-create-table-none")
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.primary))
                        .hover(move |style| style.text_color(rgb(theme.text)))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.set_create_tables_all(false, cx)
                        }))
                        .child(t!("user.create.clear_all").to_string()),
                );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(list)
            .child(actions)
            .into_any_element()
    }

    /// 角色: the roles this account belongs to, and the members of this account.
    fn render_create_roles(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let candidates = dialog.membership_candidates();

        let mut column = div().flex().flex_col().gap_5().w_full();
        column = column.child(self.render_create_role_list(true, candidates, cx));
        column = column.child(self.render_create_role_list(false, candidates, cx));
        column.into_any_element()
    }

    /// One role/member table of the 角色 section.
    fn render_create_role_list(
        &self,
        member_of: bool,
        candidates: &[(String, String)],
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let Some(context) = dialog.context.as_ref() else {
            return div().into_any_element();
        };
        let current = context.user.clone();
        let current_host = context.host.clone();
        let edges = if member_of {
            &context.roles
        } else {
            &context.members
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.0))
            .px_2()
            .flex_none()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(11.0))
            .child(div().flex_1().child(t!("user.member.username").to_string()))
            .child(
                div()
                    .w(px(70.0))
                    .child(t!("user.member.granted").to_string()),
            )
            .child(div().w(px(70.0)).child(t!("user.member.admin").to_string()));

        let mut rows = div().flex().flex_col();
        let mut shown = 0usize;
        for (user, host) in candidates {
            // An account cannot be granted to itself (MySQL rejects `GRANT x TO x`), so it is never
            // a candidate member in either direction.
            if *user == current && *host == current_host {
                continue;
            }
            let key = (user.clone(), host.clone());
            let member = edges.contains_key(&key);
            let admin = edges.get(&key).copied().unwrap_or(false);
            let label = format!("{user}@{host}");
            let grant_key = key.clone();
            let admin_key = key.clone();
            let row = shown;
            shown += 1;
            rows = rows.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-role-{}-{row}",
                        if member_of { "of" } else { "member" }
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(CREATE_ROW_HEIGHT))
                    .px_2()
                    .when(row % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-role-name-{}-{row}",
                                if member_of { "of" } else { "member" }
                            )))
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_membership(member_of, grant_key.clone(), cx)
                            }))
                            .child(tree_icon("icons/user.svg", theme.icon_users))
                            .child(div().text_size(px(12.0)).child(label)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-role-granted-{}-{row}",
                                if member_of { "of" } else { "member" }
                            )))
                            .w(px(70.0))
                            .flex()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_membership(member_of, key.clone(), cx)
                            }))
                            .child(checkbox_box(member, theme)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-role-admin-{}-{row}",
                                if member_of { "of" } else { "member" }
                            )))
                            .w(px(70.0))
                            .flex()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_role_admin(member_of, admin_key.clone(), cx)
                            }))
                            .child(checkbox_box(admin, theme)),
                    ),
            );
        }

        let body: AnyElement = if shown == 0 {
            tree_message(t!("user.member.empty").to_string(), 8.0, theme.text_muted)
                .into_any_element()
        } else {
            div()
                .id(if member_of {
                    "user-create-role-of-scroll"
                } else {
                    "user-create-members-scroll"
                })
                .flex()
                .flex_col()
                .h(px(150.0))
                .overflow_y_scroll()
                .child(rows)
                .into_any_element()
        };

        section(
            t!(if member_of {
                "user.tab.member_of"
            } else {
                "user.tab.members"
            })
            .to_string(),
            None,
            div()
                .flex()
                .flex_col()
                .w_full()
                .border_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.input_bg))
                .overflow_hidden()
                .child(header)
                .child(body)
                .into_any_element(),
            theme,
        )
        .into_any_element()
    }

    /// SQL 预览: the script Save would run.
    fn render_create_sql(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let sql = self.create_user_sql();
        let text = if sql.trim().is_empty() {
            t!("design.no_changes").to_string()
        } else {
            sql
        };
        let scroll = self
            .create_user_dialog
            .as_ref()
            .map(|dialog| dialog.sql_scroll.clone())
            .unwrap_or_default();
        div()
            .id("user-create-sql")
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("user.create.preview_sql").to_string()),
                    )
                    .child(
                        div()
                            .id("user-create-sql-copy")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.primary))
                            .hover(move |style| style.text_color(rgb(theme.text)))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.copy_create_user_sql(cx)
                            }))
                            .child(t!("user.create.copy_sql").to_string()),
                    ),
            )
            .child(
                div()
                    .id("user-create-sql-scroll")
                    .h(px(CREATE_PREVIEW_HEIGHT))
                    .flex_none()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.input_bg))
                    .p_2()
                    .child(
                        div()
                            .font_family("Consolas")
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text))
                            .child(text),
                    ),
            )
            .into_any_element()
    }

    /// The window's footer: the error line, the account badge and 取消 / 保存.
    pub(super) fn create_user_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let busy = dialog.saving || dialog.loading;
        // Nothing to submit when the editor still matches the loaded account; keep Save disabled so
        // an untouched edit never opens the change preview.
        let disabled = busy || !self.create_user_has_changes();
        let is_edit = dialog.is_edit();
        let subject = match dialog.original_account() {
            Some((user, host)) => format!("{user}@{host}"),
            None => dialog.editor.account.user.trim().to_string(),
        };
        let save_label = if dialog.saving {
            t!("user.create.saving").to_string()
        } else if is_edit {
            t!("design.save").to_string()
        } else {
            t!("user.create.submit").to_string()
        };
        let mut right = div().flex().flex_row().items_center().gap_2().flex_none();
        if !subject.is_empty() {
            right = right.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.create.account_badge", name = subject).to_string()),
            );
        }
        right = right
            .child(self.dialog_button(
                "user-create-cancel",
                t!("form.cancel").to_string(),
                false,
                cx.listener(|this, _event, window, cx| this.create_user_close(window, cx)),
            ))
            .child(self.win_button(
                "user-create-submit",
                save_label,
                if disabled {
                    ButtonKind::Disabled
                } else {
                    ButtonKind::Default
                },
                cx.listener(move |this, _event, _window, cx| {
                    if !disabled {
                        this.submit_create_user(cx);
                    }
                }),
            ));
        // The footer's left cell reports state: the failure reason, a brief 已保存 confirmation, or
        // a summary of what the account will hold (the prototype's "就绪 · 已配置 …").
        let granted = dialog.db_grants.iter().filter(|row| row.enabled).count();
        let role_count = dialog
            .context
            .as_ref()
            .map(|context| context.roles.len() + context.members.len())
            .unwrap_or(0);
        let status: AnyElement = if let Some(error) = dialog.error.as_ref() {
            div()
                .text_color(rgb(theme.danger))
                .child(error.clone())
                .into_any_element()
        } else if dialog.saved {
            div()
                .text_color(rgb(theme.primary))
                .child(t!("user.create.saved").to_string())
                .into_any_element()
        } else if granted > 0 || role_count > 0 {
            div()
                .text_color(rgb(theme.text_muted))
                .child(
                    t!(
                        "user.create.summary",
                        databases = granted,
                        roles = role_count
                    )
                    .to_string(),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .w_full()
            .h(px(46.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .child(status),
            )
            .child(right)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_progressive() {
        assert!(ServerTemplate::None.privileges().is_empty());
        assert!(
            ServerTemplate::ReadWrite
                .privileges()
                .is_superset(&ServerTemplate::ReadOnly.privileges())
        );
        assert!(
            ServerTemplate::Developer
                .privileges()
                .is_superset(&ServerTemplate::ReadWrite.privileges())
        );
        assert_eq!(
            ServerTemplate::Admin.privileges().len(),
            Privilege::ALL.len()
        );
    }

    #[test]
    fn server_groups_cover_every_privilege_exactly_once() {
        let mut grouped: Vec<Privilege> = server_privilege_groups()
            .into_iter()
            .flat_map(|(_, privileges)| privileges)
            .collect();
        grouped.sort();
        let mut all = Privilege::ALL.to_vec();
        all.sort();
        assert_eq!(grouped, all);
    }

    #[test]
    fn an_edit_dialog_remembers_its_identity() {
        let dialog = UserCreateDialog::new(
            0,
            "caching_sha2_password".to_string(),
            Some(("root".to_string(), "localhost".to_string())),
        );
        assert!(dialog.is_edit());
        assert_eq!(
            dialog.original_account(),
            Some(&("root".to_string(), "localhost".to_string()))
        );
    }

    #[test]
    fn a_new_dialog_is_not_an_edit() {
        let dialog = UserCreateDialog::new(0, "caching_sha2_password".to_string(), None);
        assert!(!dialog.is_edit());
        assert_eq!(dialog.original_account(), None);
    }

    #[test]
    fn resource_limit_fields_parse_empty_as_unlimited() {
        assert_eq!(parse_limit(""), 0);
        assert_eq!(parse_limit("  "), 0);
        assert_eq!(parse_limit("abc"), 0);
        assert_eq!(parse_limit("250"), 250);
    }

    fn grant(database: &str, name: &str) -> ObjectGrant {
        grant_with(database, name, &[Privilege::Select])
    }

    fn grant_with(database: &str, name: &str, privileges: &[Privilege]) -> ObjectGrant {
        ObjectGrant {
            database: database.to_string(),
            name: name.to_string(),
            privileges: privileges.iter().copied().collect(),
        }
    }

    #[test]
    fn rebuild_maps_whole_database_table_and_ungranted_rows() {
        let mut dialog = UserCreateDialog::new(0, "caching_sha2_password".to_string(), None);
        dialog.databases = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        dialog.editor.grants = vec![
            grant("a", ""),
            grant("b", "t1"),
            grant_with(
                "b",
                "t2",
                &[
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                ],
            ),
        ];
        dialog.rebuild_db_grants();

        assert_eq!(dialog.db_grants.len(), 3);
        assert!(dialog.db_grants[0].enabled);
        assert_eq!(dialog.db_grants[0].scope, GrantScope::AllTables);
        assert!(dialog.db_grants[1].enabled);
        assert_eq!(dialog.db_grants[1].scope, GrantScope::SpecificTables);
        assert_eq!(
            dialog.db_grants[1].tables,
            vec!["t1".to_string(), "t2".to_string()]
        );
        assert_eq!(dialog.db_grants[1].active_table.as_deref(), Some("t1"));
        assert!(!dialog.db_grants[2].enabled);
        assert_eq!(dialog.object_grants().len(), 3);
    }

    #[test]
    fn per_table_privileges_are_kept_apart() {
        let mut dialog = UserCreateDialog::new(0, "caching_sha2_password".to_string(), None);
        dialog.databases = vec!["shop".to_string()];
        dialog.editor.grants = vec![
            grant("shop", "account"),
            grant_with(
                "shop",
                "orders",
                &[
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                ],
            ),
        ];
        dialog.rebuild_db_grants();

        let row = &dialog.db_grants[0];
        assert_eq!(
            row.table_privileges.get("account"),
            Some(&[Privilege::Select].into_iter().collect())
        );
        assert_eq!(
            row.table_privileges.get("orders"),
            Some(
                &[
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                ]
                .into_iter()
                .collect()
            )
        );
        // A mixed set is no longer flattened into one preset shared by both tables.
        assert_eq!(row.summary().0, format!("{} (4)", t!("user.create.custom")));

        let grants = dialog.object_grants();
        let account = grants
            .iter()
            .find(|grant| grant.name == "account")
            .expect("the account grant");
        let orders = grants
            .iter()
            .find(|grant| grant.name == "orders")
            .expect("the orders grant");
        assert_eq!(
            account.privileges,
            [Privilege::Select].into_iter().collect()
        );
        assert_eq!(orders.privileges.len(), 4);
    }

    #[test]
    fn editing_one_table_does_not_touch_another() {
        let mut dialog = UserCreateDialog::new(0, "caching_sha2_password".to_string(), None);
        dialog.databases = vec!["shop".to_string()];
        dialog.rebuild_db_grants();
        dialog.db_grants[0].activate_table("account");
        dialog.db_grants[0].activate_table("orders");

        dialog.db_grants[0]
            .active_privileges_mut()
            .expect("an active table")
            .insert(Privilege::Select);
        dialog.db_grants[0].active_table = Some("account".to_string());
        dialog.db_grants[0]
            .active_privileges_mut()
            .expect("an active table")
            .insert(Privilege::Insert);

        let row = &dialog.db_grants[0];
        assert_eq!(
            row.table_privileges.get("orders"),
            Some(&[Privilege::Select].into_iter().collect())
        );
        assert_eq!(
            row.table_privileges.get("account"),
            Some(&[Privilege::Insert].into_iter().collect())
        );
    }

    #[test]
    fn specific_scope_without_tables_falls_back_to_a_database_grant() {
        let mut dialog = UserCreateDialog::new(0, "caching_sha2_password".to_string(), None);
        dialog.databases = vec!["a".to_string()];
        dialog.rebuild_db_grants();
        dialog.db_grants[0].enabled = true;
        dialog.db_grants[0].scope = GrantScope::SpecificTables;
        dialog.db_grants[0].privileges.insert(Privilege::Select);

        let grants = dialog.object_grants();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].database, "a");
        assert!(grants[0].name.is_empty());
    }

    #[test]
    fn rebuild_keeps_granted_databases_missing_from_the_listing() {
        let mut dialog = UserCreateDialog::new(0, "caching_sha2_password".to_string(), None);
        dialog.databases = vec!["a".to_string()];
        dialog.editor.grants = vec![grant("only_in_grants", "")];
        dialog.rebuild_db_grants();

        assert_eq!(dialog.db_grants.len(), 2);
        assert!(
            dialog
                .db_grants
                .iter()
                .any(|row| row.name == "only_in_grants" && row.enabled)
        );
        let grants = dialog.object_grants();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].database, "only_in_grants");
    }

    #[test]
    fn loading_details_populates_the_role_lists() {
        use rustgrid_core::RoleMembership;

        let mut dialog = UserCreateDialog::new(
            0,
            "caching_sha2_password".to_string(),
            Some(("test".to_string(), "%".to_string())),
        );
        dialog.apply_details(UserDetails {
            account: UserAccount {
                user: "test".to_string(),
                host: "%".to_string(),
                ..Default::default()
            },
            roles: vec![RoleMembership {
                role_user: "root".to_string(),
                role_host: "%".to_string(),
                member_user: "test".to_string(),
                member_host: "%".to_string(),
                admin_option: true,
            }],
            members: vec![RoleMembership {
                role_user: "test".to_string(),
                role_host: "%".to_string(),
                member_user: "alice".to_string(),
                member_host: "localhost".to_string(),
                admin_option: false,
            }],
            ..Default::default()
        });

        let context = dialog.context.as_ref().expect("a role context");
        assert_eq!(
            context.roles.get(&("root".to_string(), "%".to_string())),
            Some(&true),
            "{:?}",
            context.roles
        );
        assert_eq!(
            context
                .members
                .get(&("alice".to_string(), "localhost".to_string())),
            Some(&false),
            "{:?}",
            context.members
        );
    }
}

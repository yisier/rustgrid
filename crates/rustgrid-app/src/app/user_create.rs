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
/// Default width of the 权限 section's database list (the drag-resizable left pane).
const CREATE_DB_LIST_DEFAULT_WIDTH: f32 = 270.0;
/// Narrowest the 权限 database list can be dragged.
const CREATE_DB_LIST_MIN_WIDTH: f32 = 180.0;
/// Widest the 权限 database list can be dragged.
const CREATE_DB_LIST_MAX_WIDTH: f32 = 480.0;
/// Width of the 默认权限 section's schema list.
const CREATE_DEFAULT_SCHEMA_WIDTH: f32 = 190.0;
/// Max height of the 默认权限 section's grantee list before it scrolls.
const CREATE_DEFAULT_LIST_MAX_HEIGHT: f32 = 180.0;
/// Width of the server-securables matrix's name column.
const SECURABLE_NAME_WIDTH: f32 = 180.0;
/// Width of one server-securables permission column.
const SECURABLE_COL_WIDTH: f32 = 104.0;
/// Width of one checkbox column of the server-privilege table (授予 / 含授予选项 / 拒绝).
const SERVER_PRIV_CHECK_WIDTH: f32 = 96.0;

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

/// The privileges of a preset (empty when `index` is out of range). Used by the editor's one-click
/// preset buttons.
pub(super) fn preset_privileges(
    presets: &[PrivilegePreset],
    index: usize,
) -> BTreeSet<PrivilegeId> {
    presets
        .get(index)
        .map(|preset| preset.privileges.iter().cloned().collect())
        .unwrap_or_default()
}

/// Resolve a catalog label: its i18n key when the app knows one, else the literal fallback.
pub(super) fn catalog_label(label_key: &Option<String>, label: &str) -> String {
    match label_key {
        Some(key) => t!(key.as_str()).to_string(),
        None => label.to_string(),
    }
}

/// The display label of one privilege from the catalog (its raw id when the catalog omits it).
pub(super) fn privilege_label(catalog: &PrivilegeCatalog, id: &PrivilegeId) -> String {
    catalog
        .info(id)
        .map(|info| catalog_label(&info.label_key, &info.label))
        .unwrap_or_else(|| id.as_str().to_string())
}

/// The hover tooltip of a privilege check box. The box itself shows the engine's raw keyword (e.g.
/// `CONTROL SERVER`); this shows the localized name, so a translated privilege is never ambiguous.
pub(super) struct PrivilegeTooltip(pub(super) String);

impl Render for PrivilegeTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = gpui_kit::component::Theme::global(cx);
        div()
            .max_w(px(320.0))
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.0))
            .text_color(theme.popover_foreground)
            .child(self.0.clone())
    }
}

/// An account's display label: `user@host` for an engine that identifies accounts that way, else
/// just the user name (SQL Server, PostgreSQL, Oracle have no host part).
fn user_host_label(user: &str, host: &str, spec: &UserEditorSpec) -> String {
    if spec.host && !host.is_empty() {
        format!("{user}@{host}")
    } else {
        user.to_string()
    }
}

/// The i18n key naming one default-privileges object kind.
fn default_object_type_key(object_type: DefaultObjectType) -> &'static str {
    match object_type {
        DefaultObjectType::Tables => "user.default.object.tables",
        DefaultObjectType::Sequences => "user.default.object.sequences",
        DefaultObjectType::Functions => "user.default.object.functions",
        DefaultObjectType::Types => "user.default.object.types",
        DefaultObjectType::Schemas => "user.default.object.schemas",
    }
}

/// The window's left-navigation sections.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UserSection {
    General,
    Advanced,
    ServerPrivileges,
    ObjectPrivileges,
    DefaultPrivileges,
    Roles,
    UserMapping,
    EndpointPermissions,
    LoginPermissions,
    Sql,
}

impl UserSection {
    const ALL: [UserSection; 10] = [
        UserSection::General,
        UserSection::Advanced,
        UserSection::ServerPrivileges,
        UserSection::ObjectPrivileges,
        UserSection::DefaultPrivileges,
        UserSection::Roles,
        UserSection::UserMapping,
        UserSection::EndpointPermissions,
        UserSection::LoginPermissions,
        UserSection::Sql,
    ];

    fn label_key(self) -> &'static str {
        match self {
            UserSection::General => "user.tab.general",
            UserSection::Advanced => "user.tab.advanced",
            UserSection::ServerPrivileges => "user.tab.server_privileges",
            UserSection::ObjectPrivileges => "user.tab.privileges",
            UserSection::DefaultPrivileges => "user.tab.default_privileges",
            UserSection::Roles => "user.tab.roles",
            UserSection::UserMapping => "user.tab.user_mapping",
            UserSection::EndpointPermissions => "user.tab.endpoint_permissions",
            UserSection::LoginPermissions => "user.tab.login_permissions",
            UserSection::Sql => "user.tab.sql",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            UserSection::General => "icons/user.svg",
            UserSection::Advanced => "icons/gear.svg",
            UserSection::ServerPrivileges => "icons/gear.svg",
            UserSection::ObjectPrivileges => "icons/database.svg",
            UserSection::DefaultPrivileges => "icons/tables.svg",
            UserSection::Roles => "icons/user.svg",
            UserSection::UserMapping => "icons/database.svg",
            UserSection::EndpointPermissions => "icons/gear.svg",
            UserSection::LoginPermissions => "icons/user.svg",
            UserSection::Sql => "icons/queries.svg",
        }
    }

    fn id(self) -> &'static str {
        match self {
            UserSection::General => "user-create-nav-general",
            UserSection::Advanced => "user-create-nav-advanced",
            UserSection::ServerPrivileges => "user-create-nav-server",
            UserSection::ObjectPrivileges => "user-create-nav-object",
            UserSection::DefaultPrivileges => "user-create-nav-default",
            UserSection::Roles => "user-create-nav-roles",
            UserSection::UserMapping => "user-create-nav-mapping",
            UserSection::EndpointPermissions => "user-create-nav-endpoint",
            UserSection::LoginPermissions => "user-create-nav-login",
            UserSection::Sql => "user-create-nav-sql",
        }
    }

    /// Whether the section is shown for an engine's editor spec. 常规 and SQL 预览 are always
    /// present; the privilege and role sections follow the engine's capabilities.
    fn visible(self, spec: &UserEditorSpec) -> bool {
        match self {
            UserSection::General | UserSection::Sql => true,
            UserSection::Advanced => spec.ssl,
            UserSection::ServerPrivileges => spec.server_privileges,
            UserSection::ObjectPrivileges => spec.object_privileges,
            UserSection::DefaultPrivileges => spec.default_privileges,
            UserSection::Roles => spec.roles,
            UserSection::UserMapping => spec.user_mapping,
            UserSection::EndpointPermissions => spec.endpoint_permissions,
            UserSection::LoginPermissions => spec.login_permissions,
        }
    }

    /// The `SecurableClass::id` this section edits, for the two server-securable sections.
    fn securable_class_id(self) -> Option<&'static str> {
        match self {
            UserSection::EndpointPermissions => Some("endpoint"),
            UserSection::LoginPermissions => Some("login"),
            _ => None,
        }
    }
}

/// Whether a database's grant covers the whole database or only picked tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GrantScope {
    AllTables,
    SpecificTables,
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
    pub(super) privileges: BTreeSet<PrivilegeId>,
    /// The privileges granted per table (the 指定具体表 scope), keyed by table name.
    pub(super) table_privileges: BTreeMap<String, BTreeSet<PrivilegeId>>,
}

/// The badge for one privilege set: the matching preset's name (with its count) or 未授权.
pub(super) fn privilege_summary(
    privileges: &BTreeSet<PrivilegeId>,
    presets: &[PrivilegePreset],
) -> (String, bool) {
    if privileges.is_empty() {
        return (t!("user.create.unauthorized").to_string(), false);
    }
    let preset = presets.iter().find(|preset| {
        let set: BTreeSet<PrivilegeId> = preset.privileges.iter().cloned().collect();
        !set.is_empty() && set == *privileges
    });
    let label = match preset {
        Some(preset) => catalog_label(&preset.label_key, &preset.label),
        None => t!("user.create.custom").to_string(),
    };
    (format!("{} ({})", label, privileges.len()), true)
}

impl DbGrant {
    /// The privilege set the detail pane edits: the whole database for 全部表, otherwise the active
    /// table's own set (empty when no table is active).
    fn active_privileges(&self) -> BTreeSet<PrivilegeId> {
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
    fn active_privileges_mut(&mut self) -> Option<&mut BTreeSet<PrivilegeId>> {
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
    fn effective_sets(&self) -> Vec<BTreeSet<PrivilegeId>> {
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
    fn summary(&self, presets: &[PrivilegePreset]) -> (String, bool) {
        if !self.enabled {
            return (t!("user.create.unauthorized").to_string(), false);
        }
        let sets = self.effective_sets();
        let union: BTreeSet<PrivilegeId> =
            sets.iter().flat_map(|set| set.iter().cloned()).collect();
        if union.is_empty() {
            return (t!("user.create.unauthorized").to_string(), false);
        }
        if sets.iter().all(|set| *set == sets[0]) {
            privilege_summary(&sets[0], presets)
        } else {
            (
                format!("{} ({})", t!("user.create.custom"), union.len()),
                true,
            )
        }
    }
}

/// One database row of the 用户映射 section (SQL Server): whether the login is mapped into the
/// database, its database user and default schema, and the database roles picked for it.
pub(super) struct UserMappingRow {
    pub(super) database: String,
    pub(super) mapped: bool,
    pub(super) user_name: String,
    pub(super) default_schema: String,
    pub(super) roles: BTreeSet<String>,
    /// Every database role the database offers (fixed and user-defined).
    pub(super) available_roles: Vec<String>,
}

impl UserMappingRow {
    fn from_mapping(mapping: UserMapping) -> Self {
        Self {
            database: mapping.database,
            mapped: mapping.mapped,
            user_name: mapping.user_name,
            default_schema: if mapping.default_schema.trim().is_empty() {
                "dbo".to_string()
            } else {
                mapping.default_schema
            },
            roles: mapping.roles,
            available_roles: mapping.available_roles,
        }
    }

    /// The `UserEdit` projection of this row.
    fn to_mapping(&self) -> UserMapping {
        UserMapping {
            database: self.database.clone(),
            mapped: self.mapped,
            user_name: self.user_name.clone(),
            default_schema: self.default_schema.clone(),
            roles: self.roles.clone(),
            available_roles: Vec::new(),
        }
    }
}

/// One server-level securable row of the 终端节点权限 / 登录权限 sections (SQL Server).
pub(super) struct SecurableRow {
    pub(super) class: String,
    pub(super) name: String,
    pub(super) privileges: BTreeSet<PrivilegeId>,
    pub(super) denied: BTreeSet<PrivilegeId>,
}

impl SecurableRow {
    fn from_grant(grant: ServerSecurableGrant) -> Self {
        Self {
            class: grant.class,
            name: grant.name,
            privileges: grant.privileges,
            denied: grant.denied,
        }
    }

    fn to_grant(&self) -> ServerSecurableGrant {
        ServerSecurableGrant {
            class: self.class.clone(),
            name: self.name.clone(),
            privileges: self.privileges.clone(),
            denied: self.denied.clone(),
        }
    }
}

/// State of the user-account window.
pub(super) struct UserCreateDialog {
    /// The connection the account lives on.
    pub(super) connection_index: usize,
    /// Which fields, sections and privileges this engine's editor shows.
    pub(super) spec: UserEditorSpec,
    /// The engine's privilege catalog (groups, privileges and presets).
    pub(super) catalog: PrivilegeCatalog,
    /// Whether the engine qualifies object names with a schema (`schema.name`), so a picked table
    /// name is split into its parts.
    pub(super) schemas: bool,
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
    /// The drag-resizable width of the 权限 database list.
    pub(super) db_list_width: f32,
    /// `(pointer x, width)` captured when the list's divider drag started.
    pub(super) db_list_drag: Option<(f32, f32)>,
    pub(super) table_scroll: ScrollHandle,
    /// Lazily-loaded tables per database (the 指定具体表 picker).
    pub(super) tables: BTreeMap<String, Vec<String>>,
    pub(super) loading_tables: BTreeSet<String>,
    /// The account's default-privileges rules (PostgreSQL's `ALTER DEFAULT PRIVILEGES`).
    pub(super) default_rules: Vec<DefaultPrivilege>,
    /// The schemas the 默认权限 section lists; the first row is the implicit "all schemas".
    pub(super) default_schemas: Vec<String>,
    /// The schema selected in the 默认权限 section; empty means every schema.
    pub(super) default_schema: String,
    /// The object kind selected in the 默认权限 section.
    pub(super) default_object_type: DefaultObjectType,
    /// The grantee whose privileges the 默认权限 detail grid edits.
    pub(super) default_active: Option<String>,
    pub(super) default_schema_scroll: ScrollHandle,
    pub(super) default_account_scroll: ScrollHandle,
    /// SQL Server's 用户映射: one row per database.
    pub(super) mappings: Vec<UserMappingRow>,
    /// The database shown in the 用户映射 detail pane (index into `mappings`).
    pub(super) active_mapping: Option<usize>,
    pub(super) mapping_scroll: ScrollHandle,
    /// The mappings as loaded, for the save diff.
    pub(super) mapping_original: Vec<UserMapping>,
    /// SQL Server's 终端节点权限 / 登录权限 rows (all endpoints and logins).
    pub(super) securables: Vec<SecurableRow>,
    /// The securable grants as loaded, for the save diff.
    pub(super) securables_original: Vec<ServerSecurableGrant>,
    /// Whether the 指定旧密码 field is active while editing.
    pub(super) use_old_password: bool,
    /// The 指定旧密码 value (never persisted).
    pub(super) old_password: String,
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
        spec: UserEditorSpec,
        catalog: PrivilegeCatalog,
        schemas: bool,
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
            spec,
            catalog,
            schemas,
            section: UserSection::General,
            editor,
            expiry: CreateExpiry::Default,
            expiry_days: 30,
            databases: Vec::new(),
            db_grants: Vec::new(),
            active_database: None,
            database_search: String::new(),
            db_scroll: ScrollHandle::new(),
            db_list_width: CREATE_DB_LIST_DEFAULT_WIDTH,
            db_list_drag: None,
            table_scroll: ScrollHandle::new(),
            tables: BTreeMap::new(),
            loading_tables: BTreeSet::new(),
            default_rules: Vec::new(),
            default_schemas: Vec::new(),
            default_schema: String::new(),
            default_object_type: DefaultObjectType::Tables,
            default_active: None,
            default_schema_scroll: ScrollHandle::new(),
            default_account_scroll: ScrollHandle::new(),
            mappings: Vec::new(),
            active_mapping: None,
            mapping_scroll: ScrollHandle::new(),
            mapping_original: Vec::new(),
            securables: Vec::new(),
            securables_original: Vec::new(),
            use_old_password: false,
            old_password: String::new(),
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
        self.default_rules = self
            .editor
            .original
            .as_ref()
            .map(|details| details.default_privileges.clone())
            .unwrap_or_default();
        // Open on the first loaded rule so an existing default privilege is visible immediately.
        self.default_schema = self
            .default_rules
            .first()
            .map(|rule| rule.schema.clone())
            .unwrap_or_default();
        self.default_object_type = self
            .default_rules
            .first()
            .map(|rule| rule.object_type)
            .unwrap_or(DefaultObjectType::Tables);
        self.default_active = self.default_rules.first().map(|rule| rule.grantee.clone());
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

    /// Replace the 用户映射 rows (used on load). The first database is selected. The loaded rows
    /// are also kept as the diff baseline, so only actual mapping changes are written.
    pub(super) fn set_mappings(&mut self, mappings: Vec<UserMapping>) {
        self.mapping_original = mappings.clone();
        self.mappings = mappings
            .into_iter()
            .map(UserMappingRow::from_mapping)
            .collect();
        self.active_mapping = (!self.mappings.is_empty()).then_some(0);
    }

    /// The mappings the edit writes.
    pub(super) fn edit_mappings(&self) -> Vec<UserMapping> {
        self.mappings
            .iter()
            .map(UserMappingRow::to_mapping)
            .collect()
    }

    /// Replace the server securable rows (used on load), keeping the loaded grants for the diff.
    pub(super) fn set_securables(&mut self, securables: Vec<ServerSecurableGrant>) {
        self.securables_original = securables.clone();
        self.securables = securables
            .into_iter()
            .map(SecurableRow::from_grant)
            .collect();
    }

    /// The server securable grants the edit writes.
    pub(super) fn edit_securables(&self) -> Vec<ServerSecurableGrant> {
        self.securables.iter().map(SecurableRow::to_grant).collect()
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
                    .map(|grant| grant.object_name())
                    .collect();
                let mut table_privileges: BTreeMap<String, BTreeSet<PrivilegeId>> = BTreeMap::new();
                for grant in &table_grants {
                    table_privileges
                        .entry(grant.object_name())
                        .or_default()
                        .extend(grant.privileges.iter().cloned());
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
                    let (schema, name) = if self.schemas {
                        ObjectGrant::split_object_name(table)
                    } else {
                        (String::new(), table.clone())
                    };
                    grants.push(ObjectGrant {
                        database: row.name.clone(),
                        schema,
                        name,
                        privileges: row.table_privileges.get(table).cloned().unwrap_or_default(),
                    });
                }
            } else {
                grants.push(ObjectGrant {
                    database: row.name.clone(),
                    schema: String::new(),
                    name: String::new(),
                    privileges: row.privileges.clone(),
                });
            }
        }
        grants
    }

    /// The default-privileges rule for one `(schema, object kind, grantee)`, if any.
    fn default_rule(
        &self,
        schema: &str,
        object_type: DefaultObjectType,
        grantee: &str,
    ) -> Option<&DefaultPrivilege> {
        self.default_rules.iter().find(|rule| {
            rule.schema == schema && rule.object_type == object_type && rule.grantee == grantee
        })
    }

    fn default_rule_mut(
        &mut self,
        schema: &str,
        object_type: DefaultObjectType,
        grantee: &str,
    ) -> Option<&mut DefaultPrivilege> {
        self.default_rules.iter_mut().find(|rule| {
            rule.schema == schema && rule.object_type == object_type && rule.grantee == grantee
        })
    }

    /// Ensure every schema named by a loaded rule is listed, so an existing default privilege is
    /// never hidden when the server's schema list omits it.
    fn merge_default_schemas(&mut self) {
        for rule in &self.default_rules {
            if !rule.schema.is_empty() && !self.default_schemas.contains(&rule.schema) {
                self.default_schemas.push(rule.schema.clone());
            }
        }
        self.default_schemas.sort();
        self.default_schemas.dedup();
    }

    /// The grantees with a rule at `(schema, object kind)`, as `grantee -> privileges`.
    fn default_grantees(
        &self,
        schema: &str,
        object_type: DefaultObjectType,
    ) -> BTreeMap<String, BTreeSet<PrivilegeId>> {
        let mut map: BTreeMap<String, BTreeSet<PrivilegeId>> = BTreeMap::new();
        for rule in &self.default_rules {
            if rule.schema == schema && rule.object_type == object_type {
                map.insert(rule.grantee.clone(), rule.privileges.clone());
            }
        }
        map
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
    PasswordValidUntil,
    DefaultTablespace,
    Profile,
    TablespaceQuota,
    MaxQuestions,
    MaxUpdates,
    MaxConnections,
    MaxUserConnections,
    DatabaseSearch,
    MappingUser,
    MappingSchema,
    OldPassword,
    DefaultLanguage,
    Certificate,
    AsymmetricKey,
    Credential,
    SslCipher,
    SslIssuer,
    SslSubject,
}

/// Which dropdown a change came from.
#[derive(Clone, Copy)]
enum CreateCombo {
    Plugin,
    Expiry,
    Verification,
    DefaultDatabase,
    Ssl,
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
    pub(super) server_privileges: BTreeSet<PrivilegeId>,
    /// Server-wide privileges explicitly denied (SQL Server).
    pub(super) denied_server_privileges: BTreeSet<PrivilegeId>,
    /// Server-wide privileges held `WITH GRANT OPTION` (SQL Server).
    pub(super) grant_option_server_privileges: BTreeSet<PrivilegeId>,
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
            denied_server_privileges: BTreeSet::new(),
            grant_option_server_privileges: BTreeSet::new(),
            grants: Vec::new(),
        }
    }

    fn apply_details(&mut self, details: UserDetails) {
        self.account = details.account.clone();
        self.server_privileges = details.server_privileges.clone();
        self.denied_server_privileges = details.denied_server_privileges.clone();
        self.grant_option_server_privileges = details.grant_option_server_privileges.clone();
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
pub(super) fn section(
    title: String,
    hint: Option<String>,
    content: AnyElement,
    theme: Theme,
) -> Div {
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

/// The tri-state marker for one server-securable permission: empty (none), a check (granted) or a
/// red `×` (denied). Clicking the cell cycles through them, so the checkbox stays centered under
/// its column header.
fn securable_state_box(checked: bool, denied: bool, theme: Theme) -> AnyElement {
    if denied {
        let foreground = if theme.is_dark() {
            theme.window_bg
        } else {
            0xffffff
        };
        div()
            .w(px(14.0))
            .h(px(14.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.0))
            .border_1()
            .border_color(rgb(theme.danger))
            .bg(rgb(theme.danger))
            .text_color(rgb(foreground))
            .text_size(px(11.0))
            .child("×".to_string())
            .into_any_element()
    } else {
        checkbox_box(checked, theme).into_any_element()
    }
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
        let spec = self.user_editor_spec(connection_index);
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
        self.create_user_password_valid_until = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::PasswordValidUntil,
            &weak,
            cx,
        ));
        self.create_user_default_tablespace = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::DefaultTablespace,
            &weak,
            cx,
        ));
        self.create_user_profile = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::Profile,
            &weak,
            cx,
        ));
        self.create_user_tablespace_quota = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::TablespaceQuota,
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
        self.create_user_mapping_user = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::MappingUser,
            &weak,
            cx,
        ));
        self.create_user_mapping_schema = Some(make_create_field_input(
            theme,
            "dbo".to_string(),
            false,
            CreateField::MappingSchema,
            &weak,
            cx,
        ));
        let login_types = connection.login_types();
        let verification_options = login_types
            .iter()
            .map(|login_type| ComboOption::plain(*login_type))
            .collect();
        self.create_user_verification_combo = Some(make_create_combo(
            theme,
            verification_options,
            login_types
                .first()
                .copied()
                .unwrap_or("SQL Server")
                .to_string(),
            CreateCombo::Verification,
            &weak,
            cx,
        ));
        self.create_user_default_database_combo = Some(make_create_combo(
            theme,
            Vec::new(),
            String::new(),
            CreateCombo::DefaultDatabase,
            &weak,
            cx,
        ));
        self.create_user_old_password = Some(make_create_field_input(
            theme,
            String::new(),
            true,
            CreateField::OldPassword,
            &weak,
            cx,
        ));
        self.create_user_default_language = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::DefaultLanguage,
            &weak,
            cx,
        ));
        self.create_user_certificate = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::Certificate,
            &weak,
            cx,
        ));
        self.create_user_asymmetric_key = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::AsymmetricKey,
            &weak,
            cx,
        ));
        self.create_user_credential = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::Credential,
            &weak,
            cx,
        ));
        let ssl_options = connection
            .ssl_types()
            .into_iter()
            .map(|ssl_type| {
                if ssl_type.is_empty() {
                    ComboOption::new("NONE", t!("user.ssl.none").to_string())
                } else {
                    ComboOption::new(ssl_type, ssl_type)
                }
            })
            .collect();
        self.create_user_ssl_combo = Some(make_create_combo(
            theme,
            ssl_options,
            "NONE".to_string(),
            CreateCombo::Ssl,
            &weak,
            cx,
        ));
        self.create_user_ssl_cipher = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::SslCipher,
            &weak,
            cx,
        ));
        self.create_user_ssl_issuer = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::SslIssuer,
            &weak,
            cx,
        ));
        self.create_user_ssl_subject = Some(make_create_field_input(
            theme,
            String::new(),
            false,
            CreateField::SslSubject,
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

        let catalog = self.user_privilege_catalog(connection_index);
        let schemas = self.driver_supports(connection_index, DriverCapability::Schemas);
        let mut dialog =
            UserCreateDialog::new(connection_index, plugin, spec, catalog, schemas, account);
        // Default a new SQL Server login to the first verification type, so its password fields
        // show before the user touches the dropdown.
        if dialog.editor.account.login_type.trim().is_empty()
            && let Some(first) = login_types.first()
        {
            dialog.editor.account.login_type = (*first).to_string();
        }
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
        let want_schemas = dialog.spec.default_privileges;
        let want_mappings = dialog.spec.user_mapping;
        let want_securables = dialog.spec.endpoint_permissions || dialog.spec.login_permissions;
        cx.spawn(async move |this, cx| {
            let joined = {
                let connection = connection.clone();
                runtime
                    .spawn(async move {
                        let details = connection.user_details(&user, &host).await;
                        let accounts = connection.list_users().await;
                        let databases = connection.list_databases().await;
                        let schemas = if want_schemas {
                            connection.default_privilege_schemas().await
                        } else {
                            Ok(Vec::new())
                        };
                        let mappings = if want_mappings {
                            connection.user_mappings(&user, &host).await
                        } else {
                            Ok(Vec::new())
                        };
                        let securables = if want_securables {
                            connection.user_securables(&user, &host).await
                        } else {
                            Ok(Vec::new())
                        };
                        (details, accounts, databases, schemas, mappings, securables)
                    })
                    .await
            };
            let (details, accounts, databases, schemas, mappings, securables) = match joined {
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
                match schemas {
                    Ok(schemas) => dialog.default_schemas = schemas,
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                if let Ok(mappings) = mappings {
                    dialog.set_mappings(mappings);
                }
                if let Ok(securables) = securables {
                    dialog.set_securables(securables);
                }
                dialog.merge_default_schemas();
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
        let want_schemas = dialog.spec.default_privileges;
        let want_mappings = dialog.spec.user_mapping;
        let want_securables = dialog.spec.endpoint_permissions || dialog.spec.login_permissions;
        let user = dialog.editor.account.user.clone();
        cx.spawn(async move |this, cx| {
            let joined = {
                let connection = connection.clone();
                runtime
                    .spawn(async move {
                        let databases = connection.list_databases().await;
                        let accounts = connection.list_users().await;
                        let schemas = if want_schemas {
                            connection.default_privilege_schemas().await
                        } else {
                            Ok(Vec::new())
                        };
                        let mappings = if want_mappings {
                            connection.user_mappings(&user, "").await
                        } else {
                            Ok(Vec::new())
                        };
                        let securables = if want_securables {
                            connection.user_securables(&user, "").await
                        } else {
                            Ok(Vec::new())
                        };
                        (databases, accounts, schemas, mappings, securables)
                    })
                    .await
            };
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                match joined {
                    Ok((databases, accounts, schemas, mappings, securables)) => {
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
                        match schemas {
                            Ok(schemas) => dialog.default_schemas = schemas,
                            Err(error) => dialog.error = Some(error.to_string()),
                        }
                        if let Ok(mappings) = mappings {
                            dialog.set_mappings(mappings);
                        }
                        if let Ok(securables) = securables {
                            dialog.set_securables(securables);
                        }
                    }
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                dialog.merge_default_schemas();
                dialog.rebuild_db_grants();
                app.sync_create_editor_fields(cx);
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
        let focus = self.create_user_focus.clone();
        let spec = self
            .create_user_dialog
            .as_ref()
            .map(|dialog| dialog.spec.clone())
            .unwrap_or_else(UserEditorSpec::mysql);
        let title = match self
            .create_user_dialog
            .as_ref()
            .and_then(|dialog| dialog.original_account())
        {
            Some((user, host)) => format!(
                "{} - {}",
                user_host_label(user, host, &spec),
                t!("user.create.edit_title")
            ),
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
                    let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                    // Give the window root the focus so ESC reaches its key handler even when no
                    // field is focused yet.
                    window.focus(&focus, cx);
                    root
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
        self.create_user_password_valid_until = None;
        self.create_user_default_tablespace = None;
        self.create_user_profile = None;
        self.create_user_tablespace_quota = None;
        self.create_user_max_questions = None;
        self.create_user_max_updates = None;
        self.create_user_max_connections = None;
        self.create_user_max_user_connections = None;
        self.create_user_database_search = None;
        self.create_user_mapping_user = None;
        self.create_user_mapping_schema = None;
        self.create_user_verification_combo = None;
        self.create_user_default_database_combo = None;
        self.create_user_old_password = None;
        self.create_user_default_language = None;
        self.create_user_certificate = None;
        self.create_user_asymmetric_key = None;
        self.create_user_credential = None;
        self.create_user_ssl_combo = None;
        self.create_user_ssl_cipher = None;
        self.create_user_ssl_issuer = None;
        self.create_user_ssl_subject = None;
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
                CreateField::PasswordValidUntil => {
                    let value = text.trim();
                    dialog.editor.account.password_valid_until = if value.is_empty() {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                CreateField::DefaultTablespace => {
                    dialog.editor.account.default_tablespace = text.trim().to_string()
                }
                CreateField::Profile => dialog.editor.account.profile = text.trim().to_string(),
                CreateField::TablespaceQuota => {
                    dialog.editor.account.tablespace_quota = text.trim().to_string()
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
                CreateField::MappingUser => {
                    let index = dialog.active_mapping;
                    if let Some(row) = index.and_then(|index| dialog.mappings.get_mut(index)) {
                        row.user_name = text.to_string();
                    }
                }
                CreateField::MappingSchema => {
                    let index = dialog.active_mapping;
                    if let Some(row) = index.and_then(|index| dialog.mappings.get_mut(index)) {
                        row.default_schema = text.trim().to_string();
                    }
                }
                CreateField::OldPassword => dialog.old_password = text.to_string(),
                CreateField::DefaultLanguage => {
                    dialog.editor.account.default_language = text.trim().to_string()
                }
                CreateField::Certificate => {
                    dialog.editor.account.certificate = text.trim().to_string()
                }
                CreateField::AsymmetricKey => {
                    dialog.editor.account.asymmetric_key = text.trim().to_string()
                }
                CreateField::Credential => {
                    dialog.editor.account.credential = text.trim().to_string()
                }
                CreateField::SslCipher => {
                    dialog.editor.account.ssl_cipher = text.trim().to_string()
                }
                CreateField::SslIssuer => {
                    dialog.editor.account.x509_issuer = text.trim().to_string()
                }
                CreateField::SslSubject => {
                    dialog.editor.account.x509_subject = text.trim().to_string()
                }
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
                CreateCombo::Verification => dialog.editor.account.login_type = value.to_string(),
                CreateCombo::DefaultDatabase => {
                    dialog.editor.account.default_database = value.to_string()
                }
                CreateCombo::Ssl => {
                    dialog.editor.account.ssl_type = if value == "NONE" {
                        String::new()
                    } else {
                        value.to_string()
                    };
                }
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

    /// Toggle the 授予 checkbox of one server privilege. Clearing it also clears the grant option;
    /// granting clears any deny of the same privilege.
    pub(super) fn toggle_create_server_privilege(
        &mut self,
        privilege: PrivilegeId,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let editor = &mut dialog.editor;
            if editor.server_privileges.contains(&privilege) {
                editor.server_privileges.remove(&privilege);
                editor.grant_option_server_privileges.remove(&privilege);
            } else {
                editor.server_privileges.insert(privilege.clone());
                editor.denied_server_privileges.remove(&privilege);
            }
        }
        cx.notify();
    }

    /// Toggle the 含授予选项 checkbox. It implies 授予: the privilege is granted and may be
    /// re-granted, and any deny of it is cleared.
    pub(super) fn toggle_create_server_grant_option(
        &mut self,
        privilege: PrivilegeId,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let editor = &mut dialog.editor;
            if editor.grant_option_server_privileges.contains(&privilege) {
                editor.grant_option_server_privileges.remove(&privilege);
            } else {
                editor
                    .grant_option_server_privileges
                    .insert(privilege.clone());
                editor.server_privileges.insert(privilege.clone());
                editor.denied_server_privileges.remove(&privilege);
            }
        }
        cx.notify();
    }

    /// Toggle the 拒绝 checkbox of one server privilege. Denying clears any grant and grant option.
    pub(super) fn toggle_create_server_privilege_deny(
        &mut self,
        privilege: PrivilegeId,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let editor = &mut dialog.editor;
            if editor.denied_server_privileges.contains(&privilege) {
                editor.denied_server_privileges.remove(&privilege);
            } else {
                editor.denied_server_privileges.insert(privilege.clone());
                editor.server_privileges.remove(&privilege);
                editor.grant_option_server_privileges.remove(&privilege);
            }
        }
        cx.notify();
    }

    /// Replace the server privileges with a preset's set (index into `catalog.server_presets`).
    pub(super) fn apply_create_server_template(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let privileges = preset_privileges(&dialog.catalog.server_presets, index);
            dialog.editor.server_privileges = privileges;
            dialog.editor.denied_server_privileges.clear();
            dialog.editor.grant_option_server_privileges.clear();
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

    /// Begin dragging the 权限 database list's right divider.
    pub(super) fn begin_create_db_list_drag(
        &mut self,
        mouse_x: Pixels,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.db_list_drag = Some((f32::from(mouse_x), dialog.db_list_width));
        }
        cx.notify();
    }

    /// Continue the 权限 database list drag. The root window routes every mouse move here so the
    /// drag survives the pointer leaving the divider.
    pub(super) fn drag_create_db_list(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(dialog) = self.create_user_dialog.as_mut() else {
            return;
        };
        let Some((start_x, start_width)) = dialog.db_list_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            dialog.db_list_drag = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.x) - start_x;
        dialog.db_list_width =
            (start_width + delta).clamp(CREATE_DB_LIST_MIN_WIDTH, CREATE_DB_LIST_MAX_WIDTH);
        cx.notify();
    }

    /// End the 权限 database list drag.
    pub(super) fn end_create_db_list_drag(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && dialog.db_list_drag.take().is_some()
        {
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

    /// Select the database shown in the 用户映射 detail pane and load its fields.
    pub(super) fn activate_create_mapping(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let (user, schema) = {
            let Some(dialog) = self.create_user_dialog.as_mut() else {
                return;
            };
            let Some(row) = dialog.mappings.get(index) else {
                return;
            };
            dialog.active_mapping = Some(index);
            (row.user_name.clone(), row.default_schema.clone())
        };
        if let Some(input) = self.create_user_mapping_user.clone() {
            input.update(cx, |input, cx| input.set_text(user, cx));
        }
        if let Some(input) = self.create_user_mapping_schema.clone() {
            input.update(cx, |input, cx| input.set_text(schema, cx));
        }
        cx.notify();
    }

    /// Toggle whether the login is mapped into one database.
    pub(super) fn toggle_create_mapping(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        {
            let Some(dialog) = self.create_user_dialog.as_mut() else {
                return;
            };
            let login = dialog.editor.account.user.clone();
            let Some(row) = dialog.mappings.get_mut(index) else {
                return;
            };
            row.mapped = !row.mapped;
            if row.mapped {
                if row.user_name.trim().is_empty() {
                    row.user_name = login;
                }
                if row.default_schema.trim().is_empty() {
                    row.default_schema = "dbo".to_string();
                }
            }
        }
        self.activate_create_mapping(index, cx);
    }

    /// Toggle one database role for a mapping row.
    pub(super) fn toggle_create_mapping_role(
        &mut self,
        index: usize,
        role: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(row) = dialog.mappings.get_mut(index)
        {
            if row.roles.contains(&role) {
                row.roles.remove(&role);
            } else {
                row.roles.insert(role);
            }
        }
        cx.notify();
    }

    /// Cycle one server-securable permission: none → granted → denied → none.
    pub(super) fn cycle_create_securable(
        &mut self,
        index: usize,
        privilege: PrivilegeId,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(row) = dialog.securables.get_mut(index)
        {
            if row.privileges.remove(&privilege) {
                row.denied.insert(privilege);
            } else if row.denied.remove(&privilege) {
                // Denied → none: nothing left to record.
            } else {
                row.privileges.insert(privilege);
            }
        }
        cx.notify();
    }

    /// Toggle the 指定旧密码 checkbox; clearing it drops any typed old password.
    pub(super) fn toggle_create_old_password(&mut self, cx: &mut Context<'_, Self>) {
        let mut cleared = false;
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.use_old_password = !dialog.use_old_password;
            if !dialog.use_old_password {
                dialog.old_password.clear();
                cleared = true;
            }
        }
        if cleared && let Some(input) = self.create_user_old_password.clone() {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        cx.notify();
    }

    /// Toggle the SQL Server 实施密码策略 (CHECK_POLICY) checkbox.
    pub(super) fn toggle_create_check_policy(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.editor.account.check_policy = !dialog.editor.account.check_policy;
        }
        cx.notify();
    }

    /// Toggle the SQL Server 实施密码过期 (CHECK_EXPIRATION) checkbox.
    pub(super) fn toggle_create_check_expiration(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.editor.account.check_expiration = !dialog.editor.account.check_expiration;
        }
        cx.notify();
    }

    /// Toggle the SQL Server 用户必须在下次登录时更改密码 (MUST_CHANGE) checkbox.
    pub(super) fn toggle_create_must_change(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.editor.account.must_change = !dialog.editor.account.must_change;
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

    /// Unmap every database and clear its picked roles.
    pub(super) fn clear_create_mappings(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            for row in &mut dialog.mappings {
                row.mapped = false;
                row.roles.clear();
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
        index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let privileges = preset_privileges(&dialog.catalog.object_presets, index);
            if let Some(index) = dialog.active_database
                && let Some(row) = dialog.db_grants.get_mut(index)
            {
                if row.scope == GrantScope::SpecificTables && row.active_table.is_none() {
                    // A preset needs a table to land on; default to the first picked one.
                    if let Some(first) = row.tables.first().cloned() {
                        row.activate_table(&first);
                    }
                }
                let is_all_tables = row.scope == GrantScope::AllTables;
                if let Some(target) = row.active_privileges_mut() {
                    *target = privileges.clone();
                }
                if is_all_tables {
                    row.enabled = !privileges.is_empty();
                }
            }
        }
        cx.notify();
    }

    /// Toggle one privilege on the active target (whole database or active table).
    pub(super) fn toggle_create_database_privilege(
        &mut self,
        privilege: PrivilegeId,
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
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let all: BTreeSet<PrivilegeId> = dialog
                .catalog
                .object()
                .map(|info| info.id.clone())
                .collect();
            if let Some(index) = dialog.active_database
                && let Some(row) = dialog.db_grants.get_mut(index)
            {
                let is_all_tables = row.scope == GrantScope::AllTables;
                let mut cleared = false;
                if let Some(target) = row.active_privileges_mut() {
                    if *target == all {
                        target.clear();
                        cleared = true;
                    } else {
                        *target = all;
                    }
                }
                // Clearing a whole-database grant leaves the database unconfigured; a table scope
                // stays selected so its revoke is still emitted.
                row.enabled = !(cleared && is_all_tables);
            }
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

    /// Select the schema whose default privileges the 默认权限 section edits (empty = all schemas).
    pub(super) fn select_create_default_schema(
        &mut self,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.default_schema = schema.clone();
            let object_type = dialog.default_object_type;
            dialog.default_active = dialog
                .default_rules
                .iter()
                .find(|rule| rule.schema == schema && rule.object_type == object_type)
                .map(|rule| rule.grantee.clone());
        }
        cx.notify();
    }

    /// Select the object kind whose default privileges the 默认权限 section edits.
    pub(super) fn select_create_default_object_type(
        &mut self,
        object_type: DefaultObjectType,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.default_object_type = object_type;
            let schema = dialog.default_schema.clone();
            dialog.default_active = dialog
                .default_rules
                .iter()
                .find(|rule| rule.schema == schema && rule.object_type == object_type)
                .map(|rule| rule.grantee.clone());
        }
        cx.notify();
    }

    /// Tick a grantee into the default-privileges rules of the active schema and object kind, or
    /// untick it (the rule is dropped, so the save revokes it).
    pub(super) fn toggle_create_default_grantee(
        &mut self,
        grantee: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let schema = dialog.default_schema.clone();
            let object_type = dialog.default_object_type;
            let existing = dialog
                .default_rule(&schema, object_type, &grantee)
                .is_some();
            if existing {
                dialog.default_rules.retain(|rule| {
                    !(rule.schema == schema
                        && rule.object_type == object_type
                        && rule.grantee == grantee)
                });
                if dialog.default_active.as_deref() == Some(grantee.as_str()) {
                    dialog.default_active = dialog
                        .default_rules
                        .iter()
                        .find(|rule| rule.schema == schema && rule.object_type == object_type)
                        .map(|rule| rule.grantee.clone());
                }
            } else {
                dialog.default_rules.push(DefaultPrivilege::new(
                    schema,
                    object_type,
                    grantee.clone(),
                ));
                dialog.default_active = Some(grantee);
            }
        }
        cx.notify();
    }

    /// Make one grantee the active target of the 默认权限 detail grid, granting it if needed.
    pub(super) fn activate_create_default_grantee(
        &mut self,
        grantee: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let schema = dialog.default_schema.clone();
            let object_type = dialog.default_object_type;
            if dialog
                .default_rule(&schema, object_type, &grantee)
                .is_none()
            {
                dialog.default_rules.push(DefaultPrivilege::new(
                    schema,
                    object_type,
                    grantee.clone(),
                ));
            }
            dialog.default_active = Some(grantee);
        }
        cx.notify();
    }

    /// Toggle one privilege on the active default-privileges rule.
    pub(super) fn toggle_create_default_privilege(
        &mut self,
        privilege: PrivilegeId,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let schema = dialog.default_schema.clone();
            let object_type = dialog.default_object_type;
            let grantee = dialog.default_active.clone();
            if let Some(grantee) = grantee
                && let Some(rule) = dialog.default_rule_mut(&schema, object_type, &grantee)
                && !rule.privileges.remove(&privilege)
            {
                rule.privileges.insert(privilege);
            }
        }
        cx.notify();
    }

    /// Select every privilege of the active default-privileges rule, or clear them all.
    pub(super) fn toggle_create_default_privileges_all(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            let object_type = dialog.default_object_type;
            let all: BTreeSet<PrivilegeId> = dialog
                .catalog
                .default_privileges_for(object_type)
                .into_iter()
                .map(|info| info.id.clone())
                .collect();
            let schema = dialog.default_schema.clone();
            let grantee = dialog.default_active.clone();
            if let Some(grantee) = grantee
                && let Some(rule) = dialog.default_rule_mut(&schema, object_type, &grantee)
            {
                if rule.privileges == all {
                    rule.privileges.clear();
                } else {
                    rule.privileges = all;
                }
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
        if let Some(input) = self.create_user_password_valid_until.clone() {
            let text = account.password_valid_until.clone().unwrap_or_default();
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
        if let Some(input) = self.create_user_default_tablespace.clone() {
            input.update(cx, |input, cx| {
                input.set_text(account.default_tablespace.clone(), cx)
            });
        }
        if let Some(input) = self.create_user_profile.clone() {
            input.update(cx, |input, cx| input.set_text(account.profile.clone(), cx));
        }
        if let Some(input) = self.create_user_tablespace_quota.clone() {
            input.update(cx, |input, cx| {
                input.set_text(account.tablespace_quota.clone(), cx)
            });
        }
        let (mapping_user, mapping_schema) = self
            .create_user_dialog
            .as_ref()
            .and_then(|dialog| {
                dialog
                    .active_mapping
                    .and_then(|index| dialog.mappings.get(index))
                    .map(|row| (row.user_name.clone(), row.default_schema.clone()))
            })
            .unwrap_or_default();
        if let Some(input) = self.create_user_mapping_user.clone() {
            input.update(cx, |input, cx| input.set_text(mapping_user, cx));
        }
        if let Some(input) = self.create_user_mapping_schema.clone() {
            input.update(cx, |input, cx| input.set_text(mapping_schema, cx));
        }
        let databases = self
            .create_user_dialog
            .as_ref()
            .map(|dialog| dialog.databases.clone())
            .unwrap_or_default();
        if let Some(combo) = self.create_user_verification_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_selected(account.login_type.clone(), cx)
            });
        }
        if let Some(combo) = self.create_user_default_database_combo.clone() {
            let options: Vec<ComboOption> = databases
                .iter()
                .map(|name| ComboOption::new(name.clone(), name.clone()))
                .collect();
            let selected = account.default_database.clone();
            combo.update(cx, |combo, cx| {
                combo.set_options(options, cx);
                combo.set_selected(selected, cx);
            });
        }
        for (input, value) in [
            (
                &self.create_user_default_language,
                account.default_language.clone(),
            ),
            (&self.create_user_certificate, account.certificate.clone()),
            (
                &self.create_user_asymmetric_key,
                account.asymmetric_key.clone(),
            ),
            (&self.create_user_credential, account.credential.clone()),
        ] {
            if let Some(input) = input.clone() {
                input.update(cx, |input, cx| input.set_text(value, cx));
            }
        }
        if let Some(combo) = self.create_user_ssl_combo.clone() {
            let selected = if account.ssl_type.is_empty() {
                "NONE".to_string()
            } else {
                account.ssl_type.clone()
            };
            combo.update(cx, |combo, cx| combo.set_selected(selected, cx));
        }
        for (input, value) in [
            (&self.create_user_ssl_cipher, account.ssl_cipher.clone()),
            (&self.create_user_ssl_issuer, account.x509_issuer.clone()),
            (&self.create_user_ssl_subject, account.x509_subject.clone()),
        ] {
            if let Some(input) = input.clone() {
                input.update(cx, |input, cx| input.set_text(value, cx));
            }
        }
        if let Some(input) = self.create_user_old_password.clone() {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
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
        let want_mappings = dialog.spec.user_mapping;
        let want_securables = dialog.spec.endpoint_permissions || dialog.spec.login_permissions;
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move {
                    let details = connection.user_details(&user, &host).await;
                    let mappings = if want_mappings {
                        connection.user_mappings(&user, &host).await
                    } else {
                        Ok(Vec::new())
                    };
                    let securables = if want_securables {
                        connection.user_securables(&user, &host).await
                    } else {
                        Ok(Vec::new())
                    };
                    (details, mappings, securables)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                dialog.loading = false;
                match result {
                    Ok((Ok(details), mappings, securables)) => {
                        dialog.apply_details(details);
                        if let Ok(mappings) = mappings {
                            dialog.set_mappings(mappings);
                        }
                        if let Ok(securables) = securables {
                            dialog.set_securables(securables);
                        }
                        app.sync_create_editor_fields(cx);
                    }
                    Ok((Err(error), _, _)) => dialog.error = Some(error.to_string()),
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
            denied_server_privileges: dialog.editor.denied_server_privileges.clone(),
            grant_option_server_privileges: dialog.editor.grant_option_server_privileges.clone(),
            grants: dialog.object_grants(),
            default_privileges: dialog.default_rules.clone(),
            roles,
            members,
            mappings: dialog.edit_mappings(),
            original_mappings: dialog.mapping_original.clone(),
            old_password: if dialog.use_old_password && !dialog.old_password.is_empty() {
                Some(dialog.old_password.clone())
            } else {
                None
            },
            securables: dialog.edit_securables(),
            original_securables: dialog.securables_original.clone(),
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
            Some((user, host)) => user_host_label(user, host, &dialog.spec),
            None => user_host_label(
                &dialog.editor.account.user,
                &dialog.editor.account.host,
                &dialog.spec,
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
        // MySQL/MariaDB refresh the grant tables once after a non-empty script; mirror it so the
        // preview matches exactly what Save runs. Other engines apply GRANT/REVOKE immediately.
        if dialog.spec.flush_privileges {
            out.push('\n');
            out.push_str(&format!("-- {}\n", t!("user.create.preview_refresh")));
            out.push_str("FLUSH PRIVILEGES;\n");
        }
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
        UserEditSection::DefaultPrivileges => "user.tab.default_privileges",
        UserEditSection::Roles => "user.tab.roles",
        UserEditSection::UserMapping => "user.tab.user_mapping",
        UserEditSection::Securables => "user.tab.securables",
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
        // No `on_cancel`: ESC must reach the window root, which closes the whole window. Handling it
        // here would clear the editor state first and leave a blank window behind.
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
            Some((user, host)) => format!(
                "{} - {}",
                user_host_label(user, host, &dialog.spec),
                t!("user.create.edit_title")
            ),
            None => t!("user.create.title").to_string(),
        };

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .track_focus(&self.create_user_focus)
            // ESC dismisses the open change preview first, then closes the whole window (there is no
            // 取消 button any more, so this and the native close button are the only ways out).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if let Some(dialog) = this.create_user_dialog.as_ref() {
                    if dialog.saving {
                        return;
                    }
                    if dialog.confirm_open {
                        this.close_create_confirm(cx);
                        return;
                    }
                }
                // Close even when a nested control already cleared the dialog state, so the OS
                // window never lingers blank.
                this.create_user_close(window, cx);
            }))
            // The 权限 divider sits between two panes, so the pointer leaves it while dragging.
            // These root-level handlers keep that drag alive anywhere in the window.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.drag_create_db_list(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.end_create_db_list_drag(cx);
                }),
            )
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
        for section in UserSection::ALL
            .into_iter()
            .filter(|section| section.visible(&dialog.spec))
        {
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
        let section = dialog.section;
        match section {
            UserSection::General => self.render_create_general(cx),
            UserSection::Advanced => self.render_create_advanced(cx),
            UserSection::ServerPrivileges => self.render_create_server_privileges(cx),
            UserSection::ObjectPrivileges => self.render_create_grants(cx),
            UserSection::DefaultPrivileges => self.render_create_default_privileges(cx),
            UserSection::Roles => self.render_create_roles(cx),
            UserSection::UserMapping => self.render_create_user_mapping(cx),
            UserSection::EndpointPermissions | UserSection::LoginPermissions => {
                self.render_create_securables(section, cx)
            }
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
        // A login with Windows / certificate / key verification has no password of its own.
        let sql_auth =
            !dialog.spec.verification_type || account.login_type.eq_ignore_ascii_case("SQL Server");

        let mut identity = div().flex().flex_col().gap_2().w_full();
        identity = identity.child(create_row(
            t!("user.field.username").to_string(),
            sized_text(self.create_user_user.as_ref(), theme),
            theme,
        ));

        // MySQL/MariaDB have a user@host identity with quick host chips; other engines identify an
        // account by name alone.
        if dialog.spec.host {
            identity = identity.child(create_row(
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
            identity = identity.child(host_choices);
        }

        if dialog.spec.authentication_plugin {
            identity = identity.child(create_row(
                t!("user.field.plugin").to_string(),
                sized_combo(self.create_user_plugin_combo.as_ref(), theme),
                theme,
            ));
        }

        if dialog.spec.verification_type {
            identity = identity.child(create_row(
                t!("user.field.verification_type").to_string(),
                sized_combo(self.create_user_verification_combo.as_ref(), theme),
                theme,
            ));
        }

        if sql_auth {
            if dialog.is_edit() {
                identity = identity.child(create_row(
                    String::new(),
                    check_row(
                        "user-create-change-password",
                        t!("user.create.change_password").to_string(),
                        dialog.change_password,
                        theme,
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_create_change_password(cx)
                        }),
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
        }

        // SQL Server's 指定旧密码: verify the current password when setting a new one.
        if dialog.spec.verification_type && sql_auth && dialog.is_edit() {
            identity = identity.child(create_row(
                String::new(),
                check_row(
                    "user-create-old-password",
                    t!("user.create.specify_old_password").to_string(),
                    dialog.use_old_password,
                    theme,
                    cx.listener(|this, _event, _window, cx| this.toggle_create_old_password(cx)),
                )
                .into_any_element(),
                theme,
            ));
            if dialog.use_old_password {
                identity = identity.child(create_row(
                    t!("user.field.old_password").to_string(),
                    sized_text(self.create_user_old_password.as_ref(), theme),
                    theme,
                ));
            }
        }

        let show_status = dialog.spec.password_expiry
            || dialog.spec.password_valid_until
            || dialog.spec.account_lock;
        let mut status = div().flex().flex_col().gap_2().w_full();
        if show_status {
            // 锁定该账号 sits beside the expiry dropdown, like the prototype.
            let mut controls = div().flex().flex_row().items_center().gap_4();
            if dialog.spec.password_expiry {
                controls =
                    controls.child(sized_combo(self.create_user_expiry_combo.as_ref(), theme));
            }
            if dialog.spec.account_lock {
                let (label_key, locked) = if dialog.spec.account_enabled {
                    ("user.field.enabled", !account.account_locked)
                } else {
                    ("user.field.locked_account", account.account_locked)
                };
                controls = controls.child(
                    check_row(
                        "user-create-locked",
                        t!(label_key).to_string(),
                        locked,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.toggle_create_locked(cx)),
                    )
                    .into_any_element(),
                );
            }
            status = status.child(create_row(
                if dialog.spec.password_expiry {
                    t!("user.field.password_expiry").to_string()
                } else {
                    String::new()
                },
                controls.into_any_element(),
                theme,
            ));
        }
        if dialog.spec.password_expiry && dialog.expiry == CreateExpiry::Interval {
            status = status.child(create_row(
                String::new(),
                sized_text(self.create_user_expiry_days.as_ref(), theme),
                theme,
            ));
        }
        if dialog.spec.password_valid_until {
            status = status.child(create_row(
                t!("user.field.password_valid_until").to_string(),
                sized_text(self.create_user_password_valid_until.as_ref(), theme),
                theme,
            ));
        }

        // Oracle's storage/profile settings.
        let show_storage =
            dialog.spec.default_tablespace || dialog.spec.profile || dialog.spec.tablespace_quota;
        let mut storage = div().flex().flex_col().gap_2().w_full();
        if dialog.spec.default_tablespace {
            storage = storage.child(create_row(
                t!("user.field.default_tablespace").to_string(),
                sized_text(self.create_user_default_tablespace.as_ref(), theme),
                theme,
            ));
        }
        if dialog.spec.tablespace_quota {
            storage = storage.child(create_row(
                t!("user.field.tablespace_quota").to_string(),
                sized_text(self.create_user_tablespace_quota.as_ref(), theme),
                theme,
            ));
        }
        if dialog.spec.profile {
            storage = storage.child(create_row(
                t!("user.field.profile").to_string(),
                sized_text(self.create_user_profile.as_ref(), theme),
                theme,
            ));
        }

        // SQL Server's login options: password policy, defaults and a credential.
        let mut login_options = div().flex().flex_col().gap_2().w_full();
        if dialog.spec.verification_type {
            if sql_auth {
                login_options = login_options.child(create_row(
                    String::new(),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_4()
                        .child(
                            check_row(
                                "user-create-check-policy",
                                t!("user.create.check_policy").to_string(),
                                account.check_policy,
                                theme,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_create_check_policy(cx)
                                }),
                            )
                            .into_any_element(),
                        )
                        .child(
                            check_row(
                                "user-create-check-expiration",
                                t!("user.create.check_expiration").to_string(),
                                account.check_expiration,
                                theme,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_create_check_expiration(cx)
                                }),
                            )
                            .into_any_element(),
                        )
                        .child(
                            check_row(
                                "user-create-must-change",
                                t!("user.create.must_change").to_string(),
                                account.must_change,
                                theme,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_create_must_change(cx)
                                }),
                            )
                            .into_any_element(),
                        )
                        .into_any_element(),
                    theme,
                ));
            }
            login_options = login_options.child(create_row(
                t!("user.field.default_database").to_string(),
                sized_combo(self.create_user_default_database_combo.as_ref(), theme),
                theme,
            ));
            login_options = login_options.child(create_row(
                t!("user.field.default_language").to_string(),
                sized_text(self.create_user_default_language.as_ref(), theme),
                theme,
            ));
            if !account.login_type.eq_ignore_ascii_case("Windows") {
                login_options = login_options
                    .child(create_row(
                        t!("user.field.certificate").to_string(),
                        sized_text(self.create_user_certificate.as_ref(), theme),
                        theme,
                    ))
                    .child(create_row(
                        t!("user.field.asymmetric_key").to_string(),
                        sized_text(self.create_user_asymmetric_key.as_ref(), theme),
                        theme,
                    ));
            }
            login_options = login_options.child(create_row(
                t!("user.field.credential").to_string(),
                sized_text(self.create_user_credential.as_ref(), theme),
                theme,
            ));
        }

        let mut limits = div().flex().flex_col().gap_2().w_full();
        for (enabled, label_key, input) in [
            (
                dialog.spec.max_questions,
                "user.field.max_questions",
                &self.create_user_max_questions,
            ),
            (
                dialog.spec.max_updates,
                "user.field.max_updates",
                &self.create_user_max_updates,
            ),
            (
                dialog.spec.max_connections,
                "user.field.max_connections",
                &self.create_user_max_connections,
            ),
            (
                dialog.spec.max_user_connections,
                "user.field.max_user_connections",
                &self.create_user_max_user_connections,
            ),
        ] {
            if !enabled {
                continue;
            }
            limits = limits.child(create_row(
                t!(label_key).to_string(),
                sized_text_w(input.as_ref(), theme, CREATE_LIMIT_WIDTH),
                theme,
            ));
        }
        let show_limits = dialog.spec.max_questions
            || dialog.spec.max_updates
            || dialog.spec.max_connections
            || dialog.spec.max_user_connections;

        let mut general = div()
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
            ));
        if show_status {
            general = general.child(section(
                t!("user.create.status").to_string(),
                None,
                status.into_any_element(),
                theme,
            ));
        }
        if show_limits {
            general = general.child(section(
                t!("user.create.limits").to_string(),
                Some(t!("user.create.limits_hint").to_string()),
                limits.into_any_element(),
                theme,
            ));
        }
        if show_storage {
            general = general.child(section(
                t!("user.create.storage").to_string(),
                None,
                storage.into_any_element(),
                theme,
            ));
        }
        if dialog.spec.verification_type {
            general = general.child(section(
                t!("user.create.login_options").to_string(),
                None,
                login_options.into_any_element(),
                theme,
            ));
        }
        general.into_any_element()
    }

    /// 高级: MySQL's `REQUIRE`/SSL settings.
    fn render_create_advanced(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let account = &dialog.editor.account;
        let specified = account.ssl_type.eq_ignore_ascii_case("SPECIFIED");

        let mut ssl = div().flex().flex_col().gap_2().w_full();
        ssl = ssl.child(create_row(
            t!("user.field.ssl_type").to_string(),
            sized_combo(self.create_user_ssl_combo.as_ref(), theme),
            theme,
        ));
        if specified {
            ssl = ssl
                .child(create_row(
                    t!("user.field.ssl_cipher").to_string(),
                    sized_text(self.create_user_ssl_cipher.as_ref(), theme),
                    theme,
                ))
                .child(create_row(
                    t!("user.field.ssl_issuer").to_string(),
                    sized_text(self.create_user_ssl_issuer.as_ref(), theme),
                    theme,
                ))
                .child(create_row(
                    t!("user.field.ssl_subject").to_string(),
                    sized_text(self.create_user_ssl_subject.as_ref(), theme),
                    theme,
                ));
        }

        div()
            .id("user-create-advanced")
            .flex()
            .flex_col()
            .gap_5()
            .w_full()
            .child(section(
                t!("user.ssl.section").to_string(),
                None,
                ssl.into_any_element(),
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
        let checked_count = dialog
            .editor
            .server_privileges
            .iter()
            .filter(|privilege| dialog.catalog.is_server(privilege))
            .count();

        let mut templates = div().flex().flex_row().items_center().gap_2().w_full();
        for (index, preset) in dialog.catalog.server_presets.iter().enumerate() {
            let active = dialog.editor.server_privileges
                == preset_privileges(&dialog.catalog.server_presets, index);
            let label = catalog_label(&preset.label_key, &preset.label);
            templates = templates.child(
                div()
                    .id(SharedString::from(format!("user-create-template-{index}")))
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
                        this.apply_create_server_template(index, cx)
                    }))
                    .child(label),
            );
        }

        // Grouped by the catalog, so the global set reads as categories instead of one long grid.
        let grant_option_supported = dialog.catalog.grant_option_supported;
        let deny_supported = dialog.catalog.deny_supported;

        // One header (`权限 | 授予 | 含授予选项 | 拒绝`), a sub-header per group and one row per
        // privilege, mirroring Navicat's server-permission table.
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.0))
            .flex_none()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(11.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px_2()
                    .child(t!("user.privilege.column").to_string()),
            )
            .child(
                div()
                    .w(px(SERVER_PRIV_CHECK_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(t!("user.privilege.grant").to_string()),
            );
        if grant_option_supported {
            header = header.child(
                div()
                    .w(px(SERVER_PRIV_CHECK_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(t!("user.privilege.grant_option").to_string()),
            );
        }
        if deny_supported {
            header = header.child(
                div()
                    .w(px(SERVER_PRIV_CHECK_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(t!("user.privilege.deny").to_string()),
            );
        }

        let mut rows = div().flex().flex_col().w_full().child(header);
        for group in &dialog.catalog.groups {
            let offered = dialog
                .catalog
                .group_privileges(&group.id, PrivilegeScope::Server);
            if offered.is_empty() {
                continue;
            }
            let group_label = catalog_label(&group.label_key, &group.label);
            let group_key = group.id.clone();
            rows = rows.child(
                div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(group_label),
            );
            for (position, privilege) in offered.into_iter().enumerate() {
                let checked = dialog.editor.server_privileges.contains(&privilege);
                let option = dialog
                    .editor
                    .grant_option_server_privileges
                    .contains(&privilege);
                let is_denied = dialog.editor.denied_server_privileges.contains(&privilege);
                let keyword = privilege.as_str().to_string();
                let description = privilege_label(&dialog.catalog, &privilege);
                let grant_key = privilege.clone();
                let option_key = privilege.clone();
                let deny_key = privilege.clone();
                let mut row = div()
                    .id(SharedString::from(format!(
                        "user-create-server-row-{group_key}-{position}"
                    )))
                    .tooltip(move |_, cx| cx.new(|_| PrivilegeTooltip(description.clone())).into())
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_h(px(CREATE_ROW_HEIGHT))
                    .flex_none()
                    .when(position % 2 == 1, move |style| {
                        style.bg(rgb(theme.row_alt_bg))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .px_2()
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .text_color(rgb(theme.text))
                            .child(keyword),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-server-grant-{group_key}-{position}"
                            )))
                            .w(px(SERVER_PRIV_CHECK_WIDTH))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_server_privilege(grant_key.clone(), cx)
                            }))
                            .child(checkbox_box(checked, theme)),
                    );
                if grant_option_supported {
                    row = row.child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-server-option-{group_key}-{position}"
                            )))
                            .w(px(SERVER_PRIV_CHECK_WIDTH))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_server_grant_option(option_key.clone(), cx)
                            }))
                            .child(checkbox_box(option, theme)),
                    );
                }
                if deny_supported {
                    row = row.child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-server-deny-{group_key}-{position}"
                            )))
                            .w(px(SERVER_PRIV_CHECK_WIDTH))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_server_privilege_deny(deny_key.clone(), cx)
                            }))
                            .child(checkbox_box(is_denied, theme)),
                    );
                }
                rows = rows.child(row);
            }
        }

        let panel = div()
            .flex()
            .flex_col()
            .w_full()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .overflow_hidden()
            .child(rows);

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
                panel.into_any_element(),
                theme,
            ))
            .into_any_element()
    }

    /// 权限: a database list on the left and the selected database's privilege detail on the right.
    /// The divider between them is drag-resizable.
    fn render_create_grants(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
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
            .child(
                // The list and its right-edge divider travel together, so the divider stays flush
                // against the list however it is resized.
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .h_full()
                    .min_h(px(0.0))
                    .child(self.render_create_database_list(&visible, cx))
                    .child(
                        ui::pane_resize_divider("user-create-db-divider", theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                this.begin_create_db_list_drag(event.position.x, cx)
                            }),
                        ),
                    ),
            )
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
            let (badge, authorized) = row.summary(&dialog.catalog.object_presets);
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
            .w(px(dialog.db_list_width))
            .flex_none()
            .h_full()
            .min_h(px(0.0))
            .pr_3()
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
        for (index, preset) in dialog.catalog.object_presets.iter().enumerate() {
            let preset_set = preset_privileges(&dialog.catalog.object_presets, index);
            let active = if preset_set.is_empty() {
                privileges.is_empty()
            } else {
                !privileges.is_empty() && privileges == preset_set
            };
            let label = catalog_label(&preset.label_key, &preset.label);
            presets = presets.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-db-template-{index}"
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
                        this.apply_create_database_template(index, cx)
                    }))
                    .child(label),
            );
        }

        // The grouped fine-grained privileges from the catalog.
        let mut groups = div().flex().flex_col().gap_3().w_full();
        for group in &dialog.catalog.groups {
            let offered = dialog
                .catalog
                .group_privileges(&group.id, PrivilegeScope::Object);
            if offered.is_empty() {
                continue;
            }
            let group_label = catalog_label(&group.label_key, &group.label);
            let group_key = group.id.clone();
            let mut grid = div().flex().flex_row().flex_wrap().w_full();
            for (position, privilege) in offered.into_iter().enumerate() {
                let checked = privileges.contains(&privilege);
                let keyword = privilege.as_str().to_string();
                let description = privilege_label(&dialog.catalog, &privilege);
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
                        .tooltip(move |_, cx| {
                            cx.new(|_| PrivilegeTooltip(description.clone())).into()
                        })
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_create_database_privilege(privilege.clone(), cx)
                        }))
                        .child(checkbox_box(checked, theme))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(theme.text))
                                .child(keyword),
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
                            .child(group_label),
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

    /// 默认权限: the account's PostgreSQL `ALTER DEFAULT PRIVILEGES` rules — a schema list on the
    /// left and, on the right, the object kind and the grantee roles that receive the privileges.
    fn render_create_default_privileges(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let schema = dialog.default_schema.clone();
        let object_type = dialog.default_object_type;

        // Left: every schema plus the implicit "all schemas" row.
        let mut schema_list = div()
            .id("user-create-default-schemas")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&dialog.default_schema_scroll);
        let entries: Vec<(String, String)> =
            std::iter::once((String::new(), t!("user.default.all_schemas").to_string()))
                .chain(
                    dialog
                        .default_schemas
                        .iter()
                        .cloned()
                        .map(|name| (name.clone(), name)),
                )
                .collect();
        for (index, (value, label)) in entries.iter().enumerate() {
            let active = dialog.default_schema == *value;
            let selected = value.clone();
            schema_list = schema_list.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-default-schema-{index}"
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .px_2()
                    .flex_none()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .when(active, move |style| style.bg(rgb(theme.tree_selected_bg)))
                    .when(!active, move |style| {
                        style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.select_create_default_schema(selected.clone(), cx)
                    }))
                    .child(tree_icon(
                        "icons/database.svg",
                        if active {
                            theme.icon_database_active
                        } else {
                            theme.text_muted
                        },
                    ))
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
                            .child(label.clone()),
                    ),
            );
        }

        let left = div()
            .flex()
            .flex_col()
            .w(px(CREATE_DEFAULT_SCHEMA_WIDTH))
            .flex_none()
            .h_full()
            .min_h(px(0.0))
            .pr_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .pb_2()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(
                        t!(
                            "user.default.schema_count",
                            count = dialog.default_schemas.len()
                        )
                        .to_string(),
                    ),
            )
            .child(schema_list);

        // Right: the object kinds, then the grantee list and the fine-grained grid.
        let mut kinds = div().flex().flex_row().items_center().gap_1().flex_none();
        for option in DefaultObjectType::ALL {
            let active = option == object_type;
            kinds = kinds.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-default-kind-{}",
                        option.keyword()
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
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.select_create_default_object_type(option, cx)
                    }))
                    .child(t!(default_object_type_key(option)).to_string()),
            );
        }

        let grantees = dialog.default_grantees(&schema, object_type);
        let active = dialog.default_active.clone();
        let active_privileges = active
            .as_ref()
            .and_then(|grantee| grantees.get(grantee).cloned())
            .unwrap_or_default();

        let mut accounts = div()
            .id("user-create-default-accounts")
            .flex()
            .flex_col()
            .max_h(px(CREATE_DEFAULT_LIST_MAX_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&dialog.default_account_scroll)
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg));
        for (index, (user, host)) in dialog.accounts.iter().enumerate() {
            let label = user_host_label(user, host, &dialog.spec);
            let checked = grantees.contains_key(user);
            let is_active = active.as_deref() == Some(user.as_str());
            let count = grantees.get(user).map(BTreeSet::len).unwrap_or(0);
            let activate = user.clone();
            let toggle = user.clone();
            let mut entry = div()
                .id(SharedString::from(format!(
                    "user-create-default-account-{index}"
                )))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(24.0))
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
                    this.activate_create_default_grantee(activate.clone(), cx)
                }))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "user-create-default-account-check-{index}"
                        )))
                        .flex()
                        .items_center()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_create_default_grantee(toggle.clone(), cx);
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
                        .child(label),
                );
            if count > 0 {
                entry = entry.child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(rgb(theme.primary))
                        .child(count.to_string()),
                );
            }
            accounts = accounts.child(entry);
        }

        let mut grid = div().flex().flex_row().flex_wrap().w_full();
        for (position, info) in dialog
            .catalog
            .default_privileges_for(object_type)
            .into_iter()
            .enumerate()
        {
            let checked = active_privileges.contains(&info.id);
            let keyword = info.id.as_str().to_string();
            let description = catalog_label(&info.label_key, &info.label);
            let privilege = info.id.clone();
            grid = grid.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-default-priv-{position}"
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w(px(210.0))
                    .h(px(CREATE_ROW_HEIGHT))
                    .cursor_pointer()
                    .tooltip(move |_, cx| cx.new(|_| PrivilegeTooltip(description.clone())).into())
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_create_default_privilege(privilege.clone(), cx)
                    }))
                    .child(checkbox_box(checked, theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text))
                            .child(keyword),
                    ),
            );
        }

        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("user.tab.default_privileges").to_string()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.default.hint").to_string()),
            );

        let mut column = div()
            .id("user-create-default-detail")
            .flex()
            .flex_col()
            .gap_4()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .child(header)
            .child(section(
                t!("user.default.object_kind").to_string(),
                None,
                kinds.into_any_element(),
                theme,
            ))
            .child(section(
                t!("user.default.grantees").to_string(),
                Some(t!("user.default.grantees_hint", count = grantees.len()).to_string()),
                accounts.into_any_element(),
                theme,
            ));
        if dialog.default_active.is_some() {
            let account_label = dialog.default_active.clone().unwrap_or_default();
            let schema_label = if schema.is_empty() {
                t!("user.default.all_schemas").to_string()
            } else {
                schema.clone()
            };
            let hint = t!(
                "user.default.applies",
                account = account_label,
                schema = schema_label,
                object = t!(default_object_type_key(object_type)).to_string()
            )
            .to_string();
            column = column.child(section(
                t!("user.create.fine_grained").to_string(),
                Some(hint),
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w_full()
                    .child(
                        div().flex().flex_row().justify_end().w_full().child(
                            div()
                                .id("user-create-default-priv-toggle-all")
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(rgb(theme.primary))
                                .hover(move |style| style.text_color(rgb(theme.text)))
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.toggle_create_default_privileges_all(cx)
                                }))
                                .child(t!("user.create.toggle_all").to_string()),
                        ),
                    )
                    .child(grid)
                    .into_any_element(),
                theme,
            ));
        } else {
            column = column.child(section(
                t!("user.create.fine_grained").to_string(),
                None,
                tree_message(
                    t!("user.default.select_grantee").to_string(),
                    8.0,
                    theme.text_muted,
                )
                .into_any_element(),
                theme,
            ));
        }

        div()
            .id("user-create-default")
            .flex()
            .flex_row()
            .gap_4()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .child(left)
            .child(column)
            .into_any_element()
    }

    /// The 指定具体表 picker of the active database. Clicking a table's name makes it active; its
    /// check box picks it into the scope, and each selected table shows its own privilege summary.
    fn render_create_table_picker(
        &self,
        database: &str,
        selected: &[String],
        active: Option<&str>,
        privileges: &BTreeMap<String, BTreeSet<PrivilegeId>>,
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
            let (badge, authorized) = privilege_summary(
                privileges.get(table).unwrap_or(&empty),
                &dialog.catalog.object_presets,
            );
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
    /// 用户映射: a database list on the left and the selected database's mapping detail on the
    /// right (SQL Server's login → database user mapping and database roles).
    fn render_create_user_mapping(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };

        let mut rows = div()
            .id("user-create-mapping-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&dialog.mapping_scroll);
        for (index, row) in dialog.mappings.iter().enumerate() {
            let active = dialog.active_mapping == Some(index);
            let mapped = row.mapped;
            let name = row.database.clone();
            let badge = if mapped {
                if row.user_name.trim().is_empty() {
                    dialog.editor.account.user.clone()
                } else {
                    row.user_name.clone()
                }
            } else {
                t!("user.mapping.unmapped").to_string()
            };
            let mut entry = div()
                .id(SharedString::from(format!("user-create-mapping-{index}")))
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
                    this.activate_create_mapping(index, cx)
                }))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "user-create-mapping-check-{index}"
                        )))
                        .flex()
                        .items_center()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_create_mapping(index, cx);
                        }))
                        .child(checkbox_box(mapped, theme)),
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
            if mapped {
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
            rows = rows.child(entry);
        }

        let list = div()
            .flex()
            .flex_col()
            .w(px(CREATE_DB_LIST_DEFAULT_WIDTH))
            .flex_none()
            .h_full()
            .min_h(px(0.0))
            .pr_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .pb_2()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.mapping.count", count = dialog.mappings.len()).to_string())
                    .child(
                        div()
                            .id("user-create-mapping-clear")
                            .cursor_pointer()
                            .text_color(rgb(theme.primary))
                            .hover(move |style| style.text_color(rgb(theme.text)))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.clear_create_mappings(cx)
                            }))
                            .child(t!("user.create.clear_all").to_string()),
                    ),
            )
            .child(rows);

        div()
            .id("user-create-mapping")
            .flex()
            .flex_row()
            .gap_4()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .child(list)
            .child(self.render_create_mapping_detail(cx))
            .into_any_element()
    }

    /// The 用户映射 section's right detail pane for the active database.
    fn render_create_mapping_detail(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let Some(index) = dialog.active_mapping else {
            return tree_message(
                t!("user.mapping.select_database").to_string(),
                8.0,
                theme.text_muted,
            )
            .into_any_element();
        };
        let Some(row) = dialog.mappings.get(index) else {
            return div().into_any_element();
        };
        let database = row.database.clone();
        let mapped = row.mapped;
        let roles = row.roles.clone();
        let available = row.available_roles.clone();

        let mut form = div().flex().flex_col().gap_2().w_full();
        form = form.child(create_row(
            t!("user.mapping.database").to_string(),
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(database)
                .into_any_element(),
            theme,
        ));
        form = form.child(create_row(
            t!("user.mapping.mapped").to_string(),
            check_row(
                "user-create-mapping-mapped",
                t!("user.mapping.mapped").to_string(),
                mapped,
                theme,
                cx.listener(move |this, _event, _window, cx| this.toggle_create_mapping(index, cx)),
            )
            .into_any_element(),
            theme,
        ));
        form = form.child(create_row(
            t!("user.mapping.user").to_string(),
            sized_text(self.create_user_mapping_user.as_ref(), theme),
            theme,
        ));
        form = form.child(create_row(
            t!("user.mapping.default_schema").to_string(),
            sized_text(self.create_user_mapping_schema.as_ref(), theme),
            theme,
        ));

        let mut column = div().flex().flex_col().gap_4().flex_1().min_w(px(0.0));
        column = column.child(section(
            t!("user.mapping.mapping").to_string(),
            Some(t!("user.mapping.user_hint").to_string()),
            form.into_any_element(),
            theme,
        ));

        if available.is_empty() {
            column = column.child(section(
                t!("user.mapping.roles").to_string(),
                None,
                tree_message(
                    t!("user.mapping.no_roles").to_string(),
                    8.0,
                    theme.text_muted,
                )
                .into_any_element(),
                theme,
            ));
        } else {
            let mut grid = div().flex().flex_row().flex_wrap().w_full();
            for (position, role) in available.iter().enumerate() {
                let checked = roles.contains(role);
                let label = role.clone();
                let key = role.clone();
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!(
                            "user-create-mapping-role-{index}-{position}"
                        )))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w(px(200.0))
                        .h(px(CREATE_ROW_HEIGHT))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_create_mapping_role(index, key.clone(), cx)
                        }))
                        .child(checkbox_box(checked, theme))
                        .child(div().text_size(px(12.0)).child(label)),
                );
            }
            column = column.child(section(
                t!("user.mapping.roles").to_string(),
                Some(t!("user.mapping.roles_hint").to_string()),
                grid.into_any_element(),
                theme,
            ));
        }
        column.into_any_element()
    }

    /// 终端节点权限 / 登录权限: a matrix of server securables × the class's permissions.
    fn render_create_securables(
        &self,
        active_section: UserSection,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let Some(class_id) = active_section.securable_class_id() else {
            return div().into_any_element();
        };
        let Some(class) = dialog
            .catalog
            .securable_classes
            .iter()
            .find(|class| class.id == class_id)
        else {
            return tree_message(
                t!("user.securables.unsupported").to_string(),
                8.0,
                theme.text_muted,
            )
            .into_any_element();
        };
        let privileges = class.privileges.clone();
        let class_keyword = class.class.clone();
        let title = catalog_label(&class.label_key, &class.label);
        let cell_hint = t!("user.securables.cell_hint").to_string();

        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.0))
            .flex_none()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(11.0))
            .child(
                div()
                    .w(px(SECURABLE_NAME_WIDTH))
                    .flex_none()
                    .px_2()
                    .child(t!("user.securables.name").to_string()),
            );
        for privilege in &privileges {
            header = header.child(
                div()
                    .w(px(SECURABLE_COL_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(privilege.as_str().to_string()),
            );
        }

        let mut rows = div().flex().flex_col();
        let mut shown = 0usize;
        for (index, row) in dialog.securables.iter().enumerate() {
            if row.class != class_keyword {
                continue;
            }
            let name = row.name.clone();
            let row_index = index;
            let position = shown;
            shown += 1;
            let mut entry = div()
                .id(SharedString::from(format!(
                    "user-create-securable-{class_id}-{position}"
                )))
                .flex()
                .flex_row()
                .items_center()
                .h(px(CREATE_ROW_HEIGHT))
                .flex_none()
                .when(position % 2 == 1, move |style| {
                    style.bg(rgb(theme.row_alt_bg))
                })
                .child(
                    div()
                        .w(px(SECURABLE_NAME_WIDTH))
                        .flex_none()
                        .px_2()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(tree_icon("icons/user.svg", theme.icon_users))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.0))
                                .child(name),
                        ),
                );
            for (column, privilege) in privileges.iter().enumerate() {
                let checked = row.privileges.contains(privilege);
                let denied = row.denied.contains(privilege);
                let cycle_key = privilege.clone();
                let description = cell_hint.clone();
                entry = entry.child(
                    div()
                        .id(SharedString::from(format!(
                            "user-create-securable-{class_id}-{position}-{column}"
                        )))
                        .w(px(SECURABLE_COL_WIDTH))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .tooltip(move |_, cx| {
                            cx.new(|_| PrivilegeTooltip(description.clone())).into()
                        })
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.cycle_create_securable(row_index, cycle_key.clone(), cx)
                        }))
                        .child(securable_state_box(checked, denied, theme)),
                );
            }
            rows = rows.child(entry);
        }

        let body: AnyElement = if shown == 0 {
            tree_message(
                t!("user.securables.empty").to_string(),
                8.0,
                theme.text_muted,
            )
            .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!(
                    "user-create-securables-scroll-{class_id}"
                )))
                .flex()
                .flex_col()
                .max_h(px(400.0))
                .overflow_y_scroll()
                .child(rows)
                .into_any_element()
        };

        section(
            title,
            Some(format!(
                "{} · {}",
                t!("user.securables.hint", count = shown),
                t!("user.securables.cell_hint")
            )),
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
            let label = user_host_label(user, host, &dialog.spec);
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
            Some((user, host)) => user_host_label(user, host, &dialog.spec),
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
        right = right.child(self.win_button(
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

    fn p(name: &str) -> PrivilegeId {
        PrivilegeId::new(name)
    }

    #[test]
    fn a_spec_hides_sections_its_engine_lacks() {
        let mysql = UserEditorSpec::mysql();
        assert!(UserSection::ServerPrivileges.visible(&mysql));
        assert!(UserSection::ObjectPrivileges.visible(&mysql));
        assert!(UserSection::Roles.visible(&mysql));
        assert!(UserSection::General.visible(&mysql));
        assert!(UserSection::Sql.visible(&mysql));
        assert!(!UserSection::UserMapping.visible(&mysql));
        assert!(UserSection::Advanced.visible(&mysql));

        // An engine without object-grant management hides the 权限 section.
        let sqlserver = UserEditorSpec {
            object_privileges: false,
            ..UserEditorSpec::mysql()
        };
        assert!(!UserSection::ObjectPrivileges.visible(&sqlserver));
        assert!(UserSection::ServerPrivileges.visible(&sqlserver));
        assert!(UserSection::General.visible(&sqlserver));
        assert!(UserSection::Sql.visible(&sqlserver));

        // SQL Server shows the 用户映射 section.
        let sqlserver_mapping = UserEditorSpec {
            user_mapping: true,
            ..UserEditorSpec::mysql()
        };
        assert!(UserSection::UserMapping.visible(&sqlserver_mapping));

        // SQL Server shows the 终端节点权限 / 登录权限 sections.
        assert!(!UserSection::EndpointPermissions.visible(&mysql));
        assert!(!UserSection::LoginPermissions.visible(&mysql));
        let sqlserver_securables = UserEditorSpec {
            endpoint_permissions: true,
            login_permissions: true,
            ..UserEditorSpec::mysql()
        };
        assert!(UserSection::EndpointPermissions.visible(&sqlserver_securables));
        assert!(UserSection::LoginPermissions.visible(&sqlserver_securables));
        assert_eq!(
            UserSection::EndpointPermissions.securable_class_id(),
            Some("endpoint")
        );
        assert_eq!(
            UserSection::LoginPermissions.securable_class_id(),
            Some("login")
        );
    }

    #[test]
    fn a_preset_resolves_to_its_privilege_set() {
        let presets = vec![PrivilegePreset {
            id: "read".to_string(),
            label_key: None,
            label: "Read".to_string(),
            privileges: vec![p("SELECT")],
        }];
        assert_eq!(
            preset_privileges(&presets, 0),
            BTreeSet::from([p("SELECT")])
        );
        assert!(preset_privileges(&presets, 9).is_empty());
    }

    #[test]
    fn catalog_label_resolves_a_known_key_and_falls_back() {
        assert_eq!(
            catalog_label(&Some("user.priv.select".to_string()), "SELECT"),
            t!("user.priv.select").to_string()
        );
        assert_eq!(catalog_label(&None, "RAW"), "RAW");
    }

    #[test]
    fn privilege_summary_names_a_matching_preset() {
        let presets = vec![PrivilegePreset {
            id: "read".to_string(),
            label_key: None,
            label: "Read".to_string(),
            privileges: vec![p("SELECT")],
        }];
        assert_eq!(
            privilege_summary(&BTreeSet::from([p("SELECT")]), &presets).0,
            "Read (1)"
        );
        assert_eq!(
            privilege_summary(&BTreeSet::from([p("INSERT")]), &presets).0,
            format!("{} (1)", t!("user.create.custom"))
        );
        assert!(!privilege_summary(&BTreeSet::new(), &presets).1);
    }

    #[test]
    fn an_edit_dialog_remembers_its_identity() {
        let dialog = UserCreateDialog::new(
            0,
            "caching_sha2_password".to_string(),
            UserEditorSpec::mysql(),
            PrivilegeCatalog::default(),
            false,
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
        let dialog = UserCreateDialog::new(
            0,
            "caching_sha2_password".to_string(),
            UserEditorSpec::mysql(),
            PrivilegeCatalog::default(),
            false,
            None,
        );
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
        grant_with(database, name, &[p("SELECT")])
    }

    fn grant_with(database: &str, name: &str, privileges: &[PrivilegeId]) -> ObjectGrant {
        ObjectGrant {
            database: database.to_string(),
            schema: String::new(),
            name: name.to_string(),
            privileges: privileges.iter().cloned().collect(),
        }
    }

    fn new_dialog(account: Option<(String, String)>) -> UserCreateDialog {
        UserCreateDialog::new(
            0,
            "caching_sha2_password".to_string(),
            UserEditorSpec::mysql(),
            PrivilegeCatalog::default(),
            false,
            account,
        )
    }

    #[test]
    fn rebuild_maps_whole_database_table_and_ungranted_rows() {
        let mut dialog = new_dialog(None);
        dialog.databases = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        dialog.editor.grants = vec![
            grant("a", ""),
            grant("b", "t1"),
            grant_with(
                "b",
                "t2",
                &[p("SELECT"), p("INSERT"), p("UPDATE"), p("DELETE")],
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
        let mut dialog = new_dialog(None);
        dialog.databases = vec!["shop".to_string()];
        dialog.editor.grants = vec![
            grant("shop", "account"),
            grant_with(
                "shop",
                "orders",
                &[p("SELECT"), p("INSERT"), p("UPDATE"), p("DELETE")],
            ),
        ];
        dialog.rebuild_db_grants();

        let row = &dialog.db_grants[0];
        assert_eq!(
            row.table_privileges.get("account"),
            Some(&[p("SELECT")].into_iter().collect())
        );
        assert_eq!(
            row.table_privileges.get("orders"),
            Some(
                &[p("SELECT"), p("INSERT"), p("UPDATE"), p("DELETE")]
                    .into_iter()
                    .collect()
            )
        );
        // A mixed set is no longer flattened into one preset shared by both tables.
        assert_eq!(
            row.summary(&[]).0,
            format!("{} (4)", t!("user.create.custom"))
        );

        let grants = dialog.object_grants();
        let account = grants
            .iter()
            .find(|grant| grant.name == "account")
            .expect("the account grant");
        let orders = grants
            .iter()
            .find(|grant| grant.name == "orders")
            .expect("the orders grant");
        assert_eq!(account.privileges, [p("SELECT")].into_iter().collect());
        assert_eq!(orders.privileges.len(), 4);
    }

    #[test]
    fn editing_one_table_does_not_touch_another() {
        let mut dialog = new_dialog(None);
        dialog.databases = vec!["shop".to_string()];
        dialog.rebuild_db_grants();
        dialog.db_grants[0].activate_table("account");
        dialog.db_grants[0].activate_table("orders");

        dialog.db_grants[0]
            .active_privileges_mut()
            .expect("an active table")
            .insert(p("SELECT"));
        dialog.db_grants[0].active_table = Some("account".to_string());
        dialog.db_grants[0]
            .active_privileges_mut()
            .expect("an active table")
            .insert(p("INSERT"));

        let row = &dialog.db_grants[0];
        assert_eq!(
            row.table_privileges.get("orders"),
            Some(&[p("SELECT")].into_iter().collect())
        );
        assert_eq!(
            row.table_privileges.get("account"),
            Some(&[p("INSERT")].into_iter().collect())
        );
    }

    #[test]
    fn specific_scope_without_tables_falls_back_to_a_database_grant() {
        let mut dialog = new_dialog(None);
        dialog.databases = vec!["a".to_string()];
        dialog.rebuild_db_grants();
        dialog.db_grants[0].enabled = true;
        dialog.db_grants[0].scope = GrantScope::SpecificTables;
        dialog.db_grants[0].privileges.insert(p("SELECT"));

        let grants = dialog.object_grants();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].database, "a");
        assert!(grants[0].name.is_empty());
    }

    #[test]
    fn rebuild_keeps_granted_databases_missing_from_the_listing() {
        let mut dialog = new_dialog(None);
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

        let mut dialog = new_dialog(Some(("test".to_string(), "%".to_string())));
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

    #[test]
    fn default_privilege_rules_round_trip_through_the_dialog() {
        let mut dialog = new_dialog(Some(("alice".to_string(), String::new())));
        dialog.default_schemas = vec!["public".to_string()];
        dialog.apply_details(UserDetails {
            account: UserAccount {
                user: "alice".to_string(),
                ..Default::default()
            },
            default_privileges: vec![DefaultPrivilege {
                schema: "public".to_string(),
                object_type: DefaultObjectType::Tables,
                grantee: "bob".to_string(),
                privileges: [p("SELECT")].into_iter().collect(),
            }],
            ..Default::default()
        });

        // Opening the window selects the first loaded rule.
        assert_eq!(dialog.default_schema, "public");
        assert_eq!(dialog.default_object_type, DefaultObjectType::Tables);
        assert_eq!(dialog.default_active.as_deref(), Some("bob"));
        assert!(
            dialog
                .default_grantees("public", DefaultObjectType::Tables)
                .contains_key("bob")
        );

        // Clearing the rule's privileges still keeps it, so the save revokes them rather than
        // silently dropping the change.
        dialog
            .default_rule_mut("public", DefaultObjectType::Tables, "bob")
            .expect("the bob rule")
            .privileges
            .clear();
        assert!(
            dialog
                .default_rule("public", DefaultObjectType::Tables, "bob")
                .is_some_and(|rule| rule.privileges.is_empty())
        );
    }
}

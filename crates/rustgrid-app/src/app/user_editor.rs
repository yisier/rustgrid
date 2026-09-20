//! The account editor tab (Navicat's "用户" designer): the general/advanced attributes, role
//! memberships, server privileges and object privileges of one MySQL account, plus a live SQL
//! preview and the "add privilege" popup.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::*;

const USER_FIELD_WIDTH: f32 = 320.0;
const USER_LABEL_WIDTH: f32 = 130.0;
const USER_ROW_HEIGHT: f32 = 22.0;

/// The sub-tabs of the user editor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UserTab {
    General,
    Advanced,
    MemberOf,
    Members,
    ServerPrivileges,
    Privileges,
    Sql,
}

impl UserTab {
    pub(super) const ALL: [UserTab; 7] = [
        UserTab::General,
        UserTab::Advanced,
        UserTab::MemberOf,
        UserTab::Members,
        UserTab::ServerPrivileges,
        UserTab::Privileges,
        UserTab::Sql,
    ];

    pub(super) fn label_key(self) -> &'static str {
        match self {
            UserTab::General => "user.tab.general",
            UserTab::Advanced => "user.tab.advanced",
            UserTab::MemberOf => "user.tab.member_of",
            UserTab::Members => "user.tab.members",
            UserTab::ServerPrivileges => "user.tab.server_privileges",
            UserTab::Privileges => "user.tab.privileges",
            UserTab::Sql => "user.tab.sql",
        }
    }

    pub(super) fn id(self) -> &'static str {
        match self {
            UserTab::General => "user-tab-general",
            UserTab::Advanced => "user-tab-advanced",
            UserTab::MemberOf => "user-tab-member-of",
            UserTab::Members => "user-tab-members",
            UserTab::ServerPrivileges => "user-tab-server-privileges",
            UserTab::Privileges => "user-tab-privileges",
            UserTab::Sql => "user-tab-sql",
        }
    }
}

/// The account attributes that are plain text fields, identified by their callback tag.
#[derive(Clone, Copy)]
enum InputTag {
    User,
    Host,
    Password,
    Confirm,
    ExpiryDays,
    MaxQuestions,
    MaxUpdates,
    MaxConnections,
    MaxUserConnections,
    Issuer,
    Subject,
    Cipher,
}

/// The combo boxes owned by the editor.
#[derive(Clone, Copy)]
enum ComboField {
    Plugin,
    Expiry,
    Ssl,
}

/// The pending/loaded state of the "add privilege" popup.
struct AddPrivilege {
    databases: Vec<String>,
    /// Loaded table names per database.
    tables: BTreeMap<String, Vec<String>>,
    /// The database whose tables are shown.
    expanded: Option<String>,
    /// The selected node: `(database, object name)`; the name is empty for a database-wide grant.
    selected: Option<(String, String)>,
    /// The privileges checked for the selected node.
    privileges: BTreeSet<Privilege>,
    loading: bool,
    error: Option<String>,
}

impl AddPrivilege {
    fn new() -> Self {
        Self {
            databases: Vec::new(),
            tables: BTreeMap::new(),
            expanded: None,
            selected: None,
            privileges: BTreeSet::new(),
            loading: false,
            error: None,
        }
    }
}

/// One account editor tab.
pub(super) struct UserEditor {
    pub(super) app: WeakEntity<AppView>,
    runtime: Arc<Runtime>,
    pub(super) theme: Theme,
    pub(super) id: u64,
    pub(super) connection: Arc<dyn Connection>,
    pub(super) connection_name: String,

    /// The account as loaded, or `None` while creating a new one.
    original: Option<UserDetails>,
    /// `(user, host)` of the account being edited, used to de-duplicate open editors.
    original_account: Option<(String, String)>,
    pub(super) account: UserAccount,
    pub(super) password: String,
    guard_password: String,
    pub(super) server_privileges: BTreeSet<Privilege>,
    pub(super) grants: Vec<ObjectGrant>,
    /// Role edges where this account is the member: `(role user, role host) -> admin option`.
    pub(super) roles: BTreeMap<(String, String), bool>,
    /// Role edges where this account is the role: `(member user, member host) -> admin option`.
    pub(super) members: BTreeMap<(String, String), bool>,
    /// Every account on the server, the candidate list of the membership tabs.
    pub(super) accounts: Vec<UserAccount>,
    pub(super) tab: UserTab,
    pub(super) dirty: bool,
    loading: bool,
    saving: bool,
    /// The selected row of the object-privilege grid.
    selected_grant: Option<usize>,
    add_privilege: Option<AddPrivilege>,

    user_input: Entity<TextInput>,
    host_input: Entity<TextInput>,
    password_input: Entity<TextInput>,
    confirm_input: Entity<TextInput>,
    expiry_days_input: Entity<TextInput>,
    max_questions_input: Entity<TextInput>,
    max_updates_input: Entity<TextInput>,
    max_connections_input: Entity<TextInput>,
    max_user_connections_input: Entity<TextInput>,
    issuer_input: Entity<TextInput>,
    subject_input: Entity<TextInput>,
    cipher_input: Entity<TextInput>,
    plugin_combo: Entity<ComboBox>,
    expiry_combo: Entity<ComboBox>,
    ssl_combo: Entity<ComboBox>,

    sql_scroll: ScrollHandle,
}

fn text_input(
    theme: Theme,
    value: String,
    weak: WeakEntity<UserEditor>,
    tag: InputTag,
    masked: bool,
    cx: &mut Context<'_, UserEditor>,
) -> Entity<TextInput> {
    cx.new(move |cx| {
        TextInput::new(
            theme,
            value,
            TextInputOptions {
                masked,
                placeholder: SharedString::default(),
                accepts: None,
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = weak.update(cx, |view, cx| view.input_changed(tag, text, cx));
        }))
    })
}

fn option_combo(
    theme: Theme,
    weak: WeakEntity<UserEditor>,
    options: Vec<ComboOption>,
    field: ComboField,
    cx: &mut Context<'_, UserEditor>,
) -> Entity<ComboBox> {
    cx.new(move |cx| {
        ComboBox::new(theme, options, String::new(), USER_FIELD_WIDTH, cx)
            .field_width(USER_FIELD_WIDTH)
            .on_select(Rc::new(move |value, _window, cx| {
                let _ = weak.update(cx, |view, cx| view.combo_selected(field, value, cx));
            }))
    })
}

impl UserEditor {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        id: u64,
        connection: Arc<dyn Connection>,
        connection_name: String,
        account: Option<(String, String)>,
        app: WeakEntity<AppView>,
        runtime: Arc<Runtime>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let plugins = connection.authentication_plugins();
        let ssl_types = connection.ssl_types();
        let weak = cx.weak_entity();

        let empty = UserAccount {
            plugin: plugins
                .first()
                .copied()
                .unwrap_or("caching_sha2_password")
                .to_string(),
            host: "%".to_string(),
            ..UserAccount::default()
        };

        let user_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::User,
            false,
            cx,
        );
        let host_input = text_input(
            theme,
            empty.host.clone(),
            weak.clone(),
            InputTag::Host,
            false,
            cx,
        );
        let password_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::Password,
            true,
            cx,
        );
        let confirm_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::Confirm,
            true,
            cx,
        );
        let expiry_days_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::ExpiryDays,
            false,
            cx,
        );
        let max_questions_input = text_input(
            theme,
            "0".to_string(),
            weak.clone(),
            InputTag::MaxQuestions,
            false,
            cx,
        );
        let max_updates_input = text_input(
            theme,
            "0".to_string(),
            weak.clone(),
            InputTag::MaxUpdates,
            false,
            cx,
        );
        let max_connections_input = text_input(
            theme,
            "0".to_string(),
            weak.clone(),
            InputTag::MaxConnections,
            false,
            cx,
        );
        let max_user_connections_input = text_input(
            theme,
            "0".to_string(),
            weak.clone(),
            InputTag::MaxUserConnections,
            false,
            cx,
        );
        let issuer_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::Issuer,
            false,
            cx,
        );
        let subject_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::Subject,
            false,
            cx,
        );
        let cipher_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::Cipher,
            false,
            cx,
        );

        let plugin_options = plugins
            .iter()
            .map(|plugin| ComboOption::plain(*plugin))
            .collect();
        let plugin_combo =
            option_combo(theme, weak.clone(), plugin_options, ComboField::Plugin, cx);
        let expiry_combo = option_combo(
            theme,
            weak.clone(),
            vec![
                ComboOption::new("default", t!("user.expiry.default").to_string()),
                ComboOption::new("never", t!("user.expiry.never").to_string()),
                ComboOption::new("interval", t!("user.expiry.interval").to_string()),
            ],
            ComboField::Expiry,
            cx,
        );
        let ssl_options = ssl_types
            .iter()
            .map(|ssl| {
                if ssl.is_empty() {
                    ComboOption::new("", t!("user.ssl.none").to_string())
                } else {
                    ComboOption::new(*ssl, *ssl)
                }
            })
            .collect();
        let ssl_combo = option_combo(theme, weak, ssl_options, ComboField::Ssl, cx);

        Self {
            app,
            runtime,
            theme,
            id,
            connection,
            connection_name,
            original: None,
            original_account: account,
            account: empty,
            password: String::new(),
            guard_password: String::new(),
            server_privileges: BTreeSet::new(),
            grants: Vec::new(),
            roles: BTreeMap::new(),
            members: BTreeMap::new(),
            accounts: Vec::new(),
            tab: UserTab::General,
            dirty: false,
            loading: false,
            saving: false,
            selected_grant: None,
            add_privilege: None,
            user_input,
            host_input,
            password_input,
            confirm_input,
            expiry_days_input,
            max_questions_input,
            max_updates_input,
            max_connections_input,
            max_user_connections_input,
            issuer_input,
            subject_input,
            cipher_input,
            plugin_combo,
            expiry_combo,
            ssl_combo,
            sql_scroll: ScrollHandle::new(),
        }
    }

    /// The account label shown by the tab strip: the original account, or "Untitled" for a new one.
    pub(super) fn title(&self) -> String {
        match &self.original_account {
            Some((user, host)) => format!("{user}@{host}"),
            None => t!("query.untitled").to_string(),
        }
    }

    /// The account being edited, or `None` for a new one. Used to de-duplicate open editors.
    pub(super) fn original_user(&self) -> Option<(String, String)> {
        self.original_account.clone()
    }

    /// Load the account's details and the server's account list (for the membership tabs).
    pub(super) fn load(&mut self, cx: &mut Context<'_, Self>) {
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        let account = self.original_account.clone();
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let joined = {
                let connection = connection.clone();
                runtime
                    .spawn(async move {
                        let details = match account {
                            Some((user, host)) => Some(connection.user_details(&user, &host).await),
                            None => None,
                        };
                        let accounts = connection.list_users().await;
                        (details, accounts)
                    })
                    .await
            };
            let (details, accounts) = match joined {
                Ok(pair) => pair,
                Err(error) => {
                    let _ = this.update(cx, |editor, cx| {
                        editor.loading = false;
                        editor.report_error(error.to_string(), cx);
                        cx.notify();
                    });
                    return;
                }
            };

            let _ = this.update(cx, |editor, cx| {
                editor.loading = false;
                if let Some(details) = details {
                    match details {
                        Ok(details) => editor.apply_details(details),
                        Err(error) => editor.report_error(error.to_string(), cx),
                    }
                }
                match accounts {
                    Ok(accounts) => editor.accounts = accounts,
                    Err(error) => editor.report_error(error.to_string(), cx),
                }
                editor.sync_controls(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_details(&mut self, details: UserDetails) {
        self.account = details.account.clone();
        self.server_privileges = details.server_privileges.clone();
        self.grants = details.grants.clone();
        self.roles = details
            .roles
            .iter()
            .map(|edge| {
                (
                    (edge.role_user.clone(), edge.role_host.clone()),
                    edge.admin_option,
                )
            })
            .collect();
        self.members = details
            .members
            .iter()
            .map(|edge| {
                (
                    (edge.member_user.clone(), edge.member_host.clone()),
                    edge.admin_option,
                )
            })
            .collect();
        self.original = Some(details);
        self.dirty = false;
        self.selected_grant = None;
    }

    /// Push the loaded account into the managed text fields and combos.
    fn sync_controls(&mut self, cx: &mut Context<'_, Self>) {
        let account = self.account.clone();
        self.user_input
            .update(cx, |input, cx| input.set_text(account.user.clone(), cx));
        self.host_input
            .update(cx, |input, cx| input.set_text(account.host.clone(), cx));
        self.max_questions_input.update(cx, |input, cx| {
            input.set_text(account.max_questions.to_string(), cx)
        });
        self.max_updates_input.update(cx, |input, cx| {
            input.set_text(account.max_updates.to_string(), cx)
        });
        self.max_connections_input.update(cx, |input, cx| {
            input.set_text(account.max_connections.to_string(), cx)
        });
        self.max_user_connections_input.update(cx, |input, cx| {
            input.set_text(account.max_user_connections.to_string(), cx)
        });
        self.issuer_input.update(cx, |input, cx| {
            input.set_text(account.x509_issuer.clone(), cx)
        });
        self.subject_input.update(cx, |input, cx| {
            input.set_text(account.x509_subject.clone(), cx)
        });
        self.cipher_input.update(cx, |input, cx| {
            input.set_text(account.ssl_cipher.clone(), cx)
        });
        let days = account.password_lifetime.unwrap_or(0);
        self.expiry_days_input.update(cx, |input, cx| {
            input.set_text(
                if days > 0 {
                    days.to_string()
                } else {
                    String::new()
                },
                cx,
            )
        });
        self.plugin_combo.update(cx, |combo, cx| {
            combo.set_selected(account.plugin.clone(), cx)
        });
        self.ssl_combo.update(cx, |combo, cx| {
            combo.set_selected(account.ssl_type.clone(), cx)
        });
        self.expiry_combo.update(cx, |combo, cx| {
            combo.set_selected(self.expiry_value().to_string(), cx)
        });
    }

    fn expiry_value(&self) -> &'static str {
        match self.account.password_lifetime {
            None => "default",
            Some(0) => "never",
            Some(_) => "interval",
        }
    }

    fn input_changed(&mut self, tag: InputTag, text: &str, cx: &mut Context<'_, Self>) {
        match tag {
            InputTag::User => self.account.user = text.to_string(),
            InputTag::Host => self.account.host = text.to_string(),
            InputTag::Password => self.password = text.to_string(),
            InputTag::Confirm => self.guard_password = text.to_string(),
            InputTag::ExpiryDays => {
                if let Ok(days) = text.trim().parse::<u32>() {
                    self.account.password_lifetime = Some(days);
                }
            }
            InputTag::MaxQuestions => self.account.max_questions = parse_count(text),
            InputTag::MaxUpdates => self.account.max_updates = parse_count(text),
            InputTag::MaxConnections => self.account.max_connections = parse_count(text),
            InputTag::MaxUserConnections => self.account.max_user_connections = parse_count(text),
            InputTag::Issuer => self.account.x509_issuer = text.to_string(),
            InputTag::Subject => self.account.x509_subject = text.to_string(),
            InputTag::Cipher => self.account.ssl_cipher = text.to_string(),
        }
        self.mark_dirty(cx);
    }

    fn combo_selected(&mut self, field: ComboField, value: &str, cx: &mut Context<'_, Self>) {
        match field {
            ComboField::Plugin => self.account.plugin = value.to_string(),
            ComboField::Ssl => self.account.ssl_type = value.to_string(),
            ComboField::Expiry => match value {
                "default" => self.account.password_lifetime = None,
                "never" => self.account.password_lifetime = Some(0),
                _ => {
                    let days = self
                        .account
                        .password_lifetime
                        .filter(|days| *days > 0)
                        .unwrap_or(30);
                    self.account.password_lifetime = Some(days);
                    self.expiry_days_input
                        .update(cx, |input, cx| input.set_text(days.to_string(), cx));
                }
            },
        }
        self.mark_dirty(cx);
    }

    fn mark_dirty(&mut self, cx: &mut Context<'_, Self>) {
        if !self.dirty {
            self.dirty = true;
        }
        cx.notify();
    }

    fn select_tab(&mut self, tab: UserTab, cx: &mut Context<'_, Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn toggle_account_locked(&mut self, cx: &mut Context<'_, Self>) {
        self.account.account_locked = !self.account.account_locked;
        self.mark_dirty(cx);
    }

    fn toggle_password_expired(&mut self, cx: &mut Context<'_, Self>) {
        self.account.password_expired = !self.account.password_expired;
        self.mark_dirty(cx);
    }

    fn toggle_server_privilege(&mut self, privilege: Privilege, cx: &mut Context<'_, Self>) {
        if !self.server_privileges.remove(&privilege) {
            self.server_privileges.insert(privilege);
        }
        self.mark_dirty(cx);
    }

    fn toggle_role(&mut self, key: (String, String), cx: &mut Context<'_, Self>) {
        if self.roles.remove(&key).is_none() {
            self.roles.insert(key, false);
        }
        self.mark_dirty(cx);
    }

    fn toggle_member(&mut self, key: (String, String), cx: &mut Context<'_, Self>) {
        if self.members.remove(&key).is_none() {
            self.members.insert(key, false);
        }
        self.mark_dirty(cx);
    }

    fn toggle_role_admin(&mut self, key: (String, String), cx: &mut Context<'_, Self>) {
        if let Some(admin) = self.roles.get_mut(&key) {
            *admin = !*admin;
            self.mark_dirty(cx);
        }
    }

    fn toggle_grant_privilege(
        &mut self,
        row: usize,
        privilege: Privilege,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(grant) = self.grants.get_mut(row) {
            if !grant.privileges.remove(&privilege) {
                grant.privileges.insert(privilege);
            }
            self.mark_dirty(cx);
        }
    }

    fn remove_selected_grant(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.selected_grant
            && index < self.grants.len()
        {
            self.grants.remove(index);
            self.selected_grant = if self.grants.is_empty() {
                None
            } else {
                Some(index.min(self.grants.len() - 1))
            };
            self.mark_dirty(cx);
        }
    }

    fn open_add_privilege(&mut self, cx: &mut Context<'_, Self>) {
        if self.add_privilege.is_some() {
            return;
        }
        self.add_privilege = Some(AddPrivilege::new());
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.list_databases().await })
                .await;
            let _ = this.update(cx, |editor, cx| {
                let Some(dialog) = editor.add_privilege.as_mut() else {
                    return;
                };
                match result {
                    Ok(Ok(databases)) => {
                        dialog.databases = databases.into_iter().map(|db| db.name).collect();
                        dialog.loading = false;
                    }
                    Ok(Err(error)) => {
                        dialog.error = Some(error.to_string());
                        dialog.loading = false;
                    }
                    Err(error) => {
                        dialog.error = Some(error.to_string());
                        dialog.loading = false;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn close_add_privilege(&mut self, cx: &mut Context<'_, Self>) {
        self.add_privilege = None;
        cx.notify();
    }

    /// Expand a database in the add-privilege tree, loading its tables the first time.
    fn toggle_add_database(&mut self, database: String, cx: &mut Context<'_, Self>) {
        let load = {
            let Some(dialog) = self.add_privilege.as_mut() else {
                return;
            };
            if dialog.expanded.as_deref() == Some(database.as_str()) {
                dialog.expanded = None;
                None
            } else {
                dialog.expanded = Some(database.clone());
                if dialog.tables.contains_key(&database) {
                    None
                } else {
                    Some(database)
                }
            }
        };
        let Some(database) = load else {
            cx.notify();
            return;
        };
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        let query = database.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.list_tables(&query).await })
                .await;
            let _ = this.update(cx, |editor, cx| {
                let Some(dialog) = editor.add_privilege.as_mut() else {
                    return;
                };
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

    /// Select a node of the add-privilege tree and mirror its current grant's check boxes.
    fn select_add_node(&mut self, node: (String, String), cx: &mut Context<'_, Self>) {
        let existing = self
            .grants
            .iter()
            .find(|grant| grant.database == node.0 && grant.name == node.1)
            .map(|grant| grant.privileges.clone())
            .unwrap_or_default();
        if let Some(dialog) = self.add_privilege.as_mut() {
            dialog.selected = Some(node);
            dialog.privileges = existing;
        }
        cx.notify();
    }

    fn toggle_add_privilege(&mut self, privilege: Privilege, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.add_privilege.as_mut()
            && !dialog.privileges.remove(&privilege)
        {
            dialog.privileges.insert(privilege);
        }
        cx.notify();
    }

    fn apply_add_privilege(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.add_privilege.take() else {
            return;
        };
        let Some((database, name)) = dialog.selected else {
            cx.notify();
            return;
        };
        if let Some(position) = self
            .grants
            .iter()
            .position(|grant| grant.database == database && grant.name == name)
        {
            if dialog.privileges.is_empty() {
                self.grants.remove(position);
            } else {
                self.grants[position].privileges = dialog.privileges;
            }
        } else if !dialog.privileges.is_empty() {
            self.grants.push(ObjectGrant {
                database,
                name,
                privileges: dialog.privileges,
            });
        }
        self.selected_grant = Some(self.grants.len().saturating_sub(1));
        self.mark_dirty(cx);
    }

    pub(super) fn set_theme(&mut self, theme: Theme, cx: &mut Context<'_, Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        for input in [
            &self.user_input,
            &self.host_input,
            &self.password_input,
            &self.confirm_input,
            &self.expiry_days_input,
            &self.max_questions_input,
            &self.max_updates_input,
            &self.max_connections_input,
            &self.max_user_connections_input,
            &self.issuer_input,
            &self.subject_input,
            &self.cipher_input,
        ] {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        for combo in [&self.plugin_combo, &self.expiry_combo, &self.ssl_combo] {
            combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
        }
        cx.notify();
    }

    fn report_error(&self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.error_dialog = Some(message);
                cx.notify();
            });
        }
    }

    fn build_edit(&self) -> UserEdit {
        UserEdit {
            original: self.original.clone(),
            account: self.account.clone(),
            password: if self.password.is_empty() {
                None
            } else {
                Some(self.password.clone())
            },
            server_privileges: self.server_privileges.clone(),
            grants: self.grants.clone(),
            roles: self
                .roles
                .iter()
                .map(|((user, host), admin)| (user.clone(), host.clone(), *admin))
                .collect(),
            members: self
                .members
                .iter()
                .map(|((user, host), admin)| (user.clone(), host.clone(), *admin))
                .collect(),
        }
    }

    fn preview_sql(&self) -> String {
        self.connection.user_edit_sql(&self.build_edit())
    }

    pub(super) fn save(&mut self, cx: &mut Context<'_, Self>) {
        if self.saving {
            return;
        }
        if self.account.user.trim().is_empty() {
            self.report_error(t!("user.name_required").to_string(), cx);
            return;
        }
        if self.password != self.guard_password {
            self.report_error(t!("user.password_mismatch").to_string(), cx);
            return;
        }
        let edit = self.build_edit();
        self.saving = true;
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        let is_new = self.original.is_none();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.save_user(&edit).await })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.saving = false;
                match result {
                    Ok(Ok(())) => {
                        editor.password.clear();
                        editor.guard_password.clear();
                        editor
                            .confirm_input
                            .update(cx, |input, cx| input.set_text("", cx));
                        editor
                            .password_input
                            .update(cx, |input, cx| input.set_text("", cx));
                        // Adopt the saved identity so the tab title and a re-save target it.
                        editor.original_account =
                            Some((editor.account.user.clone(), editor.account.host.clone()));
                        editor.reload_after_save(is_new, cx);
                    }
                    Ok(Err(error)) => editor.report_error(error.to_string(), cx),
                    Err(error) => editor.report_error(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Reload the saved account's details and refresh the app's Users list.
    fn reload_after_save(&mut self, was_new: bool, cx: &mut Context<'_, Self>) {
        self.dirty = false;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| app.refresh_users(cx));
        }
        if was_new {
            // A newly created account now exists; reload so later edits diff against it.
            self.load(cx);
        } else {
            self.load(cx);
        }
    }
}

fn parse_count(text: &str) -> u64 {
    text.trim().parse::<u64>().unwrap_or(0)
}

fn field_row(label: String, control: impl IntoElement, theme: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .min_h(px(26.0))
        .child(
            div()
                .w(px(USER_LABEL_WIDTH))
                .flex_none()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(label),
        )
        .child(control)
}

fn sized_field(entity: Entity<TextInput>) -> Div {
    div()
        .w(px(USER_FIELD_WIDTH))
        .h(px(22.0))
        .flex_none()
        .child(entity)
}

fn sized_combo(entity: Entity<ComboBox>) -> Div {
    div()
        .w(px(USER_FIELD_WIDTH))
        .h(px(20.0))
        .flex_none()
        .child(entity)
}

impl Render for UserEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.editor_bg))
            .text_size(px(12.0))
            .child(self.render_toolbar(cx))
            .child(self.render_subtabs(cx))
            .child(self.render_body(cx));
        if self.add_privilege.is_some() {
            root = root.child(self.render_add_privilege(cx));
        }
        root
    }
}

impl UserEditor {
    fn render_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(ui::toolbar_item(
                "user-save",
                "icons/save.svg",
                t!("design.save").to_string(),
                !self.loading && !self.saving,
                theme,
                cx.listener(|this, _event, _window, cx| this.save(cx)),
            ))
            .when(self.loading, |bar| {
                bar.child(
                    div()
                        .text_color(rgb(theme.text_muted))
                        .child(t!("design.loading").to_string()),
                )
            })
            .when(self.saving, |bar| {
                bar.child(
                    div()
                        .text_color(rgb(theme.text_muted))
                        .child(t!("common.saving").to_string()),
                )
            })
    }

    fn render_subtabs(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .h(px(26.0))
            .flex_none()
            .px_2()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border));
        for tab in UserTab::ALL {
            let active = self.tab == tab;
            bar = bar.child(
                div()
                    .id(tab.id())
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_3()
                    .h(px(24.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.editor_bg))
                            .border_t_1()
                            .border_l_1()
                            .border_r_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text))
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb(px(-1.0))
                    })
                    .when(!active, move |style| {
                        style
                            .bg(rgb(theme.button_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text_muted))
                            .hover(move |style| style.text_color(rgb(theme.text)))
                    })
                    .on_click(
                        cx.listener(move |this, _event, _window, cx| this.select_tab(tab, cx)),
                    )
                    .child(t!(tab.label_key()).to_string()),
            );
        }
        bar
    }

    fn render_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let content: AnyElement =
            if self.loading && self.original.is_none() && self.original_account.is_some() {
                div()
                    .p_3()
                    .text_color(rgb(self.theme.text_muted))
                    .child(t!("design.loading").to_string())
                    .into_any_element()
            } else {
                match self.tab {
                    UserTab::General => self.render_general(),
                    UserTab::Advanced => self.render_advanced(cx),
                    UserTab::MemberOf => self.render_membership(true, cx),
                    UserTab::Members => self.render_membership(false, cx),
                    UserTab::ServerPrivileges => self.render_server_privileges(cx),
                    UserTab::Privileges => self.render_privileges(cx),
                    UserTab::Sql => self.render_sql(),
                }
            };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(content)
            .into_any_element()
    }

    fn render_general(&self) -> AnyElement {
        let theme = self.theme;
        let content = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(field_row(
                t!("user.field.username").to_string(),
                sized_field(self.user_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.host").to_string(),
                sized_field(self.host_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.plugin").to_string(),
                sized_combo(self.plugin_combo.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.password").to_string(),
                sized_field(self.password_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.confirm_password").to_string(),
                sized_field(self.confirm_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.password_expiry").to_string(),
                sized_combo(self.expiry_combo.clone()),
                theme,
            ))
            .child(field_row(
                String::new(),
                div()
                    .w(px(USER_FIELD_WIDTH))
                    .h(px(22.0))
                    .child(self.expiry_days_input.clone()),
                theme,
            ));
        div()
            .id("user-general-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(content)
            .into_any_element()
    }

    fn render_advanced(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let locked = self.account.account_locked;
        let expired = self.account.password_expired;
        let content = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(field_row(
                t!("user.field.max_questions").to_string(),
                sized_field(self.max_questions_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.max_updates").to_string(),
                sized_field(self.max_updates_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.max_connections").to_string(),
                sized_field(self.max_connections_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.max_user_connections").to_string(),
                sized_field(self.max_user_connections_input.clone()),
                theme,
            ))
            .child(field_row(
                t!("user.field.account_locked").to_string(),
                div()
                    .id("user-account-locked")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.toggle_account_locked(cx)),
                    )
                    .child(self.check_row(locked, t!("user.field.locked").to_string(), theme)),
                theme,
            ))
            .child(field_row(
                t!("user.field.password_expired").to_string(),
                div()
                    .id("user-password-expired")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.toggle_password_expired(cx)),
                    )
                    .child(self.check_row(
                        expired,
                        t!("user.field.must_change").to_string(),
                        theme,
                    )),
                theme,
            ))
            .child(
                div()
                    .mt_2()
                    .p_2()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .child(
                        div()
                            .mb_2()
                            .text_color(rgb(theme.text))
                            .child(t!("user.ssl.section").to_string()),
                    )
                    .child(field_row(
                        t!("user.field.ssl_type").to_string(),
                        sized_combo(self.ssl_combo.clone()),
                        theme,
                    ))
                    .child(field_row(
                        t!("user.field.ssl_issuer").to_string(),
                        sized_field(self.issuer_input.clone()),
                        theme,
                    ))
                    .child(field_row(
                        t!("user.field.ssl_subject").to_string(),
                        sized_field(self.subject_input.clone()),
                        theme,
                    ))
                    .child(field_row(
                        t!("user.field.ssl_cipher").to_string(),
                        sized_field(self.cipher_input.clone()),
                        theme,
                    )),
            );
        div()
            .id("user-advanced-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(content)
            .into_any_element()
    }

    fn check_row(&self, checked: bool, label: String, theme: Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(checkbox_box(checked, theme))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .child(label),
            )
    }

    /// The 成员属于 (this account's roles) and 成员 (this account's members) tabs.
    fn render_membership(&self, member_of: bool, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let selected = if member_of {
            &self.roles
        } else {
            &self.members
        };
        let candidates: Vec<(String, String)> = self
            .accounts
            .iter()
            .map(|account| (account.user.clone(), account.host.clone()))
            .collect();

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
            .text_size(px(12.0))
            .child(div().flex_1().child(t!("user.member.username").to_string()))
            .child(
                div()
                    .w(px(70.0))
                    .child(t!("user.member.granted").to_string()),
            )
            .child(div().w(px(70.0)).child(t!("user.member.admin").to_string()));

        let mut rows = div().flex().flex_col();
        for (index, (user, host)) in candidates.iter().enumerate() {
            let key = (user.clone(), host.clone());
            let checked = selected.contains_key(&key);
            let admin = selected.get(&key).copied().unwrap_or(false);
            let row_key = key.clone();
            let admin_key = key.clone();
            let label = format!("{user}@{host}");
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("user-member-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(USER_ROW_HEIGHT))
                    .px_2()
                    .when(index % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                    .child(
                        div()
                            .id(SharedString::from(format!("user-member-name-{index}")))
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                if member_of {
                                    this.toggle_role(row_key.clone(), cx);
                                } else {
                                    this.toggle_member(row_key.clone(), cx);
                                }
                            }))
                            .child(tree_icon("icons/user.svg", theme.icon_users))
                            .child(label),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("user-member-granted-{index}")))
                            .w(px(70.0))
                            .flex()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                if member_of {
                                    this.toggle_role(key.clone(), cx);
                                } else {
                                    this.toggle_member(key.clone(), cx);
                                }
                            }))
                            .child(checkbox_box(checked, theme)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("user-member-admin-{index}")))
                            .w(px(70.0))
                            .flex()
                            .justify_center()
                            .when(member_of && checked, |cell| {
                                cell.cursor_pointer().on_click(cx.listener(
                                    move |this, _event, _window, cx| {
                                        this.toggle_role_admin(admin_key.clone(), cx)
                                    },
                                ))
                            })
                            .child(checkbox_box(admin, theme)),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(header)
            .child(
                div()
                    .id("user-members-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(rows),
            )
            .into_any_element()
    }

    fn render_server_privileges(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
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
            .text_size(px(12.0))
            .child(div().flex_1().child(t!("user.priv.name").to_string()))
            .child(div().w(px(70.0)).child(t!("user.priv.granted").to_string()));

        let mut rows = div().flex().flex_col();
        for (index, privilege) in Privilege::ALL.into_iter().enumerate() {
            let checked = self.server_privileges.contains(&privilege);
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("user-server-priv-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(USER_ROW_HEIGHT))
                    .px_2()
                    .cursor_pointer()
                    .when(index % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_server_privilege(privilege, cx)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .child(t!(privilege.label_key()).to_string()),
                    )
                    .child(
                        div()
                            .w(px(70.0))
                            .flex()
                            .justify_center()
                            .child(checkbox_box(checked, theme)),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(header)
            .child(
                div()
                    .id("user-server-priv-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(rows),
            )
            .into_any_element()
    }

    fn render_privileges(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let database_width = 140.0;
        let name_width = 160.0;
        let priv_width = 74.0;
        let content_width =
            database_width + name_width + priv_width * Privilege::OBJECT.len() as f32;

        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(ui::toolbar_item(
                "user-add-privilege",
                "icons/add_field.svg",
                t!("user.privilege.add").to_string(),
                true,
                theme,
                cx.listener(|this, _event, _window, cx| this.open_add_privilege(cx)),
            ))
            .child(
                ui::toolbar_item(
                    "user-remove-privilege",
                    "icons/delete_field.svg",
                    t!("user.privilege.remove").to_string(),
                    self.selected_grant.is_some(),
                    theme,
                    cx.listener(|this, _event, _window, cx| this.remove_selected_grant(cx)),
                )
                .into_any_element(),
            );

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
                    .w(px(database_width))
                    .flex_none()
                    .px_2()
                    .child(t!("user.privilege.database").to_string()),
            )
            .child(
                div()
                    .w(px(name_width))
                    .flex_none()
                    .px_2()
                    .child(t!("user.privilege.object").to_string()),
            );
        for privilege in Privilege::OBJECT {
            header = header.child(
                div()
                    .w(px(priv_width))
                    .flex_none()
                    .px_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(t!(privilege.label_key()).to_string()),
            );
        }

        let grants = self.grants.clone();
        let selected_grant = self.selected_grant;
        let mut rows = div().flex().flex_col();
        for (row, grant) in grants.iter().enumerate() {
            let is_selected = selected_grant == Some(row);
            let mut line = div()
                .id(SharedString::from(format!("user-grant-{row}")))
                .flex()
                .flex_row()
                .items_center()
                .h(px(USER_ROW_HEIGHT))
                .when(row % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                .when(is_selected, move |style| {
                    style.bg(rgb(theme.tree_selected_bg))
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.selected_grant = Some(row);
                    cx.notify();
                }))
                .child(
                    div()
                        .w(px(database_width))
                        .flex_none()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(grant.database.clone()),
                )
                .child(
                    div()
                        .w(px(name_width))
                        .flex_none()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(if grant.name.is_empty() {
                            t!("user.privilege.all_database").to_string()
                        } else {
                            grant.name.clone()
                        }),
                );
            for privilege in Privilege::OBJECT {
                let checked = grant.privileges.contains(&privilege);
                line = line.child(
                    div()
                        .id(SharedString::from(format!(
                            "user-grant-{row}-{}",
                            privilege.sql_name()
                        )))
                        .w(px(priv_width))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_grant_privilege(row, privilege, cx);
                        }))
                        .child(checkbox_box(checked, theme)),
                );
            }
            rows = rows.child(line);
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(toolbar)
            .child(
                div()
                    .id("user-grant-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_x_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(content_width))
                            .child(header)
                            .child(
                                div()
                                    .id("user-grant-rows")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .overflow_y_scroll()
                                    .child(rows),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_sql(&self) -> AnyElement {
        let theme = self.theme;
        let sql = self.preview_sql();
        let text = if sql.trim().is_empty() {
            t!("design.no_changes").to_string()
        } else {
            sql
        };
        div()
            .id("user-sql-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.sql_scroll)
            .p_2()
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(theme.text))
                    .child(text),
            )
            .into_any_element()
    }

    /// The "add privilege" popup: a database/table tree on the left and the privilege check boxes
    /// for the selected node on the right.
    fn render_add_privilege(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        if self.add_privilege.is_none() {
            return div().into_any_element();
        }
        let scrim = div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(theme.overlay))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.close_add_privilege(cx)),
            )
            .child(
                div()
                    .id("user-add-privilege-dialog")
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation()
                    })
                    .w(px(660.0))
                    .h(px(440.0))
                    .flex()
                    .flex_col()
                    .bg(rgb(theme.dialog_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .shadow(ui::dialog_shadow())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(30.0))
                            .px_3()
                            .flex_none()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .child(tree_icon("icons/gear.svg", theme.icon_users))
                            .child(
                                div()
                                    .text_color(rgb(theme.text))
                                    .child(t!("user.privilege.add_title").to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h(px(0.0))
                            .child(self.render_add_tree(cx))
                            .child(self.render_add_privilege_list(cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .h(px(38.0))
                            .px_3()
                            .flex_none()
                            .border_t_1()
                            .border_color(rgb(theme.border))
                            .child(ui::dialog_button(
                                "user-add-privilege-cancel",
                                t!("form.cancel").to_string(),
                                false,
                                theme,
                                cx.listener(|this, _event, _window, cx| {
                                    this.close_add_privilege(cx)
                                }),
                            ))
                            .child(ui::dialog_button(
                                "user-add-privilege-ok",
                                t!("form.ok").to_string(),
                                true,
                                theme,
                                cx.listener(|this, _event, _window, cx| {
                                    this.apply_add_privilege(cx)
                                }),
                            )),
                    ),
            );
        scrim.into_any_element()
    }

    fn render_add_tree(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.add_privilege.as_ref() else {
            return div().into_any_element();
        };
        let mut list = div().flex().flex_col();
        if dialog.loading {
            list = list.child(tree_message(
                t!("common.loading").to_string(),
                8.0,
                theme.text_muted,
            ));
        }
        if let Some(error) = dialog.error.as_ref() {
            list = list.child(tree_message(error.clone(), 8.0, theme.danger));
        }
        for (index, database) in dialog.databases.iter().enumerate() {
            let expanded = dialog.expanded.as_deref() == Some(database.as_str());
            let selected = dialog
                .selected
                .as_ref()
                .is_some_and(|(db, name)| db == database && name.is_empty());
            let db = database.clone();
            let db_select = database.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("user-add-db-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(USER_ROW_HEIGHT))
                    .cursor_pointer()
                    .when(selected, move |style| style.bg(rgb(theme.tree_selected_bg)))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(
                        div()
                            .id(SharedString::from(format!("user-add-db-toggle-{index}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(16.0))
                            .h_full()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_add_database(db.clone(), cx)
                            }))
                            .child(tree_chevron(expanded, theme.chevron)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("user-add-db-select-{index}")))
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.select_add_node((db_select.clone(), String::new()), cx)
                            }))
                            .child(tree_icon("icons/database.svg", theme.icon_database_active))
                            .child(database.clone()),
                    ),
            );
            if expanded {
                let tables = dialog.tables.get(database).cloned().unwrap_or_default();
                for (table_index, table) in tables.iter().enumerate() {
                    let selected = dialog
                        .selected
                        .as_ref()
                        .is_some_and(|(db, name)| db == database && name == table);
                    let node = (database.clone(), table.clone());
                    list = list.child(
                        div()
                            .id(SharedString::from(format!(
                                "user-add-table-{index}-{table_index}"
                            )))
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(USER_ROW_HEIGHT))
                            .pl(px(28.0))
                            .cursor_pointer()
                            .when(selected, move |style| style.bg(rgb(theme.tree_selected_bg)))
                            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.select_add_node(node.clone(), cx)
                            }))
                            .child(tree_icon("icons/tables.svg", theme.icon_tables))
                            .child(table.clone()),
                    );
                }
            }
        }

        div()
            .id("user-add-tree")
            .flex()
            .flex_col()
            .w(px(260.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(list)
            .into_any_element()
    }

    fn render_add_privilege_list(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.add_privilege.as_ref() else {
            return div().into_any_element();
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
            .text_size(px(12.0))
            .child(div().flex_1().child(t!("user.priv.name").to_string()))
            .child(div().w(px(60.0)).child(t!("user.priv.granted").to_string()));

        let mut rows = div().flex().flex_col();
        for (index, privilege) in Privilege::OBJECT.into_iter().enumerate() {
            let checked = dialog.privileges.contains(&privilege);
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("user-add-priv-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(USER_ROW_HEIGHT))
                    .px_2()
                    .cursor_pointer()
                    .when(index % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_add_privilege(privilege, cx)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .child(t!(privilege.label_key()).to_string()),
                    )
                    .child(
                        div()
                            .w(px(60.0))
                            .flex()
                            .justify_center()
                            .child(checkbox_box(checked, theme)),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_hidden()
            .child(header)
            .child(
                div()
                    .id("user-add-priv-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(rows),
            )
            .into_any_element()
    }
}

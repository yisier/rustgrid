//! The "New User" window (Navicat's 新建用户): the guided create flow opened by the Users
//! toolbar's New button.
//!
//! The layout follows Navicat's: the two account presets (普通用户 / 管理用户), the identity
//! fields (username, host, plugin, password, confirm, password-expiry policy), and a
//! per-database privilege section. The privilege section is master-detail — the database list on
//! the left selects the database being configured, the card above it picks that database's
//! privilege level and table scope, and a table picker appears beside the list for 指定表.
//!
//! Like the Export/Import wizards and the Backup window, this is a **separate OS window**, not a
//! `Root` modal: `AppView` owns the state ([`UserCreateDialog`]) and this module renders it, so
//! `AppView::sync_dialog` is not involved. Saving hands the resulting [`UserEdit`] to
//! `Connection::save_user` and then opens the account in the full user editor for further tuning.

use std::collections::BTreeSet;

use super::*;

/// Height of one row in the identity form and the database/table lists.
const CREATE_ROW_HEIGHT: f32 = 26.0;
/// Left inset of the identity form, so its labels line up under the preset card's text.
const CREATE_FORM_INDENT: f32 = 40.0;
/// Width of the identity form's label column; every label ends at the same x, which is what makes
/// the control column left-aligned.
const CREATE_LABEL_WIDTH: f32 = 160.0;
/// Gap between a label and its control.
const CREATE_LABEL_GAP: f32 = 16.0;
/// Width of the identity form's control column.
const CREATE_FIELD_WIDTH: f32 = 300.0;
/// Fixed width of the privilege-level dropdown.
const CREATE_LEVEL_WIDTH: f32 = 116.0;
/// Fixed width of the database list while the table picker is shown beside it.
const CREATE_DB_LIST_WIDTH: f32 = 236.0;
/// Height of the database / table list panes.
const CREATE_LIST_HEIGHT: f32 = 220.0;

/// The window's account preset, drawn as the two cards at the top.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateKind {
    /// A regular account: only the databases and privileges picked below.
    Regular,
    /// An instance-level administrator: server privileges plus the standard admin object grants.
    Admin,
}

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

/// One database row of the window's privilege matrix.
pub(super) struct CreateDatabaseRow {
    pub(super) name: String,
    /// Whether the database is granted to the account.
    pub(super) selected: bool,
    /// The privilege level applied to the database (or to its picked tables).
    pub(super) level: CreateLevel,
    /// Whether the grant covers the picked tables (指定表) rather than the whole database (全部表).
    pub(super) specific: bool,
    /// The tables picked for the 指定表 scope.
    pub(super) tables: Vec<String>,
}

/// The canned privilege set applied to one database by the window's level dropdown. The window is
/// a quick-start tool: the user editor's 权限 tab is where individual privileges are ticked.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateLevel {
    /// Read-only access (SELECT, SHOW VIEW).
    ReadOnly,
    /// Read/write access.
    ReadWrite,
    /// Read/write plus the data-definition privileges (CREATE/ALTER/DROP/INDEX/CREATE VIEW).
    Developer,
    /// Every object-level privilege, including GRANT OPTION.
    All,
}

impl CreateLevel {
    pub(super) const ALL: [CreateLevel; 4] = [
        CreateLevel::ReadOnly,
        CreateLevel::ReadWrite,
        CreateLevel::Developer,
        CreateLevel::All,
    ];

    pub(super) fn label_key(self) -> &'static str {
        match self {
            CreateLevel::ReadOnly => "user.create.level.read_only",
            CreateLevel::ReadWrite => "user.create.level.read_write",
            CreateLevel::Developer => "user.create.level.developer",
            CreateLevel::All => "user.create.level.all",
        }
    }

    /// The privileges this level grants.
    pub(super) fn privileges(self) -> BTreeSet<Privilege> {
        let mut set = BTreeSet::new();
        match self {
            CreateLevel::ReadOnly => {
                set.insert(Privilege::Select);
                set.insert(Privilege::ShowView);
            }
            CreateLevel::ReadWrite => {
                set.extend([
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                ]);
                set.insert(Privilege::ShowView);
            }
            CreateLevel::Developer => {
                set.extend([
                    Privilege::Select,
                    Privilege::Insert,
                    Privilege::Update,
                    Privilege::Delete,
                    Privilege::Create,
                    Privilege::Alter,
                    Privilege::Drop,
                    Privilege::Index,
                    Privilege::CreateView,
                    Privilege::ShowView,
                ]);
            }
            // `Privilege::OBJECT` already includes GRANT OPTION.
            CreateLevel::All => set.extend(Privilege::OBJECT),
        }
        set
    }
}

/// Which view of the account the window shows.
///
/// The two are the *same* account described two ways: 快速视图 is Navicat's guided form (presets
/// plus the per-database privilege matrix) and 完整视图 is the full account designer (attributes,
/// roles and the individual privilege matrix). Both edit [`UserCreateDialog::editor`], so
/// switching views never loses or re-reads anything, and Save is one path for both.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateView {
    /// The guided form, with the account presets.
    Quick,
    /// The full account designer.
    Full,
}

/// State of the "New User" window.
pub(super) struct UserCreateDialog {
    /// The connection the account is created on.
    pub(super) connection_index: usize,
    pub(super) connection_name: String,
    /// Which view is showing.
    pub(super) view: CreateView,
    /// The account being described. Shared by both views and owned here, so the window is the
    /// single source of truth for the account (the full view is rendered from it directly).
    pub(super) editor: UserEditorState,
    pub(super) kind: CreateKind,
    pub(super) expiry: CreateExpiry,
    pub(super) expiry_days: u32,
    /// The databases of the connection; picked up when the window opens.
    pub(super) databases: Vec<CreateDatabaseRow>,
    /// Every database's tables, loaded lazily per database for the 指定表 picker.
    pub(super) tables: BTreeMap<String, Vec<String>>,
    pub(super) loading_databases: bool,
    /// The databases whose tables are currently being fetched.
    pub(super) loading_tables: BTreeSet<String>,
    /// The database currently being configured in the detail card (index into `databases`).
    pub(super) active_database: Option<usize>,
    /// The 搜索数据库 filter text.
    pub(super) database_search: String,
    /// The 搜索表 filter text of the table picker.
    pub(super) table_search: String,
    /// Whether the 指定表 table picker is expanded beside the database list.
    pub(super) tables_open: bool,
    /// The 预览 SQL pane at the bottom of the window.
    pub(super) preview_open: bool,
    pub(super) preview_scroll: ScrollHandle,
    /// The account's role/member edges, loaded when the full view is first opened.
    pub(super) context: Option<UserAccountContext>,
    /// The membership candidate list (deferred, like its combo box).
    pub(super) account_menu: Option<AccountMenu>,
    /// The window's inline validation error.
    pub(super) error: Option<String>,
    pub(super) saving: bool,
}

impl UserCreateDialog {
    /// A fresh window state for a connection supporting `plugins`. `account` is `Some((user,
    /// host))` when editing an existing account, `None` when creating one.
    pub(super) fn new(
        connection_index: usize,
        connection_name: String,
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
        let mut editor = UserEditorState::new(plugin.clone());
        editor.account.user = user;
        editor.account.host = host;
        let context = account.map(|(user, host)| UserAccountContext {
            user,
            host,
            roles: BTreeMap::new(),
            members: BTreeMap::new(),
        });
        Self {
            connection_index,
            connection_name,
            view: CreateView::Quick,
            editor,
            kind: CreateKind::Regular,
            expiry: CreateExpiry::Default,
            expiry_days: 30,
            databases: Vec::new(),
            tables: BTreeMap::new(),
            loading_databases: true,
            loading_tables: BTreeSet::new(),
            active_database: None,
            database_search: String::new(),
            table_search: String::new(),
            tables_open: false,
            preview_open: false,
            preview_scroll: ScrollHandle::new(),
            context,
            account_menu: None,
            error: None,
            saving: false,
        }
    }

    /// `(user, host)` of the account being edited, for the window title.
    pub(super) fn original_account(&self) -> Option<&(String, String)> {
        self.editor.original_account.as_ref()
    }

    /// Whether an existing account is open.
    pub(super) fn is_edit(&self) -> bool {
        self.editor.original_account.is_some()
    }

    /// Whether the guided (quick) view is showing.
    pub(super) fn is_quick(&self) -> bool {
        self.view == CreateView::Quick
    }

    /// The user/host pairs a membership tab can pick from.
    pub(super) fn membership_candidates(&self) -> Vec<(String, String)> {
        self.account_menu
            .as_ref()
            .map(|menu| menu.accounts.clone())
            .unwrap_or_default()
    }

    /// The number of databases granted, for the quick view's "已选 N 个数据库" badge.
    pub(super) fn selected_database_count(&self) -> usize {
        self.databases.iter().filter(|row| row.selected).count()
    }

    /// The row currently being configured in the detail card.
    fn active_row(&self) -> Option<&CreateDatabaseRow> {
        self.active_database
            .and_then(|index| self.databases.get(index))
    }
}

/// Which identity field a change came from, used by the window's text inputs.
#[derive(Clone, Copy)]
pub(super) enum CreateField {
    User,
    Host,
    Password,
    Confirm,
    ExpiryDays,
    DatabaseSearch,
    TableSearch,
}

/// Which dropdown a change came from.
#[derive(Clone, Copy)]
enum CreateCombo {
    Plugin,
    Expiry,
}

/// The account-level state both views edit: the attributes, the server privileges and the
/// individual object grants. The quick view projects a privilege matrix onto [`Self::grants`];
/// the full view edits the 权限 tab directly. Owning it once is what lets the two views be the
/// same account rather than two copies.
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

/// The role/member edges of the account being edited, loaded with the full view.
pub(super) struct UserAccountContext {
    pub(super) user: String,
    pub(super) host: String,
    /// Role edges where this account is the member: `(role user, role host) -> admin option`.
    pub(super) roles: BTreeMap<(String, String), bool>,
    /// Role edges where this account is the role: `(member user, member host) -> admin option`.
    pub(super) members: BTreeMap<(String, String), bool>,
}

/// The membership candidate list, held until the full view's 成员属于 / 成员 tabs exist to render
/// it (the gpui-kit combo state needs a `Window`).
pub(super) struct AccountMenu {
    pub(super) accounts: Vec<(String, String)>,
}

// ----- Grant building ------------------------------------------------------------------------

/// The object grants the window currently describes.
///
/// 管理用户 is instance-level: the server privileges are what matter, so the standard admin package
/// is granted on every database. 普通用户 grants only the checked databases — a whole-database
/// grant, or one grant per picked table for the 指定表 scope (falling back to the whole database
/// when nothing is picked, so Save is never a silent no-op).
fn create_object_grants(dialog: &UserCreateDialog) -> Vec<ObjectGrant> {
    if dialog.kind == CreateKind::Admin {
        return dialog
            .databases
            .iter()
            .map(|row| ObjectGrant {
                database: row.name.clone(),
                name: String::new(),
                privileges: CreateLevel::All.privileges(),
            })
            .collect();
    }
    let mut grants: Vec<ObjectGrant> = Vec::new();
    for row in &dialog.databases {
        if !row.selected {
            continue;
        }
        let privileges = row.level.privileges();
        if row.specific && !row.tables.is_empty() {
            for table in &row.tables {
                grants.push(ObjectGrant {
                    database: row.name.clone(),
                    name: table.clone(),
                    privileges: privileges.clone(),
                });
            }
        } else {
            grants.push(ObjectGrant {
                database: row.name.clone(),
                name: String::new(),
                privileges,
            });
        }
    }
    grants
}

// ----- Layout helpers ------------------------------------------------------------------------

/// A titled section of the full view: a heading (with an optional hint) above its content.
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
///
/// The label cell is `CREATE_LABEL_WIDTH` wide for **every** row, so all six controls start at the
/// same x and the column is left-aligned — a per-label width would stagger them.
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

/// A `CREATE_FIELD_WIDTH` text field (or its empty placeholder while the entity is missing).
fn sized_text(input: Option<&Entity<TextInput>>, theme: Theme) -> AnyElement {
    match input {
        Some(input) => div()
            .w(px(CREATE_FIELD_WIDTH))
            .h(px(24.0))
            .flex_none()
            .child(input.clone())
            .into_any_element(),
        None => div()
            .w(px(CREATE_FIELD_WIDTH))
            .h(px(24.0))
            .flex_none()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .into_any_element(),
    }
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

// ----- Window --------------------------------------------------------------------------------

/// The root view of the "New User" OS window. It re-renders whenever `AppView` changes, so the
/// preset cards, the privilege matrix and the SQL preview stay live while the user edits them.
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
    /// Open the "New User" window for the connection the Users tab is scoped to.
    pub(super) fn open_create_user(&mut self, cx: &mut Context<'_, Self>) {
        self.open_user_window(None, cx);
    }

    /// Open an existing account in the same window, starting in the full view.
    pub(super) fn open_edit_user(&mut self, account: (String, String), cx: &mut Context<'_, Self>) {
        self.open_user_window(Some(account), cx);
    }

    /// Open the account window — creating an account when `account` is `None`, editing it
    /// otherwise. Both flows share this bootstrap so there is exactly one place that builds the
    /// window state and the field entities.
    fn open_user_window(&mut self, account: Option<(String, String)>, cx: &mut Context<'_, Self>) {
        if self.create_user_dialog.is_some() {
            // Already open: raise it rather than ignoring the click, so the window is never left
            // hidden behind the main one with no way to get back to it.
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
        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
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
        self.create_user_db_search = Some(make_create_search_input(
            theme,
            t!("user.create.search_database").to_string(),
            CreateField::DatabaseSearch,
            &weak,
            cx,
        ));
        self.create_user_table_search = Some(make_create_search_input(
            theme,
            t!("user.create.search_table").to_string(),
            CreateField::TableSearch,
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

        let mut dialog =
            UserCreateDialog::new(connection_index, connection_name, plugin, account.clone());
        if editing {
            dialog.view = CreateView::Full;
        }
        self.create_user_dialog = Some(dialog);
        self.create_user_level_menu = None;
        if editing {
            self.load_edit_account(cx);
        } else {
            self.load_create_databases(cx);
        }
        self.open_create_user_window(cx);
        cx.notify();
    }

    /// Load the account being edited into the window, and the databases its privilege matrix
    /// shows.
    fn load_edit_account(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let Some((user, host)) = dialog.original_account().cloned() else {
            self.load_create_databases(cx);
            return;
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return;
        };
        let runtime = self.runtime.clone();
        let query_user = user.clone();
        let query_host = host.clone();
        cx.spawn(async move |this, cx| {
            let joined = {
                let connection = connection.clone();
                runtime
                    .spawn(async move {
                        let details = connection.user_details(&query_user, &query_host).await;
                        let accounts = connection.list_users().await;
                        (details, accounts)
                    })
                    .await
            };
            let (details, accounts) = match joined {
                Ok(pair) => pair,
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        if let Some(dialog) = app.create_user_dialog.as_mut() {
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
                match details {
                    Ok(details) => dialog.editor.apply_details(details),
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                if let Ok(accounts) = accounts {
                    dialog.account_menu = Some(AccountMenu {
                        accounts: accounts
                            .into_iter()
                            .map(|account| (account.user, account.host))
                            .collect(),
                    });
                }
                app.sync_create_editor_fields(cx);
                cx.notify();
            });
        })
        .detach();
        self.load_create_databases(cx);
    }

    /// Open the OS window hosting the "New User" dialog (mirrors the Export/Import windows).
    fn open_create_user_window(&mut self, cx: &mut Context<'_, Self>) {
        if self.create_user_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let title = t!("user.create.title").to_string();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(820.0), px(720.0)), cx);
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
                    // `open_window` does not raise what it opens, so without this the window can
                    // appear behind the main one and look like the toolbar button did nothing.
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

    /// Close the window and drop the entities it created. Called from the footer, where the
    /// window is at hand.
    pub(super) fn create_user_close(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.cancel_create_user(cx);
        window.remove_window();
    }

    /// Close the window and drop the entities it created.
    pub(super) fn cancel_create_user(&mut self, cx: &mut Context<'_, Self>) {
        self.create_user_dialog = None;
        self.create_user_user = None;
        self.create_user_host = None;
        self.create_user_password = None;
        self.create_user_confirm = None;
        self.create_user_plugin_combo = None;
        self.create_user_expiry_combo = None;
        self.create_user_expiry_days = None;
        self.create_user_db_search = None;
        self.create_user_table_search = None;
        self.create_user_level_menu = None;
        self.create_user_window = None;
        cx.notify();
    }

    /// Pick the account preset.
    pub(super) fn set_create_user_kind(&mut self, kind: CreateKind, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.kind = kind;
            dialog.error = None;
        }
        self.create_user_level_menu = None;
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
                CreateField::DatabaseSearch => dialog.database_search = text.to_string(),
                CreateField::TableSearch => dialog.table_search = text.to_string(),
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

    /// Load the connection's databases into the window's privilege matrix.
    pub(super) fn load_create_databases(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.loading_databases = false;
                dialog.error = Some(t!("info.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.loading_databases = true;
            dialog.error = None;
        }
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.list_databases().await })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                dialog.loading_databases = false;
                match result {
                    Ok(Ok(databases)) => {
                        dialog.databases = databases
                            .into_iter()
                            .map(|database| CreateDatabaseRow {
                                name: database.name,
                                selected: false,
                                level: CreateLevel::ReadOnly,
                                specific: false,
                                tables: Vec::new(),
                            })
                            .collect();
                        dialog.active_database = (!dialog.databases.is_empty()).then_some(0);
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

    /// Load one database's tables for the 指定表 picker the first time it is needed.
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

    /// Toggle the check box of one database row.
    pub(super) fn toggle_create_database(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(row) = dialog.databases.get_mut(index)
        {
            row.selected = !row.selected;
        }
        cx.notify();
    }

    /// Make one database the one shown in the detail card (the master-detail selection).
    pub(super) fn activate_create_database(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        {
            let Some(dialog) = self.create_user_dialog.as_mut() else {
                return;
            };
            if index >= dialog.databases.len() {
                return;
            }
            dialog.active_database = Some(index);
            dialog.table_search.clear();
            dialog.tables_open = dialog.databases[index].specific;
        }
        self.create_user_level_menu = None;
        if let Some(input) = self.create_user_table_search.clone() {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        let database = self.create_user_database_needing_tables();
        if let Some(database) = database {
            self.load_create_tables(database, cx);
        } else {
            cx.notify();
        }
    }

    /// The active database's name if its 指定表 scope needs its tables loaded.
    fn create_user_database_needing_tables(&self) -> Option<String> {
        let dialog = self.create_user_dialog.as_ref()?;
        let row = dialog.active_row()?;
        (row.specific).then(|| row.name.clone())
    }

    /// Set the active database's table scope (全部表 / 指定表).
    pub(super) fn set_create_scope(&mut self, specific: bool, cx: &mut Context<'_, Self>) {
        let database = {
            let Some(dialog) = self.create_user_dialog.as_mut() else {
                return;
            };
            let Some(index) = dialog.active_database else {
                return;
            };
            let Some(row) = dialog.databases.get_mut(index) else {
                return;
            };
            row.specific = specific;
            dialog.tables_open = specific;
            specific.then(|| row.name.clone())
        };
        self.create_user_level_menu = None;
        if let Some(database) = database {
            self.load_create_tables(database, cx);
        } else {
            cx.notify();
        }
    }

    /// Expand or collapse the 指定表 table picker.
    pub(super) fn toggle_create_tables_open(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.tables_open = !dialog.tables_open;
        }
        cx.notify();
    }

    /// Toggle one table in the active database's 指定表 list.
    pub(super) fn toggle_create_table(&mut self, name: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(index) = dialog.active_database
            && let Some(row) = dialog.databases.get_mut(index)
        {
            if let Some(position) = row.tables.iter().position(|table| *table == name) {
                row.tables.remove(position);
            } else {
                row.tables.push(name);
            }
        }
        cx.notify();
    }

    /// Select or clear every table of the active database.
    pub(super) fn set_create_tables_all(&mut self, all: bool, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_mut() else {
            return;
        };
        let Some(index) = dialog.active_database else {
            return;
        };
        let Some(row) = dialog.databases.get(index) else {
            return;
        };
        let database = row.name.clone();
        let tables = dialog.tables.get(&database).cloned().unwrap_or_default();
        if let Some(row) = dialog.databases.get_mut(index) {
            row.tables = if all { tables } else { Vec::new() };
        }
        cx.notify();
    }

    /// Pick a privilege level for the active database, closing the inline menu.
    pub(super) fn choose_create_level(&mut self, level: CreateLevel, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(index) = dialog.active_database
            && let Some(row) = dialog.databases.get_mut(index)
        {
            row.level = level;
            row.selected = true;
        }
        self.create_user_level_menu = None;
        cx.notify();
    }

    /// Show/hide the active database's inline privilege-level list.
    pub(super) fn toggle_create_level_menu(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let open = self.create_user_level_menu.as_ref().map(|menu| menu.index) == Some(index);
        self.create_user_level_menu = if open {
            None
        } else {
            Some(CreateLevelMenu { index })
        };
        cx.notify();
    }

    /// Show/hide the window's SQL preview pane.
    pub(super) fn toggle_create_preview(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.preview_open = !dialog.preview_open;
        }
        cx.notify();
    }

    /// Push the loaded account into the window's managed text fields, combos and the quick view.
    /// Called after loading or reloading the account.
    pub(super) fn sync_create_editor_fields(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let account = dialog.editor.account.clone();
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
        if let Some(input) = self.create_user_expiry_days.clone() {
            input.update(cx, |input, cx| input.set_text(expiry_days.to_string(), cx));
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
        let Some((user, host)) = dialog.original_account().cloned() else {
            return;
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
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
                match result {
                    Ok(Ok(details)) => {
                        dialog.editor.apply_details(details);
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

    /// Switch the window between the guided (quick) and full views.
    pub(super) fn set_create_view(&mut self, view: CreateView, cx: &mut Context<'_, Self>) {
        let load_membership = {
            let Some(dialog) = self.create_user_dialog.as_mut() else {
                return;
            };
            dialog.view = view;
            view == CreateView::Full && dialog.account_menu.is_none()
        };
        self.create_user_level_menu = None;
        if load_membership {
            self.load_create_accounts(cx);
        } else {
            cx.notify();
        }
    }

    /// Load the server's account list for the full view's 成员属于 / 成员 tabs.
    fn load_create_accounts(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.list_users().await })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.create_user_dialog.as_mut() else {
                    return;
                };
                match result {
                    Ok(Ok(accounts)) => {
                        dialog.account_menu = Some(AccountMenu {
                            accounts: accounts
                                .into_iter()
                                .map(|account| (account.user, account.host))
                                .collect(),
                        });
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

    /// Toggle one object privilege on the selected grant row.
    pub(super) fn toggle_create_grant_privilege(
        &mut self,
        row: usize,
        privilege: Privilege,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(grant) = dialog.editor.grants.get_mut(row)
            && !grant.privileges.remove(&privilege)
        {
            grant.privileges.insert(privilege);
        }
        cx.notify();
    }

    /// Toggle one server privilege in the full view's 服务器权限 tab.
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

    /// Toggle one role/member edge in the full view's membership tabs.
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

    /// Toggle the admin option of one role edge.
    pub(super) fn toggle_create_role_admin(
        &mut self,
        key: (String, String),
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.create_user_dialog.as_mut()
            && let Some(context) = dialog.context.as_mut()
            && let Some(admin) = context.roles.get_mut(&key)
        {
            *admin = !*admin;
        }
        cx.notify();
    }

    /// The edit the window currently describes. `None` while the window is closed.
    ///
    /// The account identity and the individual privilege surface come from the shared
    /// [`UserEditorState`], so both views build exactly one edit.
    pub(super) fn create_user_edit(&self) -> Option<UserEdit> {
        let dialog = self.create_user_dialog.as_ref()?;
        let is_admin = dialog.kind == CreateKind::Admin && dialog.is_quick();
        let server_privileges = if is_admin {
            Privilege::ALL.into_iter().collect()
        } else {
            dialog.editor.server_privileges.clone()
        };
        let grants = if dialog.is_quick() {
            // The guided view describes privileges with its matrix; the full view edits the
            // individual grants directly.
            create_object_grants(dialog)
        } else {
            dialog.editor.grants.clone()
        };
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
            password: if dialog.editor.password.is_empty() {
                None
            } else {
                Some(dialog.editor.password.clone())
            },
            server_privileges,
            grants,
            roles,
            members,
        })
    }

    /// The SQL the window would run, for the preview pane.
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

    /// Validate and save the account. Creating an account switches the window to the full view
    /// (no need to close and reopen); saving an existing one just reloads it in place.
    pub(super) fn submit_create_user(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return;
        };
        if dialog.saving {
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
        if dialog.editor.password != dialog.editor.guard_password {
            let message = t!("user.password_mismatch").to_string();
            if let Some(dialog) = self.create_user_dialog.as_mut() {
                dialog.error = Some(message);
            }
            cx.notify();
            return;
        }
        let Some(edit) = self.create_user_edit() else {
            return;
        };
        let connection_index = dialog.connection_index;
        let connection_name = dialog.connection_name.clone();
        let was_new = edit.original.is_none();
        if let Some(dialog) = self.create_user_dialog.as_mut() {
            dialog.saving = true;
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
                    Ok(Ok(())) => {}
                }
                if succeeded {
                    app.refresh_users(cx);
                    {
                        let Some(dialog) = app.create_user_dialog.as_mut() else {
                            return;
                        };
                        dialog.saving = false;
                        dialog.editor.password.clear();
                        dialog.editor.guard_password.clear();
                        // Adopt the saved identity so a re-save targets it, and so the window is
                        // now editing this account rather than creating one.
                        dialog.editor.original_account = Some((
                            dialog.editor.account.user.clone(),
                            dialog.editor.account.host.clone(),
                        ));
                        if was_new {
                            dialog.view = CreateView::Full;
                        }
                    }
                    if was_new {
                        // A newly created account now exists; load it so later edits diff
                        // against it.
                        app.load_edit_account(cx);
                    } else {
                        app.reload_create_account(cx);
                    }
                    let _ = connection_name;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

// ----- Window field entities -----------------------------------------------------------------

/// Build one identity field of the window. Edits write straight into the window state so the SQL
/// preview and the save path always see the latest text.
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
    /// The window's contents: the child titlebar, the view switcher, the body and the footer.
    pub(super) fn create_user_window_contents(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let title = match self
            .create_user_dialog
            .as_ref()
            .and_then(|dialog| dialog.original_account())
        {
            Some((user, _host)) => user.clone(),
            None => t!("user.create.title").to_string(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .child(export::child_window_titlebar(title, theme))
            .child(self.render_create_view_tabs(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .child(self.create_user_body(cx)),
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
            )
            .into_any_element()
    }

    /// The 快速视图 / 完整视图 switcher. Both views edit the same account, so switching is lossless.
    fn render_create_view_tabs(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .px_4()
            .py_2()
            .flex_none()
            .bg(rgb(theme.dialog_face))
            .border_b_1()
            .border_color(rgb(theme.border));
        for (view, label_key) in [
            (CreateView::Quick, "user.create.view.quick"),
            (CreateView::Full, "user.create.view.full"),
        ] {
            let active = dialog.view == view;
            bar =
                bar.child(
                    div()
                        .id(match view {
                            CreateView::Quick => "user-create-view-quick",
                            CreateView::Full => "user-create-view-full",
                        })
                        .flex_none()
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
                            this.set_create_view(view, cx)
                        }))
                        .child(t!(label_key).to_string()),
                );
        }
        bar.into_any_element()
    }

    /// The body of the current view.
    fn create_user_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        if dialog.is_quick() {
            self.create_user_quick_body(cx)
        } else {
            self.create_user_full_body(cx)
        }
    }

    /// The guided body: the presets, the identity fields, the privilege section and the optional
    /// SQL preview.
    fn create_user_quick_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let is_admin = dialog.kind == CreateKind::Admin;

        let mut body = div()
            .id("user-create-body")
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_4()
            .child(self.render_create_presets(cx))
            .child(self.render_create_identity());

        if is_admin {
            // 管理用户 is instance-level: the matrix is replaced by the preset's warning.
            body = body.child(self.render_create_admin_warning());
        } else {
            body = body.child(self.render_create_databases(cx));
        }

        if let Some(error) = dialog.error.as_ref() {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(error.clone()),
            );
        }

        if dialog.preview_open {
            body = body.child(self.render_create_preview());
        }

        body.into_any_element()
    }

    /// The full body: the identity attributes, the server privileges, the individual privilege
    /// matrix and the membership tabs — the same surface the old user-editor tab showed, now
    /// rendering the shared [`UserEditorState`].
    fn create_user_full_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let _ = dialog;

        div()
            .id("user-create-full-body")
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_4()
            .child(self.render_create_identity())
            .child(self.render_create_server_privileges(cx))
            .child(self.render_create_grants(cx))
            .child(self.render_create_memberships(cx))
            .into_any_element()
    }

    /// The 服务器权限 section of the full view.
    fn render_create_server_privileges(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let checked_count = dialog.editor.server_privileges.len();
        let mut grid = div().flex().flex_row().flex_wrap().w_full();
        for (index, privilege) in Privilege::ALL.into_iter().enumerate() {
            let checked = dialog.editor.server_privileges.contains(&privilege);
            grid = grid.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-server-priv-{index}"
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w(px(200.0))
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
        section(
            t!("user.tab.server_privileges").to_string(),
            Some(t!("user.create.server_privileges_hint", count = checked_count).to_string()),
            grid.into_any_element(),
            theme,
        )
        .into_any_element()
    }

    /// The 权限 section of the full view: the object grants with their privilege check boxes.
    fn render_create_grants(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let grants = dialog.editor.grants.clone();
        let database_width = 140.0;
        let name_width = 150.0;
        let priv_width = 62.0;

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

        let mut rows = div().flex().flex_col();
        if grants.is_empty() {
            rows = rows.child(tree_message(
                t!("user.privilege.empty").to_string(),
                8.0,
                theme.text_muted,
            ));
        }
        for (row, grant) in grants.iter().enumerate() {
            let mut line = div()
                .id(SharedString::from(format!("user-create-grant-{row}")))
                .flex()
                .flex_row()
                .items_center()
                .h(px(CREATE_ROW_HEIGHT))
                .when(row % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                .child(
                    div()
                        .w(px(database_width))
                        .flex_none()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .child(grant.database.clone()),
                )
                .child(
                    div()
                        .w(px(name_width))
                        .flex_none()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
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
                            "user-create-grant-{row}-{}",
                            privilege.sql_name()
                        )))
                        .w(px(priv_width))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_create_grant_privilege(row, privilege, cx);
                        }))
                        .child(checkbox_box(checked, theme)),
                );
            }
            rows = rows.child(line);
        }

        let content_width =
            database_width + name_width + priv_width * Privilege::OBJECT.len() as f32;
        let table = div()
            .id("user-create-grants-scroll")
            .flex()
            .flex_col()
            .w_full()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .overflow_hidden()
            .overflow_x_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(content_width))
                    .child(header)
                    .child(rows),
            );

        section(
            t!("user.tab.privileges").to_string(),
            Some(t!("user.create.grants_hint").to_string()),
            table.into_any_element(),
            theme,
        )
        .into_any_element()
    }

    /// The 成员属于 / 成员 section of the full view.
    fn render_create_memberships(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let Some(context) = dialog.context.as_ref() else {
            return div().into_any_element();
        };
        let candidates = dialog.membership_candidates();
        if candidates.is_empty() {
            return div().into_any_element();
        }
        let current = context.user.clone();
        let current_host = context.host.clone();

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
        for (index, (user, host)) in candidates.iter().enumerate() {
            // A role cannot be granted to itself.
            if *user == current && *host == current_host {
                continue;
            }
            let key = (user.clone(), host.clone());
            let member = context.roles.contains_key(&key);
            let admin = context.roles.get(&key).copied().unwrap_or(false);
            let label = format!("{user}@{host}");
            let click_key = key.clone();
            let admin_key = key.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("user-create-member-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(CREATE_ROW_HEIGHT))
                    .px_2()
                    .when(index % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-member-name-{index}"
                            )))
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_membership(true, click_key.clone(), cx)
                            }))
                            .child(tree_icon("icons/user.svg", theme.icon_users))
                            .child(div().text_size(px(12.0)).child(label)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-member-granted-{index}"
                            )))
                            .w(px(70.0))
                            .flex()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_membership(true, key.clone(), cx)
                            }))
                            .child(checkbox_box(member, theme)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "user-create-member-admin-{index}"
                            )))
                            .w(px(70.0))
                            .flex()
                            .justify_center()
                            .when(member, |cell| {
                                cell.cursor_pointer().on_click(cx.listener(
                                    move |this, _event, _window, cx| {
                                        this.toggle_create_role_admin(admin_key.clone(), cx)
                                    },
                                ))
                            })
                            .child(checkbox_box(admin, theme)),
                    ),
            );
        }

        section(
            t!("user.tab.member_of").to_string(),
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
                .child(
                    div()
                        .id("user-create-member-scroll")
                        .flex()
                        .flex_col()
                        .h(px(150.0))
                        .overflow_y_scroll()
                        .child(rows),
                )
                .into_any_element(),
            theme,
        )
        .into_any_element()
    }

    /// The two preset cards at the top of the window.
    fn render_create_presets(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let mut row = div().flex().flex_row().gap_3().w_full();
        for (kind, title_key, description_key) in [
            (
                CreateKind::Regular,
                "user.create.kind.regular",
                "user.create.kind.regular_desc",
            ),
            (
                CreateKind::Admin,
                "user.create.kind.admin",
                "user.create.kind.admin_desc",
            ),
        ] {
            let selected = dialog.kind == kind;
            row = row.child(
                div()
                    .id(match kind {
                        CreateKind::Admin => "user-create-kind-admin",
                        CreateKind::Regular => "user-create-kind-regular",
                    })
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.0))
                    .p_3()
                    .rounded(px(6.0))
                    .border_1()
                    .cursor_pointer()
                    .when(selected, move |style| {
                        style
                            .border_color(rgb(theme.primary))
                            .bg(rgb(theme.tree_hover_bg))
                    })
                    .when(!selected, move |style| {
                        style
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.input_bg))
                            .hover(move |style| {
                                style.border_color(rgb(theme.button_default_border))
                            })
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_create_user_kind(kind, cx)
                    }))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(theme.text))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!(title_key).to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(theme.text_muted))
                            .child(t!(description_key).to_string()),
                    ),
            );
        }
        row.into_any_element()
    }

    /// The identity fields: username, host, plugin, password, confirm and the expiry policy.
    fn render_create_identity(&self) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let mut column = div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .pl(px(CREATE_FORM_INDENT))
            .child(create_row(
                t!("user.field.username").to_string(),
                sized_text(self.create_user_user.as_ref(), theme),
                theme,
            ))
            .child(create_row(
                t!("user.field.host").to_string(),
                sized_text(self.create_user_host.as_ref(), theme),
                theme,
            ))
            .child(create_row(
                t!("user.field.plugin").to_string(),
                sized_combo(self.create_user_plugin_combo.as_ref(), theme),
                theme,
            ))
            .child(create_row(
                t!("user.field.password").to_string(),
                sized_text(self.create_user_password.as_ref(), theme),
                theme,
            ))
            .child(create_row(
                t!("user.field.confirm_password").to_string(),
                sized_text(self.create_user_confirm.as_ref(), theme),
                theme,
            ))
            .child(create_row(
                t!("user.field.password_expiry").to_string(),
                sized_combo(self.create_user_expiry_combo.as_ref(), theme),
                theme,
            ));
        if dialog.expiry == CreateExpiry::Interval {
            column = column.child(create_row(
                String::new(),
                sized_text(self.create_user_expiry_days.as_ref(), theme),
                theme,
            ));
        }
        column.into_any_element()
    }

    /// The regular preset's privilege section: the header, the database filter and the matrix.
    fn render_create_databases(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let selected = dialog.selected_database_count();

        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(theme.text))
                            .child(t!("user.create.databases").to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .py_0p5()
                            .rounded(px(999.0))
                            .bg(rgb(theme.header_bg))
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(
                                t!("user.create.selected_databases", count = selected).to_string(),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.create.databases_hint").to_string()),
            );

        let search = div().w_full().h(px(26.0)).flex_none().child(
            match self.create_user_db_search.clone() {
                Some(input) => input.into_any_element(),
                None => div().into_any_element(),
            },
        );

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(header)
            .child(search)
            .child(self.render_create_matrix(cx))
            .into_any_element()
    }

    /// The bordered master-detail matrix: the active database's card, its scope row and the lists.
    fn render_create_matrix(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let mut matrix = div()
            .flex()
            .flex_col()
            .w_full()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .overflow_hidden();

        if dialog.loading_databases {
            return matrix
                .child(tree_message(
                    t!("common.loading").to_string(),
                    12.0,
                    theme.text_muted,
                ))
                .into_any_element();
        }
        if dialog.databases.is_empty() {
            return matrix
                .child(tree_message(
                    t!("common.empty").to_string(),
                    12.0,
                    theme.text_muted,
                ))
                .into_any_element();
        }
        let Some(index) = dialog.active_database else {
            return matrix
                .child(tree_message(
                    t!("common.empty").to_string(),
                    12.0,
                    theme.text_muted,
                ))
                .into_any_element();
        };
        let Some(row) = dialog.databases.get(index) else {
            return matrix.into_any_element();
        };
        let level_open = self.create_user_level_menu.as_ref().map(|menu| menu.index) == Some(index);
        let specific = row.specific;

        // The active database's card.
        matrix = matrix.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(38.0))
                .px_3()
                .child(
                    div()
                        .id("user-create-active-check")
                        .flex()
                        .flex_row()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_create_database(index, cx)
                        }))
                        .child(checkbox_box(row.selected, theme)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .text_color(rgb(theme.text))
                        .child(row.name.clone()),
                )
                .child(self.render_create_level_button(index, level_open, cx)),
        );

        if level_open {
            matrix = matrix.child(self.render_create_level_options(index, cx));
        }

        // The 表范围 row: 全部表 / 指定表 plus the picked-tables chip.
        matrix = matrix.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(34.0))
                .px_3()
                .border_t_1()
                .border_color(rgb(theme.grid_line))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("user.create.scope").to_string()),
                )
                .child(self.create_scope_chip(
                    "user-create-scope-all",
                    t!("user.create.scope.all").to_string(),
                    !specific,
                    false,
                    cx,
                ))
                .child(self.create_scope_chip(
                    "user-create-scope-specific",
                    t!("user.create.scope.specific").to_string(),
                    specific,
                    true,
                    cx,
                ))
                .when(specific, |bar| {
                    bar.child(self.render_create_tables_chip(index, dialog.tables_open, cx))
                })
                .when(!specific, |bar| bar.child(div().flex_1())),
        );

        // The database list and (for 指定表) the table picker.
        let show_tables = specific && dialog.tables_open;
        let mut split = div()
            .flex()
            .flex_row()
            .h(px(CREATE_LIST_HEIGHT))
            .w_full()
            .border_t_1()
            .border_color(rgb(theme.grid_line));
        split = split.child(self.render_create_database_list(!show_tables, cx));
        if show_tables {
            split = split
                .child(div().w(px(1.0)).flex_none().h_full().bg(rgb(theme.border)))
                .child(self.render_create_table_panel(cx));
        }

        matrix.child(split).into_any_element()
    }

    /// The active database's privilege-level dropdown button.
    fn render_create_level_button(
        &self,
        index: usize,
        open: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let level = dialog
            .databases
            .get(index)
            .map(|row| row.level)
            .unwrap_or(CreateLevel::ReadOnly);
        div()
            .id("user-create-level")
            .w(px(CREATE_LEVEL_WIDTH))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(24.0))
            .px_2()
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(if open {
                theme.button_default_border
            } else {
                theme.border
            }))
            .bg(rgb(theme.dialog_bg))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_create_level_menu(index, cx)
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.text))
                    .child(t!(level.label_key()).to_string()),
            )
            .child(tree_icon("icons/chevron-down.svg", theme.text_muted))
            .into_any_element()
    }

    /// The active database's inline privilege-level list.
    fn render_create_level_options(&self, index: usize, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let current = self
            .create_user_dialog
            .as_ref()
            .and_then(|dialog| dialog.databases.get(index))
            .map(|row| row.level);
        let mut options = div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(theme.grid_line));
        for (option_index, option) in CreateLevel::ALL.into_iter().enumerate() {
            let active = current == Some(option);
            options = options.child(
                div()
                    .id(SharedString::from(format!(
                        "user-create-level-{index}-{option_index}"
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(24.0))
                    .pl(px(40.0))
                    .pr_3()
                    .cursor_pointer()
                    .when(active, move |style| style.bg(rgb(theme.tree_selected_bg)))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.choose_create_level(option, cx)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text))
                            .child(t!(option.label_key()).to_string()),
                    )
                    .child(if active {
                        tree_icon("icons/check.svg", theme.primary).into_any_element()
                    } else {
                        // Keep the row height stable when unselected.
                        div().w(px(14.0)).h(px(14.0)).flex_none().into_any_element()
                    }),
            );
        }
        options.into_any_element()
    }

    /// One 表范围 chip: `全部表` or `指定表`.
    fn create_scope_chip(
        &self,
        id: &'static str,
        label: String,
        active: bool,
        specific: bool,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .h(px(22.0))
            .px_3()
            .rounded(px(4.0))
            .text_size(px(11.5))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .border_1()
                    .border_color(rgb(theme.button_default_border))
                    .bg(rgb(theme.button_bg))
                    .text_color(rgb(theme.text))
            })
            .when(!active, move |style| {
                style
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(
                cx.listener(move |this, _event, _window, cx| this.set_create_scope(specific, cx)),
            )
            .child(label)
    }

    /// The 已选 N 张表 chip, which expands/collapses the table picker.
    fn render_create_tables_chip(
        &self,
        index: usize,
        open: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let count = self
            .create_user_dialog
            .as_ref()
            .and_then(|dialog| dialog.databases.get(index))
            .map(|row| row.tables.len())
            .unwrap_or(0);
        div()
            .id("user-create-tables-chip")
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(22.0))
            .px_2()
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.button_bg))
            .cursor_pointer()
            .on_click(cx.listener(|this, _event, _window, cx| this.toggle_create_tables_open(cx)))
            .child(tree_icon("icons/tables.svg", theme.icon_tables))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text))
                    .child(t!("user.create.tables_selected", count = count).to_string()),
            )
            .child(tree_icon(
                if open {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                },
                theme.text_muted,
            ))
            .into_any_element()
    }

    /// The database list; `full` makes it span the matrix when the table picker is collapsed.
    fn render_create_database_list(&self, full: bool, cx: &mut Context<'_, Self>) -> Stateful<Div> {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().id("user-create-db-scroll");
        };
        let needle = dialog.database_search.trim().to_ascii_lowercase();
        let active = dialog.active_database;
        let mut rows = div().flex().flex_col();
        let mut matched = false;
        for (index, row) in dialog.databases.iter().enumerate() {
            if !needle.is_empty() && !row.name.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            matched = true;
            let selected = row.selected;
            let is_active = active == Some(index);
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("user-create-db-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(CREATE_ROW_HEIGHT))
                    .px_3()
                    .when(is_active, move |style| {
                        style.bg(rgb(theme.tree_selected_bg))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(
                        div()
                            .id(SharedString::from(format!("user-create-db-check-{index}")))
                            .flex()
                            .flex_row()
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_create_database(index, cx)
                            }))
                            .child(checkbox_box(selected, theme)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("user-create-db-name-{index}")))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_1()
                            .min_w(px(0.0))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.activate_create_database(index, cx)
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(12.0))
                                    .child(row.name.clone()),
                            ),
                    ),
            );
        }
        if !matched {
            rows = rows.child(tree_message(
                t!("common.empty").to_string(),
                12.0,
                theme.text_muted,
            ));
        }
        div()
            .id("user-create-db-scroll")
            .flex()
            .flex_col()
            .h_full()
            .when(full, |list| list.w_full())
            .when(!full, |list| list.w(px(CREATE_DB_LIST_WIDTH)).flex_none())
            .overflow_y_scroll()
            .child(rows)
    }

    /// The 指定表 table picker beside the database list.
    fn render_create_table_panel(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div();
        };
        let Some(index) = dialog.active_database else {
            return div();
        };
        let Some(row) = dialog.databases.get(index) else {
            return div();
        };
        let database = row.name.clone();
        let selected_tables = row.tables.clone();
        let available = dialog.tables.get(&database).cloned().unwrap_or_default();
        let loading = dialog.loading_tables.contains(&database);
        let needle = dialog.table_search.trim().to_ascii_lowercase();
        let visible: Vec<String> = available
            .iter()
            .filter(|table| needle.is_empty() || table.to_ascii_lowercase().contains(&needle))
            .cloned()
            .collect();
        let all_selected = !available.is_empty()
            && available
                .iter()
                .all(|table| selected_tables.contains(table));

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(36.0))
            .px_2()
            .border_b_1()
            .border_color(rgb(theme.grid_line))
            .child(div().flex_1().min_w(px(0.0)).h(px(22.0)).child(
                match self.create_user_table_search.clone() {
                    Some(input) => input.into_any_element(),
                    None => div().into_any_element(),
                },
            ))
            .child(
                div()
                    .id("user-create-tables-all")
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(22.0))
                    .px_2()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_create_tables_all(!all_selected, cx)
                    }))
                    .child(checkbox_box(all_selected, theme))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(theme.text))
                            .child(t!("user.create.select_all").to_string()),
                    ),
            );

        let mut rows = div().flex().flex_col();
        if loading {
            rows = rows.child(tree_message(
                t!("common.loading").to_string(),
                8.0,
                theme.text_muted,
            ));
        } else if visible.is_empty() {
            rows = rows.child(tree_message(
                t!("common.empty").to_string(),
                8.0,
                theme.text_muted,
            ));
        }
        for (row_index, table) in visible.iter().enumerate() {
            let checked = selected_tables.contains(table);
            let name = table.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("user-create-table-{row_index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(CREATE_ROW_HEIGHT))
                    .px_2()
                    .cursor_pointer()
                    .when(row_index % 2 == 1, move |style| {
                        style.bg(rgb(theme.row_alt_bg))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_create_table(name.clone(), cx)
                    }))
                    .child(checkbox_box(checked, theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .child(table.clone()),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(header)
            .child(
                div()
                    .id("user-create-table-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(rows),
            )
    }

    /// The 管理用户 preset's warning banner (the window's risk callout).
    fn render_create_admin_warning(&self) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_start()
            .gap_2()
            .w_full()
            .p_3()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(0xef4444))
            .bg(rgba(0xef44441a))
            .text_size(px(12.0))
            .text_color(rgb(theme.danger))
            .child(tree_icon("icons/warning.svg", theme.danger))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(t!("user.create.admin_warning").to_string()),
            )
            .into_any_element()
    }

    /// The 预览 SQL pane at the bottom of the window.
    fn render_create_preview(&self) -> AnyElement {
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
            .map(|dialog| dialog.preview_scroll.clone())
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .child(t!("user.tab.sql").to_string()),
            )
            .child(
                div()
                    .id("user-create-sql")
                    .h(px(120.0))
                    .flex_none()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.input_bg))
                    .p_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text))
                            .child(text),
                    ),
            )
            .into_any_element()
    }

    /// The window's footer: 取消 / 预览 SQL / 保存.
    pub(super) fn create_user_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_user_dialog.as_ref() else {
            return div().into_any_element();
        };
        let busy = dialog.saving;
        let is_edit = dialog.is_edit();
        let subject = dialog.editor.account.user.trim().to_string();
        let save_label = if is_edit {
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
            .child(self.dialog_button(
                "user-create-preview",
                t!("user.create.preview_sql").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.toggle_create_preview(cx)),
            ))
            .child(self.dialog_button(
                "user-create-submit",
                save_label,
                true,
                cx.listener(move |this, _event, _window, cx| {
                    if !busy {
                        this.submit_create_user(cx);
                    }
                }),
            ));
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
                    .text_color(rgb(theme.danger))
                    .child(
                        self.create_user_dialog
                            .as_ref()
                            .and_then(|dialog| dialog.error.clone())
                            .unwrap_or_default(),
                    ),
            )
            .child(right)
            .into_any_element()
    }
}

/// The open privilege-level list of the create window: which database row it belongs to.
pub(super) struct CreateLevelMenu {
    pub(super) index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        name: &str,
        selected: bool,
        level: CreateLevel,
        specific: bool,
        tables: &[&str],
    ) -> CreateDatabaseRow {
        CreateDatabaseRow {
            name: name.to_string(),
            selected,
            level,
            specific,
            tables: tables.iter().map(|table| table.to_string()).collect(),
        }
    }

    fn dialog(kind: CreateKind, databases: Vec<CreateDatabaseRow>) -> UserCreateDialog {
        let mut dialog = UserCreateDialog::new(
            0,
            "test".to_string(),
            "caching_sha2_password".to_string(),
            None,
        );
        dialog.kind = kind;
        dialog.databases = databases;
        dialog
    }

    #[test]
    fn unselected_databases_are_skipped() {
        let dialog = dialog(
            CreateKind::Regular,
            vec![
                row("app", true, CreateLevel::ReadOnly, false, &[]),
                row("mysql", false, CreateLevel::All, false, &[]),
            ],
        );
        let grants = create_object_grants(&dialog);
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].database, "app");
        assert_eq!(grants[0].name, "");
        assert!(grants[0].privileges.contains(&Privilege::Select));
        assert!(!grants[0].privileges.contains(&Privilege::Insert));
    }

    #[test]
    fn specific_tables_expand_into_per_table_grants() {
        let dialog = dialog(
            CreateKind::Regular,
            vec![row(
                "app",
                true,
                CreateLevel::ReadWrite,
                true,
                &["orders", "users"],
            )],
        );
        let grants = create_object_grants(&dialog);
        assert_eq!(grants.len(), 2);
        assert_eq!(grants[0].name, "orders");
        assert_eq!(grants[1].name, "users");
        assert!(grants[0].privileges.contains(&Privilege::Insert));
    }

    #[test]
    fn specific_without_tables_falls_back_to_database_grant() {
        let dialog = dialog(
            CreateKind::Regular,
            vec![row("app", true, CreateLevel::ReadOnly, true, &[])],
        );
        let grants = create_object_grants(&dialog);
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].name, "");
    }

    #[test]
    fn admin_grants_every_database() {
        let dialog = dialog(
            CreateKind::Admin,
            vec![
                row("app", false, CreateLevel::ReadOnly, false, &[]),
                row("mysql", false, CreateLevel::ReadOnly, false, &[]),
            ],
        );
        let grants = create_object_grants(&dialog);
        assert_eq!(grants.len(), 2);
        assert!(
            grants
                .iter()
                .all(|grant| grant.privileges.contains(&Privilege::Select))
        );
        assert!(grants[0].privileges.len() > CreateLevel::ReadOnly.privileges().len());
    }

    #[test]
    fn selected_database_count_counts_checked_rows() {
        let dialog = dialog(
            CreateKind::Regular,
            vec![
                row("app", true, CreateLevel::ReadOnly, false, &[]),
                row("mysql", false, CreateLevel::ReadOnly, false, &[]),
                row("logs", true, CreateLevel::ReadOnly, false, &[]),
            ],
        );
        assert_eq!(dialog.selected_database_count(), 2);
    }
}

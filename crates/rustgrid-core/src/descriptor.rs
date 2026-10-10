//! Presentation and capability metadata for one driver.
//!
//! [`DriverDescriptor`] is the single source of truth for how the UI labels, icons, orders and
//! gates a driver. The app must not match on the engine id; it reads the descriptor instead.
//! [`Driver::descriptor`](crate::Driver::descriptor) builds one from the historical trait methods
//! by default, so adding it changes no behavior until an engine opts in.

use crate::capability::DriverCapabilities;
use crate::driver::DatabaseEditorSpec;
use crate::model::DriverId;

/// How a driver's icon is drawn in the connection tree and menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverIconStyle {
    /// A plain single-colour glyph tinted with the connection status colour.
    Plain,
    /// A white glyph drawn on a solid badge whose colour carries the connection status (the
    /// detailed MySQL/MariaDB brand marks).
    SolidBadge,
    /// The engine's own brand-coloured mark (SQLite, SQL Server).
    Brand(u32),
}

/// A page (tab) of the New/Edit Connection window, and where an engine-specific field is shown.
///
/// The vocabulary is engine-agnostic: a driver declares which pages it shows and where its own
/// fields go, so the form never matches on an engine id (mirrors [`DatabaseEditorTab`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConnectionPage {
    /// The first tab: the connection's identity (alias, network address or file, credentials).
    General,
    /// TLS / SSL.
    Tls,
    /// SSH / SOCKS5 / HTTP tunnel.
    Tunnel,
    /// Timeouts, read-only, session init SQL, plus engine fields placed here.
    Advanced,
}

impl ConnectionPage {
    /// i18n key of the page's tab label.
    pub fn label_key(self) -> &'static str {
        match self {
            ConnectionPage::General => "form.tab.general",
            ConnectionPage::Tls => "form.tab.tls",
            ConnectionPage::Tunnel => "form.tab.tunnel",
            ConnectionPage::Advanced => "form.tab.advanced",
        }
    }

    /// A stable id fragment for the tab's element id.
    pub fn id(self) -> &'static str {
        match self {
            ConnectionPage::General => "general",
            ConnectionPage::Tls => "tls",
            ConnectionPage::Tunnel => "tunnel",
            ConnectionPage::Advanced => "advanced",
        }
    }
}

/// The shape of the first (常规) page: which standard fields it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionHomePage {
    /// Alias + network address + port + credentials + default database (a network engine).
    Network,
    /// Alias + database file (a file-based engine, e.g. SQLite).
    File,
    /// Alias + the generic ODBC fields (driver name, DSN, connection string, engine hint).
    Odbc,
}

/// A standard form field whose label a driver may rename for its engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStandardField {
    Name,
    Host,
    Port,
    Username,
    Password,
    /// The "default database" field (Oracle renames it to a service name, SQL Server to the
    /// initial database, ...).
    Database,
    /// The file-based engine's database file.
    File,
}

/// A per-engine rename/hint of one standard field. Fields a driver does not list keep the app's
/// default label.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionFieldLabel {
    pub field: ConnectionStandardField,
    /// i18n key of the field's label.
    pub label_key: &'static str,
    /// i18n key of the field's label-row hint, if any.
    pub hint_key: Option<&'static str>,
}

/// One choice of a [`ConnectionFieldKind::Select`] field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFieldChoice {
    /// The value stored in `ConnectionProfile::options`.
    pub value: &'static str,
    /// i18n key of the choice's label.
    pub label_key: &'static str,
}

/// How one engine-specific connection field is edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionFieldKind {
    /// A plain single-line text field.
    Text,
    /// A masked single-line text field (a password or passphrase).
    Secret,
    /// A directory path with a trailing 浏览 (browse) button.
    Folder,
    /// A dropdown with a fixed set of choices; the first choice is the default.
    Select(Vec<ConnectionFieldChoice>),
}

/// One engine-specific field on the connection form.
///
/// The field reads and writes a single [`ConnectionProfile::options`] entry, so declaring a new
/// field needs no change to `connections.json` and no migration. Labels and hints are i18n keys
/// (a driver crate has no locale files of its own, so it hands the app the keys to resolve).
#[derive(Debug, Clone)]
pub struct ConnectionFieldSpec {
    /// The `ConnectionProfile::options` key this field reads and writes.
    pub key: &'static str,
    /// i18n key of the field's label.
    pub label_key: &'static str,
    /// i18n key of an optional hint shown on the label row.
    pub hint_key: Option<&'static str>,
    /// How the field is edited.
    pub kind: ConnectionFieldKind,
    /// Which page the field is shown on.
    pub page: ConnectionPage,
    /// Show the field only while `profile.options[0]` equals `1` (e.g. Oracle's TNS alias field is
    /// shown while the connection type is `tns`). A field that already holds a value is always
    /// shown, so a legacy profile can never hide an active option.
    pub visible_when: Option<(&'static str, &'static str)>,
}

/// Which pages the connection form shows for an engine, how its 常规 page is shaped, how it renames
/// the standard fields, and where its own option fields go. The form stays engine-agnostic by
/// reading this instead of matching on the engine id.
#[derive(Debug, Clone)]
pub struct ConnectionFormSpec {
    /// The shape of the 常规 page.
    pub home: ConnectionHomePage,
    /// The tabs shown after 常规, in order.
    pub tabs: Vec<ConnectionPage>,
    /// Per-engine renames/hints for the standard fields.
    pub labels: Vec<ConnectionFieldLabel>,
    /// Engine-specific option fields, each placed on a page.
    pub options: Vec<ConnectionFieldSpec>,
}

impl Default for ConnectionFormSpec {
    /// The standard network form: 常规 + TLS + 隧道 + 高级, with no engine fields. This is what a
    /// plain network engine (MySQL, PostgreSQL, ...) shows.
    fn default() -> Self {
        Self {
            home: ConnectionHomePage::Network,
            tabs: vec![
                ConnectionPage::Tls,
                ConnectionPage::Tunnel,
                ConnectionPage::Advanced,
            ],
            labels: Vec::new(),
            options: Vec::new(),
        }
    }
}

/// Presentation and capability metadata for one driver.
#[derive(Debug, Clone)]
pub struct DriverDescriptor {
    /// The engine id (e.g. `sqlserver`).
    pub id: DriverId,
    /// The user-facing engine name (e.g. `SQL Server`).
    pub display_name: String,
    /// The port pre-filled by the connection form.
    pub default_port: u16,
    /// Whether the engine connects to a file rather than a network server.
    pub is_file_based: bool,
    /// The embedded asset path of the engine's icon (e.g. `icons/sqlserver.svg`).
    pub icon: &'static str,
    /// How the icon is drawn in the connection tree.
    pub icon_style: DriverIconStyle,
    /// The capabilities the engine advertises to the UI.
    pub capabilities: DriverCapabilities,
    /// Which fields and tabs the New/Edit Database dialog shows for this engine.
    pub database_editor: DatabaseEditorSpec,
    /// Which extra fields the New/Edit Connection form shows for this engine.
    pub connection_form: ConnectionFormSpec,
    /// Sort key for the New Connection menu (lower first); ties fall back to the display name.
    pub order: u16,
}

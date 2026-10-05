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

/// Which extra fields the connection form shows for an engine. The form stays engine-agnostic by
/// reading these flags instead of matching on the engine id.
#[derive(Debug, Clone, Default)]
pub struct ConnectionFormSpec {
    /// Show the ODBC fields (driver name, DSN, connection string, engine hint).
    pub odbc: bool,
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
